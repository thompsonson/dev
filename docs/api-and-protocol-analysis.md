# `dev` API Reference and Agent Protocol Analysis

## 1. CLI Command Reference

### Global flags

| Flag | Applies to |
|---|---|
| `--local` | `dev`, `list`, `stop`, `kill-all` — forces local execution even when `default_host` is set |
| `--force` / `-y` | `kill-all` — skips confirmation prompt |

### Commands

| Command | Description |
|---|---|
| `dev` | Interactive session picker (fzf or numbered fallback) |
| `dev <project>` | Create or attach to session for project |
| `dev claude <project>` | Force claude layout (vertical split: agent left, shell right) |
| `dev start <project> [layout]` | Create session without attaching |
| `dev stop <session>` | Kill a session |
| `dev kill <name>` | Kill a session by name |
| `dev kill-all` | Kill all sessions (with confirmation) |
| `dev list` | JSON output of all sessions and known projects |
| `dev detach` | Detach from current tmux session |
| `dev layout [name]` | Print or switch the default layout |
| `dev daemon` | Run the Unix socket API server |
| `dev run-in <session>[:<w>.<p>] <cmd> [--timeout N] [--json]` | Run command in pane, capture stdout/exit code (pane sees nothing) |
| `dev send <session>[:<w>.<p>] <message...>` | Send message as keystrokes to pane (visible to agent; default pane: `1.1`) |
| `dev doctor [--config F]` | Check environment and config |
| `dev update [--check]` | Check for and apply updates from GitHub releases |
| `dev version` / `--version` / `-V` | Print version |
| `dev help` / `--help` / `-h` | Show help |

### Pane addressing

`:id` is a tmux pane target in `<window>.<pane>` form (e.g. `1.1`, `1.2`). Windows and panes are 1-indexed. When omitted, both `run-in` and `send` default to `1.1`.

In the `claude` layout: `1.1` = agent pane (left), `1.2` = shell pane (right).

---

## 2. Daemon API Endpoint Reference

The daemon listens on a Unix domain socket (`$XDG_RUNTIME_DIR/dev.sock` or `~/.local/run/dev.sock`) using a minimal HTTP/1.1-over-UDS protocol. All request and response bodies are JSON.

| Method | Path | Request body | Response |
|---|---|---|---|
| `GET` | `/sessions` | — | `{sessions: [...], projects: [...]}` |
| `POST` | `/sessions` | `{project, layout?}` | `201 {session}` |
| `DELETE` | `/sessions/:name` | — | `{stopped}` |
| `GET` | `/sessions/:name/panes` | — | `{session, pane_count}` |
| `GET` | `/sessions/:name/panes/:id/content?lines=N` | — | `{content}` |
| `POST` | `/sessions/:name/panes/:id/keys` | `{keys, enter?}` | `202 {sent}` |
| `POST` | `/sessions/:name/panes/:id/run` | `{command, timeout_ms?}` | `{stdout, exit_code, duration_ms, marker_id}` |
| `GET` | `/sessions/:name/panes/:id/history` | — | `[HistoryRecord, ...]` |
| `GET` | `/sessions/:name/panes/:id/history/:marker_id` | — | `HistoryRecord` |

### Key distinctions

- `POST /sessions/:name/panes/:id/keys` — injects keystrokes via `tmux send-keys`; the pane and any agent running in it sees the input. `enter` defaults to `true`.
- `POST /sessions/:name/panes/:id/run` — dispatches via `tmux run-shell -b`; the pane sees nothing. Stdout/stderr are captured to staging files and returned to the caller. Result is stored in the in-memory history ring buffer (cap: 50 per pane).

### HistoryRecord shape

```json
{
  "marker_id": "ad7a0ca189e3",
  "command": "cargo test",
  "stdout": "...",
  "exit_code": 0,
  "duration_ms": 1240,
  "at_unix_secs": 1749999999
}
```

---

## 3. Agent Protocol Comparison: MCP, A2A, ACP, ANP

### Protocol summaries

| Protocol | Owner | Transport | Model | Scope |
|---|---|---|---|---|
| **MCP** (Model Context Protocol) | Anthropic | JSON-RPC over stdio or HTTP+SSE | Single agent ↔ tools/resources | Agent-to-tool context provision |
| **A2A** (Agent-to-Agent) | Google (2025) | HTTP, SSE, JSON-RPC | Peer-to-peer task delegation via Agent Cards | Agent-to-agent collaboration |
| **ACP** (Agent Communication Protocol) | Linux Foundation / IBM BeeAI | REST (HTTP) | Brokered/centralized orchestration | Multi-agent, multi-framework orchestration |
| **ANP** (Agent Network Protocol) | Cisco / community | HTTP + JSON-LD, W3C DIDs | Fully decentralized discovery | Open-internet agent marketplace |

> Note: ACP and A2A merged under the Linux Foundation in late 2025. The merged spec preserves ACP's RESTful simplicity while incorporating A2A's Agent Cards and task lifecycle.

### Functional overlap matrix

| Capability | MCP | A2A | ACP | ANP |
|---|---|---|---|---|
| Tool/capability invocation | ✓ (primary) | ✓ (via tasks) | ✓ | ✓ |
| Agent discovery | — | ✓ Agent Cards at `/.well-known/agent.json` | ✓ central registry | ✓ DID + JSON-LD |
| Session / conversation state | ✓ stateful sessions | ✓ task lifecycle | ✓ | — |
| Async / streaming | ✓ SSE | ✓ SSE | ✓ | — |
| Identity / auth | — | ✓ per Agent Card | ✓ | ✓ W3C DIDs + encryption |
| Multi-agent orchestration | — | ✓ | ✓ (primary) | ✓ |
| Resource/data access | ✓ (primary) | — | — | — |
| Decentralized / no broker | — | — | — | ✓ (primary) |

---

## 4. Integration Points with `dev`

### Where `dev` fits in the protocol stack

`dev` is a local session management layer. It is not itself an agent protocol, but it provides primitives that map cleanly onto each protocol's concepts.

### MCP

The `dev` daemon is a natural MCP **tool server** target. Each daemon endpoint maps to an MCP tool:

| MCP Tool name (proposed) | Daemon call |
|---|---|
| `list_sessions` | `GET /sessions` |
| `start_session` | `POST /sessions` |
| `stop_session` | `DELETE /sessions/:name` |
| `run_command` | `POST /sessions/:name/panes/:id/run` |
| `send_message` | `POST /sessions/:name/panes/:id/keys` |
| `get_pane_content` | `GET /sessions/:name/panes/:id/content` |
| `get_history` | `GET /sessions/:name/panes/:id/history` |

An MCP server wrapping the `dev` daemon would let any MCP-capable agent (Claude Code, OpenCode, etc.) drive tmux sessions without direct shell access.

### A2A / ACP

Each `dev` session running an agent could publish an **Agent Card** at `/.well-known/agent.json` (or equivalent), advertising its capabilities. Inter-session messaging via `dev send` is the local transport analogue of A2A task delegation:

- A2A: `POST /tasks` with structured payload to a remote agent's HTTP endpoint
- `dev`: `POST /sessions/:name/panes/:id/keys` with message text to a local agent's pane

The `dev` daemon's history API (`GET /sessions/:name/panes/:id/history`) provides an audit trail analogous to A2A's task lifecycle state (`created → working → completed`).

### ANP

ANP's decentralized identity (W3C DIDs) and JSON-LD capability discovery are out of scope for a local-only tool. However, if `dev` sessions were extended with a network-accessible API (e.g. SSH-forwarded or HTTPS-wrapped), each session could participate in an ANP agent network with its `dev` session name as the agent identity.

### Summary: recommended integration path

For the current use case (local multi-agent development with Claude Code / OpenCode):

1. **Near term** — expose the `dev` daemon as an **MCP tool server**. This lets agents already running in sessions invoke `dev` operations (start session, run command, send message) through their native tool-use interface without needing shell access or knowledge of the Unix socket protocol.

2. **Medium term** — implement structured message payloads on `POST /sessions/:name/panes/:id/keys` (or a new `/messages` endpoint) to align with ACP/A2A's task model: message identity, sender, timestamp, optional structured body alongside the human-readable text.

3. **Long term** — if sessions need to be discoverable across machines or organisations, wrap sessions in A2A Agent Cards and expose the daemon over HTTPS, enabling cross-network agent collaboration using the merged ACP/A2A standard.

---

## Sources

- [A Survey of Agent Interoperability Protocols: MCP, ACP, A2A, ANP](https://arxiv.org/html/2505.02279v1)
- [AI Agent Protocols Explained: MCP vs A2A vs ACP vs ANP](https://data443.com/blog/ai-agent-protocols-explained-mcp-vs-a2a-vs-acp-vs-anp/)
- [Top AI Agent Protocols in 2026 — MCP, A2A, ACP & More](https://getstream.io/blog/ai-agent-protocols/)
- [Agent Interoperability Protocols 2026: MCP, A2A, ACP and the Path to Convergence](https://zylos.ai/research/2026-03-26-agent-interoperability-protocols-mcp-a2a-acp-convergence/)
- [The AI Agent Protocol Stack: MCP, A2A, ACP, ANP and How They Fit Together](https://chatforest.com/guides/ai-agent-protocol-stack-2026/)
- [Model Context Protocol — practical technical overview](https://codilime.com/blog/model-context-protocol-explained/)
