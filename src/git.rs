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

    let addr = Agent::extract_email_addr(agent.email);
    let content_lower = content.to_lowercase();
    let has_co_author = content_lower.contains("co-authored-by:") && content_lower.contains(&addr.to_lowercase());
    let has_ai_assisted = content_lower.contains("ai-assisted: true");

    if has_co_author && has_ai_assisted {
        if debug {
            eprintln!("\n=== Git Command ===");
            eprintln!("Trailers already present, skipping git interpret-trailers");
        }
        return Ok(());
    }

    let mut cmd = std::process::Command::new("git");
    cmd.arg("interpret-trailers").arg("--in-place");

    if !has_co_author {
        let co_authored = format!("Co-authored-by: {}", agent.email);
        if debug {
            eprintln!("\n=== Git Command ===");
            eprintln!(
                "git interpret-trailers --in-place --trailer \"{}\" --if-exists addIfDifferent --trailer \"Ai-assisted: true\" \"{}\"",
                co_authored,
                commit_msg_file.display()
            );
        }
        cmd.arg("--trailer")
            .arg(&co_authored)
            .arg("--if-exists")
            .arg("addIfDifferent");
    } else if debug {
        eprintln!("\n=== Git Command ===");
        eprintln!(
            "git interpret-trailers --in-place --trailer \"Ai-assisted: true\" \"{}\"",
            commit_msg_file.display()
        );
    }

    if !has_ai_assisted {
        cmd.arg("--trailer").arg("Ai-assisted: true");
    }

    cmd.arg(commit_msg_file);

    let output = cmd.output()?;

    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "git interpret-trailers failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}
