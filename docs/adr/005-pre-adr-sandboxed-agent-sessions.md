# ADR 005 (Pre-ADR): Sandboxed single-pane agent sessions

## Status

Pre-ADR — research and design questions captured. No implementation decision yet.

## Context

`dev` is increasingly used to run AI coding agents such as Claude Code and OpenCode inside persistent tmux sessions. Those agents currently run with the same filesystem access as the user. That is convenient, but it gives an agent write access to unrelated repositories and read access to sensitive user files.

The desired safety property is narrower:

- The agent can read and write the active project or worktree.
- The agent can use temporary storage such as `/tmp`.
- Any additional read access is explicit.
- The agent cannot modify unrelated repositories or user state by default.

This is especially important because `dev` sessions may become addressable by other agents through `dev send`, MCP tools, or future inter-session messaging.

## Current User Constraints

- `pop-mini` is the always-on Linux host.
- `mac-os` is a laptop client.
- `motorola-phone` is an Android/Termux client.
- Mobile screen size makes multi-pane layouts too busy.
- Initial design should assume single-pane sessions.
- OpenCode is invoked as `opencode`.
- Claude Code is invoked as `claude`.

## Provisional Direction

Use a single-pane configured session model first.

`dev <project>` should open the configured session for that project. For the preferred default, `dev dev` opens the `dev` tmux session with `opencode` launched in a single pane.

This keeps the mobile UX simple: one project, one tmux session, one pane.

### Domain split

The current `layout` field mixes two concepts: pane arrangement and the process to start. This should be split before sandboxing becomes first-class.

Proposed concepts:

| Concept | Values | Meaning |
|---|---|---|
| `kind` | `shell`, `agent` | What kind of session starts |
| `agent` | `opencode`, `claude` | Which agent command starts when `kind = "agent"` |
| `layout` | `single`, future `split` | Pane arrangement only |
| `sandbox` | config table | Execution wrapper and filesystem policy |

`layout = "claude"` should be deprecated because `claude` is an agent command, not a pane layout.

Possible config:

```toml
[defaults]
kind = "agent"
agent = "opencode"
layout = "single"

[defaults.sandbox]
enabled = true
backend = "nono"
tmp = "readwrite"

[project.dev]
path = "~/Projects/thompsonson/dev"
```

With that config:

| Invocation | tmux session |
|---|---|
| `dev dev` | `dev`, running `opencode` |
| `dev atomicguard` | `atomicguard`, running configured default agent |
| `dev <project> --kind shell` | `<project>`, running a shell |
| `dev <project> --agent claude` | `<project>`, running `claude` |

The sandbox wraps the agent command, not tmux itself, the tmux server, or the `dev` daemon.

Existing `dev claude <project>` can remain as a compatibility/convenience override, but it should not be the primary model for v1 sandboxing.

## Sandbox Backend Direction

`nono` is the preferred first backend to evaluate.

Reasons:

- It already targets AI-agent/dev-tool sandboxing.
- It supports Linux Landlock and macOS Seatbelt.
- The CLI provides policy profiles, diagnostics, dry-run, and explanation tooling.
- Using the CLI keeps `dev` smaller and avoids depending on a pre-1.0 Rust API.
- Applying a sandbox as a library call is usually process-local and irreversible, so `dev` would still need process boundaries.

Initial generated command shape for `dev dev` with default `opencode`:

```bash
nono run \
  --workdir /path/to/project \
  --allow /path/to/project \
  --allow /tmp \
  -- opencode
```

For Claude override:

```bash
nono run \
  --workdir /path/to/project \
  --allow /path/to/project \
  --allow /tmp \
  -- claude
```

Additional read-only paths can be added later with `--read` or equivalent profile configuration once the model is proven.

## Initial Filesystem Policy

### Writable

- Active project/worktree path.
- `/tmp`, unless testing shows a narrower temp directory works reliably.

### Readable

- Active project/worktree path.
- `/tmp`.
- No extra shared read paths in v1.

### Explicit Later Extension

Configured read-only paths for related repositories or documentation:

```toml
[project.dev.sandbox]
enabled = true
backend = "nono"
read = [
  "~/Projects/thompsonson/atomicguard",
  "~/Projects/thompsonson/chops"
]
```

## Agent Registry And Responsibilities

There is a related need to list active or configured agent sessions with a short responsibility description, so another agent can decide where to send a message.

Possible future shape:

```toml
[project.atomicguard.agents.guard-reviewer]
kind = "opencode"
responsibility = "Review guard implementation and suggest tests"
sandbox = true
```

Possible command:

```bash
dev agents
```

Output should include:

- Agent/session name.
- Project/worktree.
- Agent kind (`claude`, `opencode`).
- Responsibility.
- Status.
- Sandbox status.

This connects to ADR 003 inter-session messaging and ADR 004 MCP integration.

## Interaction With Existing Commands

`dev send` composes naturally because it injects text into a tmux pane. If the target pane is running a sandboxed agent, the agent remains sandboxed.

`dev run-in` does not automatically inherit the pane sandbox today. It uses `tmux run-shell -b`, which runs a background shell from the tmux server, not inside the pane's interactive process tree.

If `run-in` needs sandbox guarantees later, it must either:

- wrap each `run-in` command in the same sandbox policy, or
- add a separate mode that executes inside the pane shell.

## Open Questions

1. Should `layout = "claude"` remain as a deprecated compatibility shortcut, and if so for how long?

2. Should CLI overrides be flags (`--kind shell`, `--agent claude`, `--layout single`) rather than first-class subcommands such as `dev claude <project>`?

3. Should sandboxing be opt-in per project initially, or default-on for `kind = "agent"` when `nono` is installed?

4. Should `/tmp` be writable globally, or should `dev` allocate a per-session temp directory and expose only that?

5. Should sandbox policy live in the main `config.toml`, or in separate profile files?

6. Should agent responsibility metadata live in project config, session state, or a separate registry?

7. Should read-only related paths be global defaults, project-specific, worktree-specific, or all three?

8. What is the correct fallback when `nono` is unavailable?

   - Fail closed.
   - Warn and run unsandboxed.
   - Allow per-project override.

9. How should macOS local sandbox support be tested, given the main always-on host is Linux?

10. Should Termux ever support local sandboxed agent sessions, or should it remain a client-only platform?

11. Should `dev doctor` detect and validate `nono` only when sandbox config exists, or always report sandbox backend availability?

12. Should `dev sandbox show <project>` be implemented before sandboxed launch, so policy can be inspected before enforcement?

13. How should sandbox policy interact with future worktree URI resolution from ADR 002?

14. Should `dev send` surface responsibility metadata so agents can choose a target by role rather than raw session name?

## Non-Goals For V1

- Sandboxing the daemon or tmux server.
- Multi-pane agent layouts.
- Network restrictions.
- Rollback/snapshot integration.
- Direct Landlock/Seatbelt implementation inside `dev`.
- `nono` Rust library integration.
- Public sandbox profile registry.

## Related

- Issue #72 — sandboxed agent sessions research.
- ADR 002 — TOML config and session URI addressing.
- ADR 003 — inter-session agent messaging.
- ADR 004 — MCP integration.
- Issue #11 — microVM sandbox proposal.
