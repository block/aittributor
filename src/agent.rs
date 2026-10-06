use std::ffi::{OsStr, OsString};
use std::path::Path;

pub struct Agent {
    pub process_names: &'static [&'static str],
    pub env_vars: &'static [(&'static str, &'static str)],
    pub email: &'static str,
    pub breadcrumb_dir: Option<&'static str>,
    pub breadcrumb_ext: Option<&'static str>,
    /// When true, process_names must match the basename exactly (not as a substring).
    /// Use for short names like "pi" that would otherwise false-positive on "pipefail" etc.
    pub exact_process_match: bool,
}

pub const KNOWN_AGENTS: &[Agent] = &[
    Agent {
        process_names: &["claude"],
        email: "Claude Code <noreply@anthropic.com>",
        breadcrumb_dir: Some(".claude/projects"),
        breadcrumb_ext: Some("jsonl"),
        ..Agent::default()
    },
    Agent {
        process_names: &["goose"],
        email: "Goose <opensource@block.xyz>",
        ..Agent::default()
    },
    Agent {
        process_names: &["cursor", "cursor-agent"],
        email: "Cursor <cursoragent@cursor.com>",
        ..Agent::default()
    },
    Agent {
        process_names: &["aider"],
        email: "Aider <noreply@aider.chat>",
        ..Agent::default()
    },
    Agent {
        process_names: &["windsurf"],
        email: "Windsurf <noreply@codeium.com>",
        ..Agent::default()
    },
    Agent {
        process_names: &["codex"],
        email: "Codex <noreply@openai.com>",
        breadcrumb_dir: Some(".codex/sessions"),
        breadcrumb_ext: Some("jsonl"),
        ..Agent::default()
    },
    Agent {
        process_names: &["copilot-agent"],
        email: "GitHub Copilot <noreply@github.com>",
        ..Agent::default()
    },
    // Copilot CLI is a separate terminal agent from the VS Code extension (copilot-agent above).
    // Must appear after copilot-agent since find_by_name uses contains() and "copilot" would
    // otherwise shadow the more specific "copilot-agent" match.
    Agent {
        process_names: &["copilot"],
        email: "Copilot <223556219+Copilot@users.noreply.github.com>",
        // Sessions stored as JSONL event logs in ~/.copilot/session-state/{session-id}/events.jsonl
        breadcrumb_dir: Some(".copilot/session-state"),
        breadcrumb_ext: Some("jsonl"),
        ..Agent::default()
    },
    Agent {
        process_names: &["amazon-q"],
        email: "Amazon Q Developer <noreply@amazon.com>",
        ..Agent::default()
    },
    Agent {
        process_names: &["amp"],
        email: "Amp <amp@ampcode.com>",
        ..Agent::default()
    },
    Agent {
        env_vars: &[("CLINE_ACTIVE", "true")],
        email: "Cline <noreply@cline.bot>",
        ..Agent::default()
    },
    Agent {
        process_names: &["gemini"],
        email: "Gemini CLI Agent <gemini-cli-agent@google.com>",
        ..Agent::default()
    },
    Agent {
        process_names: &["pi"],
        email: "Pi <noreply@pi.dev>",
        breadcrumb_dir: Some(".pi/agent/sessions"),
        breadcrumb_ext: Some("jsonl"),
        exact_process_match: true,
        ..Agent::default()
    },
    // TODO: OpenCode sessions are stored in SQLite (~/.local/share/opencode/opencode.db),
    // not flat files. Breadcrumb scanning would require a new SQLite-based strategy.
    Agent {
        process_names: &["opencode"],
        email: "opencode <noreply@opencode.ai>",
        ..Agent::default()
    },
];

impl Agent {
    const fn default() -> Self {
        Agent {
            process_names: &[],
            env_vars: &[],
            email: "",
            breadcrumb_dir: None,
            breadcrumb_ext: None,
            exact_process_match: false,
        }
    }

    /// Extract the bare email address from a "Name <addr>" string.
    /// e.g. "Claude Code <noreply@anthropic.com>" → "noreply@anthropic.com"
    pub fn extract_email_addr(email: &str) -> &str {
        email
            .split('<')
            .nth(1)
            .and_then(|s| s.split('>').next())
            .unwrap_or(email)
    }

    pub fn find_by_name(name: &str) -> Option<&'static Agent> {
        let path = Path::new(name);
        let basename = path.file_name().and_then(|n| n.to_str()).unwrap_or(name);
        let basename_lower = basename.to_lowercase();

        KNOWN_AGENTS.iter().find(|agent| {
            !agent.process_names.is_empty()
                && agent.process_names.iter().any(|&pn| {
                    if agent.exact_process_match {
                        basename_lower == pn
                    } else {
                        basename_lower.contains(pn)
                    }
                })
        })
    }

    pub fn find_by_env() -> Option<&'static Agent> {
        KNOWN_AGENTS.iter().find(|agent| {
            !agent.env_vars.is_empty()
                && agent
                    .env_vars
                    .iter()
                    .all(|(key, value)| std::env::var(key).ok().as_deref() == Some(*value))
        })
    }

    pub fn find_for_process(process: &sysinfo::Process) -> Option<&'static Agent> {
        Self::find_for_command(process.name(), process.cmd())
    }

    fn find_for_command(name: &OsStr, command: &[OsString]) -> Option<&'static Agent> {
        let name = name.to_string_lossy();
        if let Some(agent) = Self::find_by_name(&name) {
            return Some(agent);
        }

        // Check basename(argv[0])
        if let Some(arg0) = command.first() {
            let arg0_str = arg0.to_string_lossy();
            if let Some(agent) = Self::find_by_name(&arg0_str) {
                return Some(agent);
            }

            // Shell -c arguments contain code, not the name of an agent script.
            let executable = Path::new(arg0).file_name().and_then(OsStr::to_str);
            if matches!(
                executable,
                Some("sh" | "bash" | "dash" | "ash" | "zsh" | "ksh" | "fish" | "csh" | "tcsh")
            ) && command
                .iter()
                .skip(1)
                .map(|arg| arg.to_string_lossy())
                .take_while(|arg| arg.starts_with('-') && arg != "--")
                .any(|arg| {
                    arg == "--command"
                        || arg
                            .strip_prefix('-')
                            .is_some_and(|flags| !flags.starts_with('-') && flags.contains('c'))
                })
            {
                return None;
            }
        }

        // Check first basename(argv[1:]) that doesn't start with '-'
        if let Some(arg) = command.iter().skip(1).find(|arg| {
            let arg_str = arg.to_string_lossy();
            !arg_str.starts_with('-')
        }) {
            let arg_str = arg.to_string_lossy();
            if let Some(agent) = Self::find_by_name(&arg_str) {
                return Some(agent);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::process::{Command, Stdio};
    use sysinfo::{ProcessRefreshKind, RefreshKind, System, UpdateKind};

    #[test]
    fn shell_command_text_does_not_identify_an_agent() {
        for (shell, flag) in [("sh", "-c"), ("bash", "-c"), ("bash", "-lc")] {
            let mut child = Command::new(shell)
                .args([
                    flag,
                    "printf ready; read -r ignored # git commit -m 'chore: deploy goose-example'",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            // Wait until the shell has started and is blocked on stdin.
            child.stdout.as_mut().unwrap().read_exact(&mut [0; 5]).unwrap();
            let system = System::new_with_specifics(
                RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always)),
            );
            let process = system.process(sysinfo::Pid::from_u32(child.id())).unwrap();
            let detected = Agent::find_for_process(process).map(|agent| agent.email);
            child.kill().unwrap();
            child.wait().unwrap();

            assert_eq!(detected, None, "{shell} {flag} was mistaken for an agent");
        }
    }

    #[test]
    fn interpreter_scripts_still_identify_agents() {
        for (name, args, email) in [
            (
                "node",
                vec!["/usr/bin/node", "/opt/codex-acp.js"],
                "Codex <noreply@openai.com>",
            ),
            (
                "python3",
                vec!["python3", "-u", "/opt/goose.py"],
                "Goose <opensource@block.xyz>",
            ),
            ("bash", vec!["bash", "/opt/codex.sh"], "Codex <noreply@openai.com>"),
            (
                "codex",
                vec!["codex", "exec", "goose-example"],
                "Codex <noreply@openai.com>",
            ),
        ] {
            let command: Vec<OsString> = args.into_iter().map(OsString::from).collect();
            assert_eq!(
                Agent::find_for_command(OsStr::new(name), &command).map(|agent| agent.email),
                Some(email),
                "failed to identify {command:?}",
            );
        }
    }

    #[test]
    fn shell_inline_command_flags_do_not_identify_agents() {
        for (name, flag) in [("bash", "-lc"), ("zsh", "-ic"), ("fish", "--command")] {
            let command = [name, flag, "git commit -m 'deploy goose-example'"].map(OsString::from);
            assert!(Agent::find_for_command(OsStr::new(name), &command).is_none());
        }
    }
}
