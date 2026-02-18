use std::path::Path;

pub struct Agent {
    pub process_names: &'static [&'static str],
    pub env_vars: &'static [(&'static str, &'static str)],
    pub email: &'static str,
    pub breadcrumb_dir: Option<&'static str>,
    pub breadcrumb_ext: Option<&'static str>,
}

pub const KNOWN_AGENTS: &[Agent] = &[
    Agent {
        process_names: &["claude"],
        env_vars: &[],
        email: "Claude Code <noreply@anthropic.com>",
        breadcrumb_dir: Some(".claude/projects"),
        breadcrumb_ext: Some("jsonl"),
    },
    Agent {
        process_names: &["goose"],
        env_vars: &[],
        email: "Goose <opensource@block.xyz>",
        breadcrumb_dir: None,
        breadcrumb_ext: None,
    },
    Agent {
        process_names: &["cursor", "cursor-agent"],
        env_vars: &[],
        email: "Cursor <noreply@cursor.com>",
        breadcrumb_dir: None,
        breadcrumb_ext: None,
    },
    Agent {
        process_names: &["aider"],
        env_vars: &[],
        email: "Aider <noreply@aider.chat>",
        breadcrumb_dir: None,
        breadcrumb_ext: None,
    },
    Agent {
        process_names: &["windsurf"],
        env_vars: &[],
        email: "Windsurf <noreply@codeium.com>",
        breadcrumb_dir: None,
        breadcrumb_ext: None,
    },
    Agent {
        process_names: &["codex"],
        env_vars: &[],
        email: "Codex <noreply@openai.com>",
        breadcrumb_dir: Some(".codex/sessions"),
        breadcrumb_ext: Some("jsonl"),
    },
    Agent {
        process_names: &["copilot-agent"],
        env_vars: &[],
        email: "GitHub Copilot <noreply@github.com>",
        breadcrumb_dir: None,
        breadcrumb_ext: None,
    },
    Agent {
        process_names: &["amazon-q", "q"],
        env_vars: &[],
        email: "Amazon Q Developer <noreply@amazon.com>",
        breadcrumb_dir: None,
        breadcrumb_ext: None,
    },
    Agent {
        process_names: &["amp"],
        env_vars: &[],
        email: "Amp <amp@ampcode.com>",
        breadcrumb_dir: None,
        breadcrumb_ext: None,
    },
    Agent {
        process_names: &[],
        env_vars: &[("CLINE_ACTIVE", "true")],
        email: "Cline <noreply@cline.bot>",
        breadcrumb_dir: None,
        breadcrumb_ext: None,
    },
    Agent {
        process_names: &["gemini"],
        env_vars: &[],
        email: "Gemini CLI Agent <gemini-cli-agent@google.com>",
        breadcrumb_dir: None,
        breadcrumb_ext: None,
    },
];

impl Agent {
    pub fn find_by_name(name: &str) -> Option<&'static Agent> {
        let path = Path::new(name);
        let basename = path.file_name().and_then(|n| n.to_str()).unwrap_or(name);
        let basename_lower = basename.to_lowercase();

        KNOWN_AGENTS.iter().find(|agent| {
            !agent.process_names.is_empty() && agent.process_names.iter().any(|&pn| basename_lower.contains(pn))
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
