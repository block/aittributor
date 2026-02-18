mod agent;
mod breadcrumbs;
mod git;

use clap::Parser;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System, UpdateKind};

use agent::Agent;
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

fn walk_ancestry(system: &System, debug: bool) -> Vec<&'static Agent> {
    let mut current_pid = Pid::from_u32(std::process::id());
    let mut agents = Vec::new();

    if debug {
        eprintln!("\nWalking ancestry from PID {}...", current_pid);
    }

    while let Some(process) = system.process(current_pid) {
        if debug {
            eprintln!("  PID {}: {:?}", current_pid, process.name());
        }
        if let Some(agent) = Agent::find_for_process(process, debug) {
            agents.push(agent);
        }

        match process.parent() {
            Some(parent_pid) if parent_pid != current_pid => {
                current_pid = parent_pid;
            }
            _ => break,
        }
    }

    agents
}

fn check_process_tree(system: &System, root_pid: Pid, repo_path: &PathBuf, debug: bool) -> Vec<&'static Agent> {
    let mut queue = std::collections::VecDeque::new();
    let mut visited = std::collections::HashSet::new();
    let mut agents = Vec::new();

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

        if let Some(agent) = Agent::find_for_process(process, debug)
            && let Some(cwd) = process.cwd()
            && cwd.starts_with(repo_path)
        {
            if debug {
                eprintln!("    Found agent in tree with matching cwd");
            }
            agents.push(agent);
        }

        for child in system.processes().values() {
            if child.parent() == Some(pid) {
                queue.push_back(child.pid());
            }
        }
    }

    agents
}

fn walk_ancestry_and_descendants(system: &System, repo_path: &PathBuf, debug: bool) -> Vec<&'static Agent> {
    let mut current_pid = Pid::from_u32(std::process::id());
    let mut checked_ancestors = std::collections::HashSet::new();
    let mut agents = Vec::new();

    if debug {
        eprintln!("\nWalking ancestry and descendants...");
    }

    while let Some(process) = system.process(current_pid) {
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

            agents.extend(check_process_tree(system, sibling.pid(), repo_path, debug));
        }

        current_pid = parent_pid;
    }

    agents
}

fn detect_agents(debug: bool) -> Vec<&'static Agent> {
    let mut agents = Vec::new();

    if debug {
        eprintln!("=== Agent Detection Debug ===");
        eprintln!("\nChecking environment variables...");
    }
    if let Some(agent) = Agent::find_by_env() {
        if debug {
            eprintln!("  ✓ Found agent via env: {}", agent.email);
        }
        agents.push(agent);
    }

    let current_dir = match std::env::current_dir() {
        Ok(d) => d,
        Err(_) => return agents,
    };
    let repo_path = find_git_root(&current_dir).unwrap_or(current_dir);
    if debug {
        eprintln!("  Repository path: {}", repo_path.display());
    }
    let system = System::new_with_specifics(
        RefreshKind::new().with_processes(
            ProcessRefreshKind::new()
                .with_cmd(UpdateKind::Always)
                .with_cwd(UpdateKind::Always),
        ),
    );

    agents.extend(walk_ancestry(&system, debug));
    agents.extend(walk_ancestry_and_descendants(&system, &repo_path, debug));

    agents
}

fn dedup_agents(agents: Vec<&'static Agent>) -> Vec<&'static Agent> {
    let mut seen = std::collections::HashSet::new();
    agents
        .into_iter()
        .filter(|a| {
            let addr = Agent::extract_email_addr(a.email);
            seen.insert(addr)
        })
        .collect()
}

fn breadcrumb_fallback(debug: bool) -> Vec<&'static Agent> {
    let current_dir = std::env::current_dir().unwrap_or_default();
    let repo_path = find_git_root(&current_dir).unwrap_or(current_dir);
    breadcrumbs::detect_agents_from_breadcrumbs(&repo_path, debug)
}

fn detect_and_merge(debug: bool) -> Vec<&'static Agent> {
    let (bc_tx, bc_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = bc_tx.send(breadcrumb_fallback(debug));
    });

    let mut agents = detect_agents(debug);

    if let Ok(bc_agents) = bc_rx.recv() {
        agents.extend(bc_agents);
    }

    dedup_agents(agents)
}

fn run(cli: Cli) {
    let agents = detect_and_merge(cli.debug);

    let Some(commit_msg_file) = cli.commit_msg_file else {
        if agents.is_empty() {
            eprintln!("No agent found");
            std::process::exit(1);
        }
        for agent in &agents {
            println!("{}", agent.email);
        }
        return;
    };

    for agent in &agents {
        if let Err(e) = append_trailers(&commit_msg_file, agent, cli.debug) {
            eprintln!("aittributor: failed to append trailers: {}", e);
        }
    }
}

fn main() {
    let cli = Cli::parse();

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        run(cli);
        let _ = tx.send(());
    });

    if rx.recv_timeout(Duration::from_secs(1)).is_err() {
        eprintln!("aittributor: timed out, skipping attribution. Check https://github.com/block/aittributor/issues");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::KNOWN_AGENTS;
    use std::fs;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_append_trailers_skips_existing_email_different_name() {
        // Simulate Claude Code already having added a trailer with a different display name
        // but the same email address (e.g. "Claude Opus 4.6 <noreply@anthropic.com>")
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "Initial commit").unwrap();
        writeln!(file).unwrap();
        writeln!(file, "Co-authored-by: Claude Opus 4.6 <noreply@anthropic.com>").unwrap();

        let agent = &KNOWN_AGENTS[0]; // Claude Code <noreply@anthropic.com>
        append_trailers(&file.path().to_path_buf(), agent, false).unwrap();

        let content = fs::read_to_string(file.path()).unwrap();
        // Should NOT have added a second Co-authored-by for noreply@anthropic.com
        let co_author_count = content.matches("noreply@anthropic.com").count();
        assert_eq!(
            co_author_count, 1,
            "Should not add duplicate trailer for same email address, found {} occurrences",
            co_author_count
        );
    }

    #[test]
    fn test_dedup_agents_removes_duplicates() {
        let claude = &KNOWN_AGENTS[0]; // Claude Code
        let amp = &KNOWN_AGENTS[8]; // Amp
        let agents = vec![claude, amp, claude];
        let deduped = dedup_agents(agents);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].email, claude.email);
        assert_eq!(deduped[1].email, amp.email);
    }

    #[test]
    fn test_dedup_agents_empty() {
        let agents: Vec<&'static Agent> = vec![];
        let deduped = dedup_agents(agents);
        assert!(deduped.is_empty());
    }

    #[test]
    fn test_extract_email_addr() {
        assert_eq!(
            Agent::extract_email_addr("Claude Code <noreply@anthropic.com>"),
            "noreply@anthropic.com"
        );
        assert_eq!(
            Agent::extract_email_addr("Claude Opus 4.6 <noreply@anthropic.com>"),
            "noreply@anthropic.com"
        );
        assert_eq!(Agent::extract_email_addr("plain@email.com"), "plain@email.com");
        assert_eq!(Agent::extract_email_addr("Amp <amp@ampcode.com>"), "amp@ampcode.com");
    }

    #[test]
    fn test_find_agent_by_name() {
        assert!(Agent::find_by_name("claude").is_some());
        assert!(Agent::find_by_name("Claude").is_some());
        assert!(Agent::find_by_name("claude-code").is_some());
        assert!(Agent::find_by_name("cursor").is_some());
        assert!(Agent::find_by_name("cursor-agent").is_some());
        assert!(Agent::find_by_name("aider").is_some());
        assert!(Agent::find_by_name("windsurf").is_some());
        assert!(Agent::find_by_name("codex").is_some());
        assert!(Agent::find_by_name("copilot-agent").is_some());
        assert!(Agent::find_by_name("amazon-q").is_some());
        assert!(Agent::find_by_name("amp").is_some());
        assert!(Agent::find_by_name("/opt/homebrew/bin/amp").is_some());
        assert!(Agent::find_by_name("gemini").is_some());
        assert!(Agent::find_by_name("goose").is_some());
        assert!(Agent::find_by_name("unknown").is_none());
    }

    #[test]
    fn test_find_agent_by_env() {
        unsafe {
            std::env::set_var("CLINE_ACTIVE", "true");
        }
        let agent = Agent::find_by_env();
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
