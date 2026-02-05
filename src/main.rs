use clap::Parser;
use std::fs;
use std::path::{Path, PathBuf};
use sysinfo::{Pid, System};

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

struct Agent {
    process_names: &'static [&'static str],
    env_vars: &'static [(&'static str, &'static str)],
    email: &'static str,
}

const KNOWN_AGENTS: &[Agent] = &[
    Agent {
        process_names: &["claude"],
        env_vars: &[],
        email: "Claude Code <noreply@anthropic.com>",
    },
    Agent {
        process_names: &["goose"],
        env_vars: &[],
        email: "Goose <noreply@block.xyz>",
    },
    Agent {
        process_names: &["cursor", "cursor-agent"],
        env_vars: &[],
        email: "Cursor <noreply@cursor.com>",
    },
    Agent {
        process_names: &["aider"],
        env_vars: &[],
        email: "Aider <noreply@aider.chat>",
    },
    Agent {
        process_names: &["windsurf"],
        env_vars: &[],
        email: "Windsurf <noreply@codeium.com>",
    },
    Agent {
        process_names: &["codex"],
        env_vars: &[],
        email: "Codex <noreply@openai.com>",
    },
    Agent {
        process_names: &["copilot-agent"],
        env_vars: &[],
        email: "GitHub Copilot <noreply@github.com>",
    },
    Agent {
        process_names: &["amazon-q", "q"],
        env_vars: &[],
        email: "Amazon Q Developer <noreply@amazon.com>",
    },
    Agent {
        process_names: &["amp"],
        env_vars: &[],
        email: "Amp <amp@ampcode.com>",
    },
    Agent {
        process_names: &[],
        env_vars: &[("CLINE_ACTIVE", "true")],
        email: "Cline <noreply@cline.bot>",
    },
    Agent {
        process_names: &["gemini"],
        env_vars: &[],
        email: "Gemini <218195315+gemini-cli@users.noreply.github.com>",
    },
];

fn find_agent_by_name(name: &str) -> Option<&'static Agent> {
    let path = Path::new(name);
    let basename = path.file_name().and_then(|n| n.to_str()).unwrap_or(name);
    let basename_lower = basename.to_lowercase();

    KNOWN_AGENTS.iter().find(|agent| {
        !agent.process_names.is_empty() && agent.process_names.iter().any(|&pn| basename_lower.contains(pn))
    })
}

fn find_agent_by_env() -> Option<&'static Agent> {
    KNOWN_AGENTS.iter().find(|agent| {
        !agent.env_vars.is_empty()
            && agent
                .env_vars
                .iter()
                .all(|(key, value)| std::env::var(key).ok().as_deref() == Some(*value))
    })
}

fn find_agent_for_process(process: &sysinfo::Process, debug: bool) -> Option<&'static Agent> {
    let name = process.name().to_string_lossy();
    if debug {
        eprintln!("      Checking process name: {}", name);
    }
    if let Some(agent) = find_agent_by_name(&name) {
        if debug {
            eprintln!("        ✓ Matched agent: {}", agent.email);
        }
        return Some(agent);
    }

    // Check basename(argv[0])
    if let Some(arg0) = process.cmd().first() {
        let arg0_str = arg0.to_string_lossy();
        if debug {
            eprintln!("      Checking basename(argv[0]): {}", arg0_str);
        }
        if let Some(agent) = find_agent_by_name(&arg0_str) {
            if debug {
                eprintln!("        ✓ Matched agent: {}", agent.email);
            }
            return Some(agent);
        }
    }

    // Check first basename(argv[1:]) that doesn't start with '-'
    if let Some(arg) = process.cmd().iter().skip(1).find(|arg| {
        let arg_str = arg.to_string_lossy();
        !arg_str.starts_with('-')
    }) {
        let arg_str = arg.to_string_lossy();
        if debug {
            eprintln!("      Checking first non-flag arg from argv[1:]: {}", arg_str);
        }
        if let Some(agent) = find_agent_by_name(&arg_str) {
            if debug {
                eprintln!("        ✓ Matched agent: {}", agent.email);
            }
            return Some(agent);
        }
    }

    None
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

fn append_trailers(commit_msg_file: &PathBuf, agent: &Agent, debug: bool) -> std::io::Result<()> {
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

fn find_git_root(start_path: &Path) -> Option<PathBuf> {
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

fn main() {
    let cli = Cli::parse();

    let Some(commit_msg_file) = cli.commit_msg_file else {
        match detect_agent(cli.debug) {
            Some(agent) => println!("{}", agent.email),
            None => {
                eprintln!("No agent found");
                std::process::exit(1);
            }
        }
        return;
    };

    if let Some(agent) = detect_agent(cli.debug)
        && let Err(e) = append_trailers(&commit_msg_file, agent, cli.debug)
    {
        eprintln!("aittributor: failed to append trailers: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
