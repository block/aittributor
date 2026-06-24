# AIttributor is a prepare-commit-msg hook that adds AI agent attribution to git commits

It does this by matching process names against known agents, and working directory against the current git repository.

It finds agents in four ways:

1. It checks for agent-specific environment variables.
2. It walks its own process ancestry, under the assumption that the git commit was initiated by an agent.
3. It walks up the process tree and checks all descendants of siblings at each level, looking for agents working in the same repository.
4. It checks agent-specific state files ("breadcrumbs") to determine if an agent was recently active in this repo (e.g. `~/.claude/projects/`, `~/.codex/sessions/`, `~/.pi/agent/sessions/`). When both the session and the commit record a branch, they must match; this avoids misattributing a commit to an agent that was last used on a different branch.

At most one agent is attributed per commit: the first one found, in the order above. If an agent is found, it will append the following git trailer to the git commit:

```
Co-authored-by: <email>
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

## Known limitations

**Process detection is not always possible.** Agents may exit before the commit runs, or use process names that don't match (e.g. Electron-based desktop apps). When process scanning fails, aittributor falls back to agent session history, checking state files for recent activity in the same repo. This fallback only works for agents that write scannable state files (currently Claude, Codex, Copilot CLI, and Pi). Some agents like OpenCode store sessions in SQLite, which is not yet supported by the breadcrumb scanner. To reduce false positives, the breadcrumb scanner matches the session's git branch against the commit's branch when both are recorded; note that Codex captures the branch only at session start, so it can still misattribute if you remain in one Codex session across a branch switch. Even so, the fallback cannot fully distinguish between an agent that wrote the code being committed and one that was only used for research, so the result is a bias toward over-attribution. This is a deliberate tradeoff, as undercounting real AI usage is harder to correct after the fact than occasional overcounting.

**Agent-initiated commits are the most reliable.** Attribution is most accurate when the agent itself runs `git commit`. Manual commits while an agent session is open (or recently closed) are the main source of attribution that may not reflect actual code contribution.

**Only one agent is attributed per commit.** When several agents are detected (e.g. a different agent is running elsewhere in the process tree, or a breadcrumb is found for another agent), only the first match is recorded. Aittributor still skips adding its trailer if a `Co-authored-by` for that email is already present in the commit message.
