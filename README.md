# AIttributor is a prepare-commit-msg hook that adds AI agent attribution to git commits

It does this by matching process names against known agents, and working directory against the current git repository.

It finds agents in four ways:

1. First it checks for agent-specific environment variables.
2. Then it walks its own process ancestry, under the assumption that the git commit was initiated by an agent.
3. If a known agent is not found, it walks up the process tree and checks all descendants of siblings at each level, looking for an agent working in the same repository.
4. If live process detection finds nothing, it checks agent-specific state files ("breadcrumbs") to determine if an agent was recently active in this repo (e.g. `~/.claude/projects/`, `~/.codex/sessions/`).

If an agent is found, it will append the following git trailers to the git commit:

```
Co-authored-by: <email>
Ai-assisted: true
```

Emails are the official "agent" emails, where available, such as `Claude Code <noreply@anthropic.com>`.

## Installation

```sh
curl -fsSL https://raw.githubusercontent.com/block/aittributor/main/install.sh | sh
```

Or to install a specific version:

```sh
curl -fsSL https://raw.githubusercontent.com/block/aittributor/main/install.sh | sh -s v0.0.1
```

To customize the installation directory:

```sh
curl -fsSL https://raw.githubusercontent.com/block/aittributor/main/install.sh | INSTALL_DIR=~/.local/bin sh
```

## Example

```
$ aittributor
Claude Code <noreply@anthropic.com>
```

## Usage with lefthook

```yaml
prepare-commit-msg:
  commands:
    aittributor:
      run: aittributor {1}
```

## Direct `.git/hooks` usage

```bash
ln -s /usr/local/bin/aittributor .git/hooks/prepare-commit-msg
```

## Breadcrumb fallback

If the AI agent exits before you commit, aittributor falls back to checking agent-specific state files to detect recently active agents. This only works when state files are available. No additional setup is required. 
