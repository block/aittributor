This is a Rust project. Hermit manages toolchains, Just manages commands.

Run `just` to see all available commands. Use `just run` to build and run — NEVER manually compile and run. Use only libraries already in Cargo.toml.

README.md describes the design of this command-line tool. ALWAYS keep it up to date with features.

## Commits and PRs

Use Conventional Commits for commit messages and PR titles. CI lints PR titles.

## Gotchas

- Consolidate all agent data into `KNOWN_AGENTS` in `src/agent.rs`.
- This tool runs on every commit — keep it fast (1-second hard timeout).

## Maintenance

If this file misleads you or you hit a surprising behavior, update it.
