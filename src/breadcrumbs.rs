use std::fs;
use std::io::BufRead;
use std::path::Path;
use std::time::SystemTime;

use crate::agent::{Agent, KNOWN_AGENTS};
use crate::git;

const CUTOFF_SECS: u64 = 2 * 60 * 60; // 2 hours as a rough approximation

/// Maximum number of lines to read from a session file when looking for "cwd".
const MAX_LINES_TO_SCAN: usize = 5;

/// Shared inputs for a breadcrumb scan: which repo we're matching, how recent a
/// session must be, and the branch the commit is happening on (if known).
struct ScanContext<'a> {
    repo_path: &'a Path,
    cutoff: SystemTime,
    current_branch: Option<&'a str>,
}

/// Outcome of scanning a breadcrumb directory for a single agent.
enum SessionScan {
    /// A recent session whose cwd and branch both matched the commit.
    Matched,
    /// A recent session matched the repo but was recorded on a different
    /// branch, so it was skipped. Carries that session's branch for debug.
    BranchMismatch(String),
    /// No recent session for this repo.
    None,
}

fn home_dir() -> Option<String> {
    std::env::var("HOME").ok()
}

fn is_recent(path: &Path, cutoff: SystemTime) -> bool {
    path.metadata()
        .and_then(|m| m.modified())
        .is_ok_and(|mtime| mtime >= cutoff)
}

fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some(ext)
}

/// Extract a JSON string value for `key` from a line via a simple substring
/// scan: find `"<key>":"` and read until the next `"`. This avoids a full JSON
/// parse, which matters because session lines can be very large.
fn extract_json_string<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("\"{}\":\"", key);
    let start = line.find(&marker)? + marker.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn extract_cwd_from_json(line: &str) -> Option<&str> {
    extract_json_string(line, "cwd")
}

/// Extract the branch a session was recorded on. Codex stores it as `branch`
/// (inside its `git` object) and Claude as `gitBranch`; we try both.
fn extract_branch_from_json(line: &str) -> Option<&str> {
    extract_json_string(line, "gitBranch").or_else(|| extract_json_string(line, "branch"))
}

fn cwd_matches_repo(cwd: &str, repo_path: &Path) -> bool {
    Path::new(cwd).starts_with(repo_path)
}

/// Decide whether a session's branch is compatible with the commit's branch.
///
/// We only reject when *both* branches are known and differ; if either side is
/// unknown we fall back to cwd-only matching to avoid false negatives.
fn branch_matches(session_branch: Option<&str>, current_branch: Option<&str>) -> bool {
    match (session_branch, current_branch) {
        (Some(session), Some(current)) => session == current,
        _ => true,
    }
}

/// Scan the first few lines of one session file. The `cwd` and branch fields
/// live on the same line (Codex's `session_meta`, each of Claude's messages),
/// so we evaluate both as soon as we find the line carrying `cwd`.
fn scan_file(path: &Path, ctx: &ScanContext) -> SessionScan {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return SessionScan::None,
    };
    let reader = std::io::BufReader::new(file);

    for line in reader.lines().take(MAX_LINES_TO_SCAN) {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let Some(cwd) = extract_cwd_from_json(&line) else {
            continue;
        };
        if !cwd_matches_repo(cwd, ctx.repo_path) {
            return SessionScan::None;
        }
        let session_branch = extract_branch_from_json(&line);
        if branch_matches(session_branch, ctx.current_branch) {
            return SessionScan::Matched;
        }
        return SessionScan::BranchMismatch(session_branch.unwrap_or_default().to_string());
    }

    SessionScan::None
}

/// Walk nested subdirectories (any depth) looking for a recent session file
/// whose `cwd` matches the repo. A full (cwd + branch) match wins immediately;
/// otherwise we remember any branch-mismatched session so the caller can
/// explain why the agent was skipped.
fn scan_breadcrumb_dir(dir: &Path, ext: &str, ctx: &ScanContext) -> SessionScan {
    let mut dirs_to_visit = vec![dir.to_path_buf()];
    let mut branch_mismatch: Option<String> = None;

    while let Some(current) = dirs_to_visit.pop() {
        let entries = match fs::read_dir(&current) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs_to_visit.push(path);
                continue;
            }
            if !has_extension(&path, ext) || !is_recent(&path, ctx.cutoff) {
                continue;
            }
            match scan_file(&path, ctx) {
                SessionScan::Matched => return SessionScan::Matched,
                SessionScan::BranchMismatch(branch) => branch_mismatch = Some(branch),
                SessionScan::None => {}
            }
        }
    }

    match branch_mismatch {
        Some(branch) => SessionScan::BranchMismatch(branch),
        None => SessionScan::None,
    }
}

fn check_source(agent: &'static Agent, ctx: &ScanContext, log: &mut Vec<String>, debug: bool) -> bool {
    let breadcrumb_dir = match agent.breadcrumb_dir {
        Some(d) => d,
        None => return false,
    };
    let breadcrumb_ext = agent.breadcrumb_ext.unwrap_or("jsonl");

    let home = match home_dir() {
        Some(h) => h,
        None => return false,
    };
    let base = Path::new(&home).join(breadcrumb_dir);

    // Only agents whose breadcrumb directory actually exists are worth
    // reporting; skipping silently keeps the debug output focused.
    if !base.is_dir() {
        return false;
    }

    let scan = scan_breadcrumb_dir(&base, breadcrumb_ext, ctx);

    if debug {
        match &scan {
            SessionScan::Matched => log.push(format!("  found {} ({})", agent.email, base.display())),
            SessionScan::BranchMismatch(branch) => log.push(format!(
                "  scanned {} (recent session on branch '{}', current '{}') — skipped",
                base.display(),
                branch,
                ctx.current_branch.unwrap_or("unknown")
            )),
            SessionScan::None => log.push(format!("  scanned {} (no recent session in repo)", base.display())),
        }
    }

    matches!(scan, SessionScan::Matched)
}

pub fn detect_agents_from_breadcrumbs(repo_path: &Path, log: &mut Vec<String>, debug: bool) -> Vec<&'static Agent> {
    let current_branch = git::current_branch(repo_path);
    let ctx = ScanContext {
        repo_path,
        cutoff: SystemTime::now() - std::time::Duration::from_secs(CUTOFF_SECS),
        current_branch: current_branch.as_deref(),
    };
    let mut agents = Vec::new();

    if debug {
        match ctx.current_branch {
            Some(branch) => log.push(format!("strategy: breadcrumb session files (branch: {})", branch)),
            None => log.push("strategy: breadcrumb session files (branch: unknown)".to_string()),
        }
    }

    for agent in KNOWN_AGENTS {
        if check_source(agent, &ctx, log, debug) {
            agents.push(agent);
        }
    }

    if debug && agents.is_empty() {
        log.push("  no match".to_string());
    }

    agents
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn test_extract_cwd_from_json() {
        let line = r#"{"type":"session_meta","cwd":"/Users/foo/myrepo","branch":"main"}"#;
        assert_eq!(extract_cwd_from_json(line), Some("/Users/foo/myrepo"));
    }

    #[test]
    fn test_extract_cwd_missing() {
        let line = r#"{"type":"session_meta","branch":"main"}"#;
        assert_eq!(extract_cwd_from_json(line), None);
    }

    #[test]
    fn test_cwd_matches_repo_uses_path_components() {
        assert!(cwd_matches_repo(
            "/Users/foo/monorepo/apps/service-a",
            Path::new("/Users/foo/monorepo")
        ));
        assert!(!cwd_matches_repo(
            "/Users/foo/aittributor2",
            Path::new("/Users/foo/aittributor")
        ));
    }

    #[test]
    fn test_no_breadcrumbs_returns_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let agents = detect_agents_from_breadcrumbs(dir.path(), &mut Vec::new(), false);
        assert!(agents.is_empty());
    }

    /// Builds a `ScanContext` with a generous recency window for tests.
    fn test_ctx<'a>(repo: &'a Path, branch: Option<&'a str>) -> ScanContext<'a> {
        ScanContext {
            repo_path: repo,
            cutoff: SystemTime::now() - std::time::Duration::from_secs(10),
            current_branch: branch,
        }
    }

    #[test]
    fn test_extract_branch_from_json() {
        // Codex: branch lives inside the git object.
        let codex = r#"{"cwd":"/r","git":{"branch":"feature-x"}}"#;
        assert_eq!(extract_branch_from_json(codex), Some("feature-x"));
        // Claude: camelCase gitBranch.
        let claude = r#"{"cwd":"/r","gitBranch":"main"}"#;
        assert_eq!(extract_branch_from_json(claude), Some("main"));
        // Absent.
        assert_eq!(extract_branch_from_json(r#"{"cwd":"/r"}"#), None);
    }

    #[test]
    fn test_branch_matches_only_rejects_when_both_known_and_differ() {
        assert!(branch_matches(Some("main"), Some("main")));
        assert!(!branch_matches(Some("main"), Some("feature")));
        // Unknown on either side falls back to a match (cwd-only behaviour).
        assert!(branch_matches(None, Some("main")));
        assert!(branch_matches(Some("main"), None));
        assert!(branch_matches(None, None));
    }

    #[test]
    fn test_scan_file_cwd_on_line_1() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"session_meta","cwd":"/Users/foo/myrepo"}}"#).unwrap();

        assert!(matches!(
            scan_file(&path, &test_ctx(Path::new("/Users/foo/myrepo"), None)),
            SessionScan::Matched
        ));
        assert!(matches!(
            scan_file(&path, &test_ctx(Path::new("/Users/bar/other"), None)),
            SessionScan::None
        ));
    }

    #[test]
    fn test_scan_file_cwd_on_line_2() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"file-history-snapshot","messageId":"abc"}}"#).unwrap();
        writeln!(f, r#"{{"type":"user","cwd":"/Users/foo/myrepo"}}"#).unwrap();

        assert!(matches!(
            scan_file(&path, &test_ctx(Path::new("/Users/foo/myrepo"), None)),
            SessionScan::Matched
        ));
    }

    #[test]
    fn test_scan_file_no_cwd_field() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"something","data":"value"}}"#).unwrap();
        writeln!(f, r#"{{"type":"other","data":"value"}}"#).unwrap();

        assert!(matches!(
            scan_file(&path, &test_ctx(Path::new("/Users/foo/myrepo"), None)),
            SessionScan::None
        ));
    }

    #[test]
    fn test_scan_file_branch_match_and_mismatch() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"session_meta","cwd":"/Users/foo/myrepo","git":{{"branch":"feature"}}}}"#
        )
        .unwrap();
        let repo = Path::new("/Users/foo/myrepo");

        assert!(matches!(
            scan_file(&path, &test_ctx(repo, Some("feature"))),
            SessionScan::Matched
        ));
        assert!(matches!(
            scan_file(&path, &test_ctx(repo, Some("main"))),
            SessionScan::BranchMismatch(b) if b == "feature"
        ));
        // Unknown current branch falls back to cwd-only matching.
        assert!(matches!(scan_file(&path, &test_ctx(repo, None)), SessionScan::Matched));
    }

    #[test]
    fn test_scan_breadcrumb_dir_matches() {
        let dir = tempfile::TempDir::new().unwrap();
        let day_dir = dir.path().join("2025").join("06").join("15");
        fs::create_dir_all(&day_dir).unwrap();
        let mut f = fs::File::create(day_dir.join("session.jsonl")).unwrap();
        writeln!(f, r#"{{"type":"session_meta","cwd":"/Users/foo/myrepo"}}"#).unwrap();

        assert!(matches!(
            scan_breadcrumb_dir(dir.path(), "jsonl", &test_ctx(Path::new("/Users/foo/myrepo"), None)),
            SessionScan::Matched
        ));
        assert!(matches!(
            scan_breadcrumb_dir(dir.path(), "jsonl", &test_ctx(Path::new("/Users/bar/other"), None)),
            SessionScan::None
        ));
    }

    #[test]
    fn test_scan_breadcrumb_dir_rejects_sibling_prefix_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        let day_dir = dir.path().join("2025").join("06").join("15");
        fs::create_dir_all(&day_dir).unwrap();
        let mut f = fs::File::create(day_dir.join("session.jsonl")).unwrap();
        writeln!(f, r#"{{"type":"session_meta","cwd":"/Users/foo/aittributor2"}}"#).unwrap();

        assert!(matches!(
            scan_breadcrumb_dir(
                dir.path(),
                "jsonl",
                &test_ctx(Path::new("/Users/foo/aittributor"), None)
            ),
            SessionScan::None
        ));
    }

    #[test]
    fn test_scan_breadcrumb_dir_matches_monorepo_sibling_subdir() {
        let dir = tempfile::TempDir::new().unwrap();
        let day_dir = dir.path().join("2025").join("06").join("15");
        fs::create_dir_all(&day_dir).unwrap();
        let mut f = fs::File::create(day_dir.join("session.jsonl")).unwrap();
        writeln!(
            f,
            r#"{{"type":"session_meta","cwd":"/Users/foo/monorepo/apps/backend"}}"#
        )
        .unwrap();

        // Commit can run from another folder in the same repo; we match by git root.
        assert!(matches!(
            scan_breadcrumb_dir(dir.path(), "jsonl", &test_ctx(Path::new("/Users/foo/monorepo"), None)),
            SessionScan::Matched
        ));
    }

    #[test]
    fn test_scan_breadcrumb_dir_prefers_branch_match_over_mismatch() {
        let dir = tempfile::TempDir::new().unwrap();
        let day_dir = dir.path().join("2025").join("06").join("15");
        fs::create_dir_all(&day_dir).unwrap();
        let repo = "/Users/foo/myrepo";

        let mut wrong = fs::File::create(day_dir.join("wrong-branch.jsonl")).unwrap();
        writeln!(wrong, r#"{{"cwd":"{repo}","git":{{"branch":"old"}}}}"#).unwrap();
        let mut right = fs::File::create(day_dir.join("right-branch.jsonl")).unwrap();
        writeln!(right, r#"{{"cwd":"{repo}","git":{{"branch":"current"}}}}"#).unwrap();

        assert!(matches!(
            scan_breadcrumb_dir(dir.path(), "jsonl", &test_ctx(Path::new(repo), Some("current"))),
            SessionScan::Matched
        ));
    }

    #[test]
    fn test_scan_breadcrumb_dir_reports_branch_mismatch_when_no_match() {
        let dir = tempfile::TempDir::new().unwrap();
        let day_dir = dir.path().join("2025").join("06").join("15");
        fs::create_dir_all(&day_dir).unwrap();
        let repo = "/Users/foo/myrepo";

        let mut f = fs::File::create(day_dir.join("session.jsonl")).unwrap();
        writeln!(f, r#"{{"cwd":"{repo}","git":{{"branch":"old"}}}}"#).unwrap();

        assert!(matches!(
            scan_breadcrumb_dir(dir.path(), "jsonl", &test_ctx(Path::new(repo), Some("current"))),
            SessionScan::BranchMismatch(b) if b == "old"
        ));
    }
}
