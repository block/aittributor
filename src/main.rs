mod agent;
mod breadcrumbs;
mod git;

use clap::Parser;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System, UpdateKind};

use agent::Agent;
use git::{append_trailers, find_git_root};

#[derive(Parser)]
#[command(name = "aittributor", version)]
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

/// Accumulates debug information while scanning a process tree.
///
/// All tracking is skipped unless `debug` is set, so the normal
/// (non-debug) code path stays allocation-free.
struct ScanReport {
    debug: bool,
    scanned: HashSet<Pid>,
    findings: Vec<String>,
    /// Emails already reported, to avoid duplicate "found" lines when the
    /// same agent shows up under multiple siblings.
    logged: HashSet<&'static str>,
}

impl ScanReport {
    fn new(debug: bool) -> Self {
        Self {
            debug,
            scanned: HashSet::new(),
            findings: Vec::new(),
            logged: HashSet::new(),
        }
    }

    fn mark_scanned(&mut self, pid: Pid) {
        if self.debug {
            self.scanned.insert(pid);
        }
    }

    fn record_match(&mut self, agent: &'static Agent, pid: Pid, name: &str) {
        // `insert` returns false if the email was already recorded.
        if self.debug && self.logged.insert(agent.email) {
            self.findings.push(format!(
                "  found {} (pid {}, process \"{}\", cwd matches repo)",
                agent.email, pid, name
            ));
        }
    }

    fn flush_into(self, log: &mut Vec<String>) {
        if !self.debug {
            return;
        }
        log.push(format!("  scanned {} processes", self.scanned.len()));
        if self.findings.is_empty() {
            log.push("  no match".to_string());
        } else {
            log.extend(self.findings);
        }
    }
}

fn walk_ancestry(system: &System, log: &mut Vec<String>, debug: bool) -> Vec<&'static Agent> {
    let mut current_pid = Pid::from_u32(std::process::id());
    let mut agents = Vec::new();
    let mut walked = 0usize;
    let mut findings = Vec::new();

    while let Some(process) = system.process(current_pid) {
        walked += 1;
        if let Some(agent) = Agent::find_for_process(process) {
            if debug {
                findings.push(format!(
                    "  found {} (pid {}, process \"{}\")",
                    agent.email,
                    current_pid,
                    process.name().to_string_lossy()
                ));
            }
            agents.push(agent);
        }

        match process.parent() {
            Some(parent_pid) if parent_pid != current_pid => {
                current_pid = parent_pid;
            }
            _ => break,
        }
    }

    if debug {
        log.push(format!("  walked {} processes", walked));
        if findings.is_empty() {
            log.push("  no match".to_string());
        } else {
            log.extend(findings);
        }
    }

    agents
}

fn check_process_tree(
    system: &System,
    root_pid: Pid,
    repo_path: &PathBuf,
    report: &mut ScanReport,
) -> Vec<&'static Agent> {
    let mut queue = VecDeque::new();
    let mut visited = HashSet::new();
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

        report.mark_scanned(pid);

        if let Some(agent) = Agent::find_for_process(process)
            && let Some(cwd) = process.cwd()
            && cwd.starts_with(repo_path)
        {
            report.record_match(agent, pid, &process.name().to_string_lossy());
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

fn walk_ancestry_and_descendants(
    system: &System,
    repo_path: &PathBuf,
    log: &mut Vec<String>,
    debug: bool,
) -> Vec<&'static Agent> {
    let mut current_pid = Pid::from_u32(std::process::id());
    let mut checked_ancestors = HashSet::new();
    let mut agents = Vec::new();
    let mut report = ScanReport::new(debug);

    while let Some(process) = system.process(current_pid) {
        if !checked_ancestors.insert(current_pid) {
            break;
        }

        let parent_pid = match process.parent() {
            Some(pid) if pid != current_pid => pid,
            _ => break,
        };

        for sibling in system.processes().values() {
            if sibling.parent() != Some(parent_pid) {
                continue;
            }

            agents.extend(check_process_tree(system, sibling.pid(), repo_path, &mut report));
        }

        current_pid = parent_pid;
    }

    report.flush_into(log);

    agents
}

fn detect_agents(log: &mut Vec<String>, debug: bool) -> Vec<&'static Agent> {
    let mut agents = Vec::new();

    if debug {
        log.push("strategy: environment variables".to_string());
    }
    match Agent::find_by_env() {
        Some(agent) => {
            if debug {
                let vars = agent
                    .env_vars
                    .iter()
                    .map(|(key, _)| *key)
                    .collect::<Vec<_>>()
                    .join(", ");
                log.push(format!("  found {} (env: {})", agent.email, vars));
            }
            agents.push(agent);
        }
        None => {
            if debug {
                log.push("  no match".to_string());
            }
        }
    }

    let current_dir = match std::env::current_dir() {
        Ok(d) => d,
        Err(_) => return agents,
    };
    let repo_path = find_git_root(&current_dir).unwrap_or(current_dir);
    if debug {
        log.push(format!("repository: {}", repo_path.display()));
        log.push("strategy: process ancestry".to_string());
    }

    let system = System::new_with_specifics(
        RefreshKind::nothing().with_processes(
            ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::Always)
                .with_cwd(UpdateKind::Always),
        ),
    );

    agents.extend(walk_ancestry(&system, log, debug));

    if debug {
        log.push("strategy: process tree (siblings and descendants)".to_string());
    }
    agents.extend(walk_ancestry_and_descendants(&system, &repo_path, log, debug));

    agents
}

fn breadcrumb_fallback(log: &mut Vec<String>, debug: bool) -> Vec<&'static Agent> {
    let current_dir = std::env::current_dir().unwrap_or_default();
    let repo_path = find_git_root(&current_dir).unwrap_or(current_dir);
    breadcrumbs::detect_agents_from_breadcrumbs(&repo_path, log, debug)
}

fn first_detected_agent(debug: bool) -> Option<&'static Agent> {
    // The breadcrumb scan runs on a separate thread, so each strategy buffers
    // its debug output into a `Vec<String>` instead of printing directly. We
    // print everything in a fixed order afterwards to keep the report readable.
    let (bc_tx, bc_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bc_log = Vec::new();
        let bc_agents = breadcrumb_fallback(&mut bc_log, debug);
        let _ = bc_tx.send((bc_agents, bc_log));
    });

    let mut log = Vec::new();
    let mut agents = detect_agents(&mut log, debug);

    if let Ok((bc_agents, bc_log)) = bc_rx.recv() {
        log.extend(bc_log);
        agents.extend(bc_agents);
    }

    let chosen = agents.into_iter().next();

    if debug {
        eprintln!("=== aittributor detection ===");
        for line in &log {
            eprintln!("{}", line);
        }
        match chosen {
            Some(agent) => eprintln!("\noutcome: attributing to {}", agent.email),
            None => eprintln!("\noutcome: no agent detected"),
        }
    }

    chosen
}

fn run(cli: Cli) {
    let agent = first_detected_agent(cli.debug);

    let Some(commit_msg_file) = cli.commit_msg_file else {
        match agent {
            Some(a) => println!("{}", a.email),
            None => {
                eprintln!("No agent found");
                std::process::exit(1);
            }
        }
        return;
    };

    if let Some(agent) = agent
        && let Err(e) = append_trailers(&commit_msg_file, agent, cli.debug)
    {
        eprintln!("aittributor: failed to append trailers: {}", e);
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

        let agent = Agent::find_by_name("claude").unwrap();
        append_trailers(&file.path().to_path_buf(), agent, false).unwrap();

        let content = fs::read_to_string(file.path()).unwrap();
        let co_author_count = content.matches("noreply@anthropic.com").count();
        assert_eq!(
            co_author_count, 1,
            "Should not add duplicate trailer for same email address, found {} occurrences",
            co_author_count
        );
    }

    #[test]
    fn test_append_trailers_skips_existing_email_different_case() {
        // The "Co-Authored-By" key can have varying capitalisation
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "Initial commit").unwrap();
        writeln!(file).unwrap();
        writeln!(
            file,
            "Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>"
        )
        .unwrap();

        let agent = Agent::find_by_name("claude").unwrap();
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

        let agent = Agent::find_by_name("claude").unwrap();
        append_trailers(&file.path().to_path_buf(), agent, false).unwrap();

        let content = fs::read_to_string(file.path()).unwrap();
        assert!(content.contains("Co-authored-by: Claude Code <noreply@anthropic.com>"));
    }

    #[test]
    fn test_append_trailers_idempotent() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "Initial commit").unwrap();

        let agent = Agent::find_by_name("claude").unwrap();
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

        let agent1 = Agent::find_by_name("claude").unwrap();
        let agent2 = Agent::find_by_name("amp").unwrap();

        append_trailers(&file.path().to_path_buf(), agent1, false).unwrap();
        append_trailers(&file.path().to_path_buf(), agent2, false).unwrap();

        let content = fs::read_to_string(file.path()).unwrap();
        assert!(content.contains("Co-authored-by: Claude Code <noreply@anthropic.com>"));
        assert!(content.contains("Co-authored-by: Amp <amp@ampcode.com>"));
    }
}
