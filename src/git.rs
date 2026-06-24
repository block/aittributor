use std::fs;
use std::path::{Path, PathBuf};

use crate::agent::Agent;

/// Returns the current branch name for the repo, or `None` when it can't be
/// determined (e.g. detached HEAD, or `git` isn't available).
///
/// Uses `git symbolic-ref --short -q HEAD`, which prints the branch and exits
/// zero on a normal checkout, and prints nothing / exits non-zero on a detached
/// HEAD. We treat both failure and empty output as "unknown".
pub fn current_branch(repo_path: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(["symbolic-ref", "--short", "-q", "HEAD"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() { None } else { Some(branch) }
}

pub fn find_git_root(start_path: &Path) -> Option<PathBuf> {
    let mut current = start_path.to_path_buf();

    loop {
        let git_dir = current.join(".git");
        if git_dir.exists() {
            return Some(current);
        }

        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => return None,
        }
    }
}

pub fn append_trailers(commit_msg_file: &PathBuf, agent: &Agent, debug: bool) -> std::io::Result<()> {
    let content = fs::read_to_string(commit_msg_file)?;

    let addr = Agent::extract_email_addr(agent.email);
    let content_lower = content.to_lowercase();
    let has_co_author = content_lower.contains("co-authored-by:") && content_lower.contains(&addr.to_lowercase());

    if has_co_author {
        if debug {
            eprintln!("\n=== Git Command ===");
            eprintln!("Co-authored-by trailer already present, skipping git interpret-trailers");
        }
        return Ok(());
    }

    let co_authored = format!("Co-authored-by: {}", agent.email);

    if debug {
        eprintln!("\n=== Git Command ===");
        eprintln!(
            "git interpret-trailers --in-place --trailer \"{}\" --if-exists addIfDifferent \"{}\"",
            co_authored,
            commit_msg_file.display()
        );
    }

    let mut cmd = std::process::Command::new("git");
    cmd.arg("interpret-trailers")
        .arg("--in-place")
        .arg("--trailer")
        .arg(&co_authored)
        .arg("--if-exists")
        .arg("addIfDifferent")
        .arg(commit_msg_file);

    let output = cmd.output()?;

    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "git interpret-trailers failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn test_current_branch_reads_checked_out_branch() {
        let dir = TempDir::new().unwrap();
        let path = dir.path();

        // `git init` followed by pointing HEAD at a branch gives a deterministic
        // branch name without needing any commits or user git config.
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .arg("init")
                .arg("-q")
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["symbolic-ref", "HEAD", "refs/heads/feature-x"])
                .status()
                .unwrap()
                .success()
        );

        assert_eq!(current_branch(path).as_deref(), Some("feature-x"));
    }

    #[test]
    fn test_current_branch_none_outside_repo() {
        let dir = TempDir::new().unwrap();
        assert_eq!(current_branch(dir.path()), None);
    }
}
