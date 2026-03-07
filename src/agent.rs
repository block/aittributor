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

    pub fn find_for_process(process: &sysinfo::Process, debug: bool) -> Option<&'static Agent> {
        let name = process.name().to_string_lossy();
        if debug {
            eprintln!("      Checking process name: {}", name);
        }
        if let Some(agent) = Self::find_by_name(&name) {
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
            if let Some(agent) = Self::find_by_name(&arg0_str) {
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
            if let Some(agent) = Self::find_by_name(&arg_str) {
                if debug {
                    eprintln!("        ✓ Matched agent: {}", agent.email);
                }
                return Some(agent);
            }
        }

        None
    }
}
