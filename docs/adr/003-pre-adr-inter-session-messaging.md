# ADR 003 (Pre-ADR): Inter-session agent messaging

## Status

Pre-ADR — context and options captured. Decision implemented in [PR #70](https://github.com/thompsonson/dev/pull/70), pending formal ADR ratification.

## Context

When running multiple `dev` sessions each hosting an AI agent (Claude Code, OpenCode, PiAgent, etc.), there is no built-in mechanism for one agent to send a message to another. The use case that drove this: an agent in session `agent-session-a` needed to delegate a task to an agent in `agent-session-b`.

### What was tried (issue #69)

**Attempt 1 — `dev run-in`:**
```bash
dev run-in agent-session-b "echo hello"
```
Result: command executed and output returned to the caller. The target pane saw nothing. This is by design — `run-in` dispatches via `tmux run-shell -b`, a background subshell; the pane is completely untouched.

**Attempt 2 — explicit window.pane with 0-indexed address:**
```bash
dev run-in "agent-session-b:0.0" "echo hello"
```
Same result. Additionally, tmux windows and panes are 1-indexed; `0.0` is invalid. The correct address is `1.1`.

**Workaround that worked — direct `tmux send-keys`:**
```bash
tmux list-windows -t agent-session-b     # → "1: opencode* (1 panes)"
tmux list-panes -t agent-session-b:1 -F "#{pane_index}"  # → "1"
tmux send-keys -t agent-session-b:1 "review this issue" Enter
```
This injected the message into the target pane where the agent could see and respond to it.

### Existing daemon capability

The daemon already implements `POST /sessions/:name/panes/:id/keys`, which calls `tmux send-keys` via the `TmuxBackend` trait. No daemon changes were required. The gap was a CLI wrapper.

### The two distinct interaction models

| Model | Daemon endpoint | Pane sees it? | CLI command |
|---|---|---|---|
| Run and capture | `POST /sessions/:name/panes/:id/run` | No | `dev run-in` |
| Send and display | `POST /sessions/:name/panes/:id/keys` | Yes | *(none — gap)* |

For agent-to-agent messaging, the target agent must see the message. The `/run` model is unsuitable; the `/keys` model is correct.

## Options considered

**Option 1 — New `dev send` subcommand** *(chosen)*
```
dev send <session>[:<window>.<pane>] <message...>
```
Calls `POST /sessions/{name}/panes/{pane}/keys` with `{"keys": "<message>", "enter": true}`. Pane defaults to `1.1`. Mirrors `run-in` argument parsing.

*Pros:* Clean separation of concerns. `run-in` = capture tool, `send` = communication tool. Consistent with the daemon's existing endpoint split. No breaking changes.

**Option 2 — `--send-keys` flag on `dev run-in`**
```
dev run-in --send-keys agent-session-b "message"
```
Reuses existing command, flag switches endpoint.

*Cons:* Muddies the semantics of `run-in`. The command name implies execution and capture; `--send-keys` makes it do something fundamentally different.

**Option 3 — Invert `dev run-in` default, add `--capture`**
```
dev run-in agent-session-b "echo hello"   # now sends keys (visible)
dev run-in --capture agent-session-b "cmd" # old behaviour
```
*Cons:* Breaking change. Existing automation relying on captured output would break silently.

## Decision

Option 1: `dev send <session>[:<window>.<pane>] <message...>`.

### Pane addressing

Consistent with `dev run-in`: if no `:<window>.<pane>` suffix is provided, defaults to `1.1`.

In a `claude` layout session: `1.1` = agent pane (left), `1.2` = shell pane (right). The default targets the agent.

### Implementation

`cmd_send()` in `dev-cli/src/main.rs`:
1. Parse `<session>[:<window>.<pane>]` — split on `:`, default pane to `"1.1"`
2. Join remaining args as the message string
3. `POST /sessions/{session}/panes/{pane}/keys` with `{"keys": message, "enter": true}`
4. Return error if the daemon responds with an error field

## IPC context

`dev send` is a terminal-level keystroke injection mechanism, not a structured message queue. This is appropriate for the current use case (agent reads from its terminal pane) but has characteristics to note:

- **No delivery guarantee** — if the pane is in a state that swallows input (e.g. running a blocking command), the message is lost
- **No message identity** — no sender, timestamp, or structured payload
- **No receipt acknowledgement** — fire-and-forget

These are acceptable trade-offs for local developer tooling. See ADR 004 (pre-ADR) for the path to structured messaging aligned with agent protocols.

## Alternatives considered (IPC approach)

**File-based polling** — write message to a file; receiving agent polls. Does not use the daemon; adds filesystem coupling; rejected.

**Daemon message queue endpoint** — add `POST /sessions/:name/messages` storing structured records. More robust, aligns with ACP/A2A task model. Deferred: higher complexity, not needed for immediate use case.

**Named pipes (FIFOs)** — unidirectional, file-based, no structure. Inferior to the existing Unix socket daemon. Rejected.

## Consequences

- `dev send` is the canonical way for one agent to address another in a local multi-session environment.
- The daemon requires no changes — `POST /sessions/:name/panes/:id/keys` already existed.
- The `enter` field (defaults `true`) is documented: `dev send` always sends Enter after the message.
- `dev send` without Enter (e.g. for partial input) is not exposed at the CLI level but is available via direct daemon calls with `{"keys": "...", "enter": false}`.
- Prompt injection risk: a message constructed from untrusted content could contain instructions that manipulate the receiving agent. Mitigate by validating/sanitising message content at the sender before calling `dev send`. See ADR 004 for security context.
