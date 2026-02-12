mod agent;
mod breadcrumbs;
mod git;

use clap::Parser;
use std::path::PathBuf;
use sysinfo::{Pid, System};

use agent::{Agent, find_agent_by_env, find_agent_for_process};
use git::{append_trailers, find_git_root};

#[derive(Parser)]
#[command(name = "aittributor")]
#[command(about = "Git prepare-commit-msg hook that adds AI agent attribution")]
struct Cli {
    /// Path to the commit message file
    commit_msg_file: Option<PathBuf>,

    /// Commit message source (message, template, merge, squash, or commit)
    #[arg(default_value = "")]
    commit_source: String,

    /// Commit SHA (when amending)
    #[arg(default_value = "")]
    commit_sha: String,

    /// Enable debug output
    #[arg(long)]
    debug: bool,
}

fn walk_ancestry(system: &System, debug: bool) -> Option<&'static Agent> {
    let mut current_pid = Pid::from_u32(std::process::id());

    if debug {
        eprintln!("\nWalking ancestry from PID {}...", current_pid);
    }

    while let Some(process) = system.process(current_pid) {
        if debug {
            eprintln!("  PID {}: {:?}", current_pid, process.name());
        }
        if let Some(agent) = find_agent_for_process(process, debug) {
            return Some(agent);
        }

        match process.parent() {
            Some(parent_pid) if parent_pid != current_pid => {
                current_pid = parent_pid;
            }
            _ => break,
        }
    }

    None
}

fn check_process_tree(system: &System, root_pid: Pid, repo_path: &PathBuf, debug: bool) -> Option<&'static Agent> {
    let mut queue = std::collections::VecDeque::new();
    let mut visited = std::collections::HashSet::new();

    queue.push_back(root_pid);

    while let Some(pid) = queue.pop_front() {
        if !visited.insert(pid) {
            continue;
        }

        let process = match system.process(pid) {
            Some(p) => p,
            None => continue,
        };

        if debug {
            eprintln!("    Checking PID {}: {:?}", pid, process.name());
        }

        if let Some(agent) = find_agent_for_process(process, debug)
            && let Some(cwd) = process.cwd()
            && cwd.starts_with(repo_path)
        {
            if debug {
                eprintln!("    Found agent in tree with matching cwd");
            }
            return Some(agent);
        }

        for child in system.processes().values() {
            if child.parent() == Some(pid) {
                queue.push_back(child.pid());
            }
        }
    }

    None
}

fn walk_ancestry_and_descendants(system: &System, repo_path: &PathBuf, debug: bool) -> Option<&'static Agent> {
    let mut current_pid = Pid::from_u32(std::process::id());
    let mut checked_ancestors = std::collections::HashSet::new();

    if debug {
        eprintln!("\nWalking ancestry and descendants...");
    }

    loop {
        let process = system.process(current_pid)?;

        if !checked_ancestors.insert(current_pid) {
            break;
        }

        let parent_pid = match process.parent() {
            Some(pid) if pid != current_pid => pid,
            _ => break,
        };

        if debug {
            eprintln!("  Checking siblings of PID {} (parent: {})", current_pid, parent_pid);
        }

        for sibling in system.processes().values() {
            if sibling.parent() != Some(parent_pid) {
                continue;
            }

            if let Some(agent) = check_process_tree(system, sibling.pid(), repo_path, debug) {
                return Some(agent);
            }
        }

        current_pid = parent_pid;
    }

    None
}

fn detect_agent(debug: bool) -> Option<&'static Agent> {
    if debug {
        eprintln!("=== Agent Detection Debug ===");
        eprintln!("\nChecking environment variables...");
    }
    if let Some(agent) = find_agent_by_env() {
        if debug {
            eprintln!("  ✓ Found agent via env: {}", agent.email);
        }
        return Some(agent);
    }

    let current_dir = std::env::current_dir().ok()?;
    let repo_path = find_git_root(&current_dir).unwrap_or(current_dir);
    if debug {
        eprintln!("  Repository path: {}", repo_path.display());
    }
    let system = System::new_all();

    if let Some(agent) = walk_ancestry(&system, debug) {
        return Some(agent);
    }

    walk_ancestry_and_descendants(&system, &repo_path, debug)
}

fn breadcrumb_fallback(debug: bool) -> Vec<&'static Agent> {
    let current_dir = std::env::current_dir().unwrap_or_default();
    let repo_path = find_git_root(&current_dir).unwrap_or(current_dir);
    breadcrumbs::detect_agents_from_breadcrumbs(&repo_path, debug)
}

fn main() {
    let cli = Cli::parse();

    let Some(commit_msg_file) = cli.commit_msg_file else {
        if let Some(agent) = detect_agent(cli.debug) {
            println!("{}", agent.email);
        } else {
            let agents = breadcrumb_fallback(cli.debug);
            if agents.is_empty() {
                eprintln!("No agent found");
                std::process::exit(1);
            }
            for agent in agents {
                println!("{}", agent.email);
            }
        }
        return;
    };

    if let Some(agent) = detect_agent(cli.debug) {
        if let Err(e) = append_trailers(&commit_msg_file, agent, cli.debug) {
            eprintln!("aittributor: failed to append trailers: {}", e);
        }
    } else {
        for agent in breadcrumb_fallback(cli.debug) {
            if let Err(e) = append_trailers(&commit_msg_file, agent, cli.debug) {
                eprintln!("aittributor: failed to append trailers: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::{KNOWN_AGENTS, find_agent_by_name};
    use std::fs;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_find_agent_by_name() {
        assert!(find_agent_by_name("claude").is_some());
        assert!(find_agent_by_name("Claude").is_some());
        assert!(find_agent_by_name("claude-code").is_some());
        assert!(find_agent_by_name("cursor").is_some());
        assert!(find_agent_by_name("cursor-agent").is_some());
        assert!(find_agent_by_name("aider").is_some());
        assert!(find_agent_by_name("windsurf").is_some());
        assert!(find_agent_by_name("codex").is_some());
        assert!(find_agent_by_name("copilot-agent").is_some());
        assert!(find_agent_by_name("amazon-q").is_some());
        assert!(find_agent_by_name("amp").is_some());
        assert!(find_agent_by_name("/opt/homebrew/bin/amp").is_some());
        assert!(find_agent_by_name("gemini").is_some());
        assert!(find_agent_by_name("goose").is_some());
        assert!(find_agent_by_name("unknown").is_none());
    }

    #[test]
    fn test_find_agent_by_env() {
        unsafe {
            std::env::set_var("CLINE_ACTIVE", "true");
        }
        let agent = find_agent_by_env();
        assert!(agent.is_some());
        assert!(agent.unwrap().email.contains("Cline"));
        unsafe {
            std::env::remove_var("CLINE_ACTIVE");
        }
    }

    #[test]
    fn test_append_trailers() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "Initial commit").unwrap();

        let agent = &KNOWN_AGENTS[0];
        append_trailers(&file.path().to_path_buf(), agent, false).unwrap();

        let content = fs::read_to_string(file.path()).unwrap();
        assert!(content.contains("Co-authored-by: Claude Code <noreply@anthropic.com>"));
        assert!(content.contains("Ai-assisted: true"));
    }

    #[test]
    fn test_append_trailers_idempotent() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "Initial commit").unwrap();

        let agent = &KNOWN_AGENTS[0];
        append_trailers(&file.path().to_path_buf(), agent, false).unwrap();
        let content1 = fs::read_to_string(file.path()).unwrap();

        append_trailers(&file.path().to_path_buf(), agent, false).unwrap();
        let content2 = fs::read_to_string(file.path()).unwrap();

        assert_eq!(content1, content2);
    }

    #[test]
    fn test_find_git_root() {
        use std::fs;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let git_dir = temp_dir.path().join(".git");
        fs::create_dir(&git_dir).unwrap();

        let subdir = temp_dir.path().join("src").join("deep");
        fs::create_dir_all(&subdir).unwrap();

        let found = find_git_root(&subdir.to_path_buf());
        assert_eq!(found, Some(temp_dir.path().to_path_buf()));

        let found = find_git_root(&temp_dir.path().to_path_buf());
        assert_eq!(found, Some(temp_dir.path().to_path_buf()));
    }

    #[test]
    fn test_append_trailers_multiple_agents() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "Initial commit").unwrap();

        let agent1 = &KNOWN_AGENTS[0]; // Claude
        let agent2 = &KNOWN_AGENTS[8]; // Amp

        append_trailers(&file.path().to_path_buf(), agent1, false).unwrap();
        append_trailers(&file.path().to_path_buf(), agent2, false).unwrap();

        let content = fs::read_to_string(file.path()).unwrap();
        assert!(content.contains("Co-authored-by: Claude Code <noreply@anthropic.com>"));
        assert!(content.contains("Co-authored-by: Amp <amp@ampcode.com>"));

        let ai_assisted_count = content.matches("Ai-assisted: true").count();
        assert_eq!(
            ai_assisted_count, 1,
            "Ai-assisted trailer should appear exactly once, found {} occurrences",
            ai_assisted_count
        );
    }
}
