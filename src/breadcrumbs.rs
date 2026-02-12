use std::fs;
use std::io::BufRead;
use std::path::Path;
use std::time::SystemTime;

use crate::agent::{Agent, KNOWN_AGENTS};

const CUTOFF_SECS: u64 = 2 * 60 * 60; // 2 hours as a rough approximation

/// Maximum number of lines to read from a session file when looking for "cwd".
const MAX_LINES_TO_SCAN: usize = 5;

struct BreadcrumbSource {
    /// Prefix to match against Agent.email in KNOWN_AGENTS
    email_prefix: &'static str,
    /// Base directory relative to $HOME (e.g. ".claude/projects")
    base_dir: &'static str,
    /// File extension to look for (without dot)
    file_ext: &'static str,
}

const SOURCES: &[BreadcrumbSource] = &[
    BreadcrumbSource {
        email_prefix: "Claude Code",
        base_dir: ".claude/projects",
        file_ext: "jsonl",
    },
    BreadcrumbSource {
        email_prefix: "Codex",
        base_dir: ".codex/sessions",
        file_ext: "jsonl",
    },
];

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

fn find_agent(email_prefix: &str) -> Option<&'static Agent> {
    KNOWN_AGENTS.iter().find(|a| a.email.starts_with(email_prefix))
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
fn file_has_matching_cwd(path: &Path, repo_path: &Path, debug: bool) -> bool {
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
            if debug {
                eprintln!("    {} cwd: {}", path.display(), cwd);
            }
            return cwd_matches_repo(cwd, repo_path);
        }
    }

    false
}

/// Walk nested subdirectories (any depth) looking for recent files whose
/// first few lines contain a "cwd" field matching the repo path.
fn find_session_file_with_cwd(dir: &Path, ext: &str, repo_path: &Path, cutoff: SystemTime, debug: bool) -> bool {
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
            if file_has_matching_cwd(&path, repo_path, debug) {
                return true;
            }
        }
    }

    false
}

fn check_source(
    source: &BreadcrumbSource,
    repo_path: &Path,
    cutoff: SystemTime,
    debug: bool,
) -> Option<&'static Agent> {
    let home = home_dir()?;
    let base = Path::new(&home).join(source.base_dir);

    if debug {
        eprintln!("  {} breadcrumb dir: {}", source.email_prefix, base.display());
    }

    if !base.is_dir() {
        if debug {
            eprintln!("    Not found");
        }
        return None;
    }

    let matched = find_session_file_with_cwd(&base, source.file_ext, repo_path, cutoff, debug);

    if matched {
        find_agent(source.email_prefix)
    } else {
        if debug {
            eprintln!("    No match for {}", source.email_prefix);
        }
        None
    }
}

pub fn detect_agents_from_breadcrumbs(repo_path: &Path, debug: bool) -> Vec<&'static Agent> {
    let cutoff = SystemTime::now() - std::time::Duration::from_secs(CUTOFF_SECS);
    let mut agents = Vec::new();

    if debug {
        eprintln!("\n=== Breadcrumb Fallback ===");
    }

    for source in SOURCES {
        if let Some(agent) = check_source(source, repo_path, cutoff, debug) {
            agents.push(agent);
        }
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
        let agents = detect_agents_from_breadcrumbs(dir.path(), false);
        assert!(agents.is_empty());
    }

    #[test]
    fn test_file_has_matching_cwd_on_line_1() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"session_meta","cwd":"/Users/foo/myrepo"}}"#).unwrap();

        assert!(file_has_matching_cwd(&path, Path::new("/Users/foo/myrepo"), false));
        assert!(!file_has_matching_cwd(&path, Path::new("/Users/bar/other"), false));
    }

    #[test]
    fn test_file_has_matching_cwd_on_line_2() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"file-history-snapshot","messageId":"abc"}}"#).unwrap();
        writeln!(f, r#"{{"type":"user","cwd":"/Users/foo/myrepo"}}"#).unwrap();

        assert!(file_has_matching_cwd(&path, Path::new("/Users/foo/myrepo"), false));
        assert!(!file_has_matching_cwd(&path, Path::new("/Users/bar/other"), false));
    }

    #[test]
    fn test_file_has_matching_cwd_no_cwd_field() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"something","data":"value"}}"#).unwrap();
        writeln!(f, r#"{{"type":"other","data":"value"}}"#).unwrap();

        assert!(!file_has_matching_cwd(&path, Path::new("/Users/foo/myrepo"), false));
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
            cutoff,
            false
        ));

        // Non-matching repo
        assert!(!find_session_file_with_cwd(
            dir.path(),
            "jsonl",
            Path::new("/Users/bar/other"),
            cutoff,
            false
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
            cutoff,
            false
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
            cutoff,
            false
        ));
    }
}
