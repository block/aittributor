use std::fs;
use std::io::BufRead;
use std::path::Path;
use std::time::SystemTime;

use crate::agent::{Agent, KNOWN_AGENTS};

const CUTOFF_SECS: u64 = 2 * 60 * 60; // 2 hours as a rough approximation

/// Maximum number of lines to read from a session file when looking for "cwd".
const MAX_LINES_TO_SCAN: usize = 5;

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

fn extract_cwd_from_json(line: &str) -> Option<&str> {
    // Simple string extraction: find "cwd":"<value>"
    let marker = "\"cwd\":\"";
    let start = line.find(marker)? + marker.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn cwd_matches_repo(cwd: &str, repo_path: &Path) -> bool {
    Path::new(cwd).starts_with(repo_path)
}

/// Read the first few lines of a file looking for a "cwd" field that
/// matches the repo path. Returns true on match.
fn file_has_matching_cwd(path: &Path, repo_path: &Path) -> bool {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let reader = std::io::BufReader::new(file);

    for line in reader.lines().take(MAX_LINES_TO_SCAN) {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if let Some(cwd) = extract_cwd_from_json(&line) {
            return cwd_matches_repo(cwd, repo_path);
        }
    }

    false
}

/// Walk nested subdirectories (any depth) looking for recent files whose
/// first few lines contain a "cwd" field matching the repo path.
fn find_session_file_with_cwd(dir: &Path, ext: &str, repo_path: &Path, cutoff: SystemTime) -> bool {
    let mut dirs_to_visit = vec![dir.to_path_buf()];

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
            if !has_extension(&path, ext) || !is_recent(&path, cutoff) {
                continue;
            }
            if file_has_matching_cwd(&path, repo_path) {
                return true;
            }
        }
    }

    false
}

fn check_source(
    agent: &'static Agent,
    repo_path: &Path,
    cutoff: SystemTime,
    log: &mut Vec<String>,
    debug: bool,
) -> bool {
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

    let matched = find_session_file_with_cwd(&base, breadcrumb_ext, repo_path, cutoff);

    if debug {
        if matched {
            log.push(format!("  found {} ({})", agent.email, base.display()));
        } else {
            log.push(format!("  scanned {} (no recent session in repo)", base.display()));
        }
    }

    matched
}

pub fn detect_agents_from_breadcrumbs(repo_path: &Path, log: &mut Vec<String>, debug: bool) -> Vec<&'static Agent> {
    let cutoff = SystemTime::now() - std::time::Duration::from_secs(CUTOFF_SECS);
    let mut agents = Vec::new();

    if debug {
        log.push("strategy: breadcrumb session files".to_string());
    }

    for agent in KNOWN_AGENTS {
        if check_source(agent, repo_path, cutoff, log, debug) {
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

    #[test]
    fn test_file_has_matching_cwd_on_line_1() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"session_meta","cwd":"/Users/foo/myrepo"}}"#).unwrap();

        assert!(file_has_matching_cwd(&path, Path::new("/Users/foo/myrepo")));
        assert!(!file_has_matching_cwd(&path, Path::new("/Users/bar/other")));
    }

    #[test]
    fn test_file_has_matching_cwd_on_line_2() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"file-history-snapshot","messageId":"abc"}}"#).unwrap();
        writeln!(f, r#"{{"type":"user","cwd":"/Users/foo/myrepo"}}"#).unwrap();

        assert!(file_has_matching_cwd(&path, Path::new("/Users/foo/myrepo")));
        assert!(!file_has_matching_cwd(&path, Path::new("/Users/bar/other")));
    }

    #[test]
    fn test_file_has_matching_cwd_no_cwd_field() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"something","data":"value"}}"#).unwrap();
        writeln!(f, r#"{{"type":"other","data":"value"}}"#).unwrap();

        assert!(!file_has_matching_cwd(&path, Path::new("/Users/foo/myrepo")));
    }

    #[test]
    fn test_find_session_file_with_cwd() {
        let dir = tempfile::TempDir::new().unwrap();
        let cutoff = SystemTime::now() - std::time::Duration::from_secs(10);

        // Create nested date dirs
        let day_dir = dir.path().join("2025").join("06").join("15");
        fs::create_dir_all(&day_dir).unwrap();

        // Write a session file with cwd
        let mut f = fs::File::create(day_dir.join("session.jsonl")).unwrap();
        writeln!(f, r#"{{"type":"session_meta","cwd":"/Users/foo/myrepo"}}"#).unwrap();

        // Matching repo
        assert!(find_session_file_with_cwd(
            dir.path(),
            "jsonl",
            Path::new("/Users/foo/myrepo"),
            cutoff
        ));

        // Non-matching repo
        assert!(!find_session_file_with_cwd(
            dir.path(),
            "jsonl",
            Path::new("/Users/bar/other"),
            cutoff
        ));
    }

    #[test]
    fn test_find_session_file_with_cwd_rejects_sibling_prefix_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        let cutoff = SystemTime::now() - std::time::Duration::from_secs(10);
        let day_dir = dir.path().join("2025").join("06").join("15");
        fs::create_dir_all(&day_dir).unwrap();

        let mut f = fs::File::create(day_dir.join("session.jsonl")).unwrap();
        writeln!(f, r#"{{"type":"session_meta","cwd":"/Users/foo/aittributor2"}}"#).unwrap();

        assert!(!find_session_file_with_cwd(
            dir.path(),
            "jsonl",
            Path::new("/Users/foo/aittributor"),
            cutoff
        ));
    }

    #[test]
    fn test_find_session_file_with_cwd_matches_monorepo_sibling_subdir() {
        let dir = tempfile::TempDir::new().unwrap();
        let cutoff = SystemTime::now() - std::time::Duration::from_secs(10);
        let day_dir = dir.path().join("2025").join("06").join("15");
        fs::create_dir_all(&day_dir).unwrap();

        let mut f = fs::File::create(day_dir.join("session.jsonl")).unwrap();
        writeln!(
            f,
            r#"{{"type":"session_meta","cwd":"/Users/foo/monorepo/apps/backend"}}"#
        )
        .unwrap();

        // Commit can run from another folder in the same repo; we match by git root.
        assert!(find_session_file_with_cwd(
            dir.path(),
            "jsonl",
            Path::new("/Users/foo/monorepo"),
            cutoff
        ));
    }
}
