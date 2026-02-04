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
        process_names: &[],
        env_vars: &[("CLINE_ACTIVE", "true")],
        email: "Cline <noreply@cline.bot>",
    },
];

fn find_agent_by_name(name: &str) -> Option<&'static Agent> {
    let name_lower = name.to_lowercase();
    KNOWN_AGENTS
        .iter()
        .find(|agent| !agent.process_names.is_empty() && agent.process_names.iter().any(|&pn| name_lower.contains(pn)))
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

fn find_agent_for_process(process: &sysinfo::Process) -> Option<&'static Agent> {
    let name = process.name().to_string_lossy();
    if let Some(agent) = find_agent_by_name(&name) {
        return Some(agent);
    }

    if let Some(exe) = process.cmd().first() {
        let exe_str = exe.to_string_lossy();
        if let Some(agent) = find_agent_by_name(&exe_str) {
            return Some(agent);
        }
    }

    None
}

fn walk_ancestry(system: &System) -> Option<&'static Agent> {
    let mut current_pid = Pid::from_u32(std::process::id());

    while let Some(process) = system.process(current_pid) {
        if let Some(agent) = find_agent_for_process(process) {
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

fn check_process_tree(system: &System, root_pid: Pid, repo_path: &PathBuf) -> Option<&'static Agent> {
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

        if let Some(agent) = find_agent_for_process(process)
            && let Some(cwd) = process.cwd()
            && cwd.starts_with(repo_path)
        {
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

fn walk_ancestry_and_descendants(system: &System, repo_path: &PathBuf) -> Option<&'static Agent> {
    let mut current_pid = Pid::from_u32(std::process::id());
    let mut checked_ancestors = std::collections::HashSet::new();

    loop {
        let process = system.process(current_pid)?;

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

            if let Some(agent) = check_process_tree(system, sibling.pid(), repo_path) {
                return Some(agent);
            }
        }

        current_pid = parent_pid;
    }

    None
}

fn append_trailers(commit_msg_file: &PathBuf, agent: &Agent) -> std::io::Result<()> {
    let content = fs::read_to_string(commit_msg_file)?;

    if content.contains("Co-authored-by:") && content.contains(agent.email) {
        return Ok(());
    }

    let output = std::process::Command::new("git")
        .arg("interpret-trailers")
        .arg("--in-place")
        .arg("--trailer")
        .arg(format!("Co-authored-by: {}", agent.email))
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

fn detect_agent() -> Option<&'static Agent> {
    if let Some(agent) = find_agent_by_env() {
        return Some(agent);
    }

    let current_dir = std::env::current_dir().ok()?;
    let repo_path = find_git_root(&current_dir).unwrap_or(current_dir);
    let system = System::new_all();

    if let Some(agent) = walk_ancestry(&system) {
        return Some(agent);
    }

    walk_ancestry_and_descendants(&system, &repo_path)
}

fn main() {
    let cli = Cli::parse();

    let Some(commit_msg_file) = cli.commit_msg_file else {
        match detect_agent() {
            Some(agent) => println!("{}", agent.email),
            None => {
                eprintln!("No agent found");
                std::process::exit(1);
            }
        }
        return;
    };

    if let Some(agent) = detect_agent()
        && let Err(e) = append_trailers(&commit_msg_file, agent)
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
        append_trailers(&file.path().to_path_buf(), agent).unwrap();

        let content = fs::read_to_string(file.path()).unwrap();
        assert!(content.contains("Co-authored-by: Claude Code <noreply@anthropic.com>"));
        assert!(content.contains("Ai-assisted: true"));
    }

    #[test]
    fn test_append_trailers_idempotent() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "Initial commit").unwrap();

        let agent = &KNOWN_AGENTS[0];
        append_trailers(&file.path().to_path_buf(), agent).unwrap();
        let content1 = fs::read_to_string(file.path()).unwrap();

        append_trailers(&file.path().to_path_buf(), agent).unwrap();
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
}
