use std::fs;
use std::path::{Path, PathBuf};

use crate::agent::Agent;

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

    if content.contains("Co-authored-by:") && content.contains(agent.email) {
        if debug {
            eprintln!("\n=== Git Command ===");
            eprintln!("Trailers already present, skipping git interpret-trailers");
        }
        return Ok(());
    }

    let co_authored = format!("Co-authored-by: {}", agent.email);

    if debug {
        eprintln!("\n=== Git Command ===");
        eprintln!(
            "git interpret-trailers --in-place --trailer \"{}\" --if-exists addIfDifferent --trailer \"Ai-assisted: true\" \"{}\"",
            co_authored,
            commit_msg_file.display()
        );
    }

    let output = std::process::Command::new("git")
        .arg("interpret-trailers")
        .arg("--in-place")
        .arg("--trailer")
        .arg(&co_authored)
        .arg("--if-exists")
        .arg("addIfDifferent")
        .arg("--trailer")
        .arg("Ai-assisted: true")
        .arg(commit_msg_file)
        .output()?;

    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "git interpret-trailers failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}
