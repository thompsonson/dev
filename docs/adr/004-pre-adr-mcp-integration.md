# ADR 004 (Pre-ADR): Agent protocol integration — MCP as tool server

## Status

Pre-ADR — research complete, decision not yet implemented.

## Context

The `dev` daemon exposes a clean HTTP/1.1-over-Unix-socket API for session and pane management. As the primary consumers become AI agents (Claude Code, OpenCode, PiAgent, chat applications), there is a requirement that the daemon be accessible from these agents through their native tool-use interfaces — without requiring shell access or knowledge of the Unix socket protocol.

This requirement is non-negotiable: agents must be able to call `dev` operations as tools from whatever interface they are running in (chat UI, IDE extension, CLI, headless runner).

### Current state

Agents can already interact with `dev` via shell:
- `dev run-in <session> <command>` — run a command and capture output
- `dev send <session> <message>` — inject a message into a pane

This requires the agent to have shell access and knowledge of the `dev` CLI. It does not work from chat UIs or environments where only tool-use (function calling) is available.

## Protocol landscape

Four agent communication protocols are relevant:

| Protocol | Owner | Transport | Primary scope |
|---|---|---|---|
| **MCP** (Model Context Protocol) | Anthropic | JSON-RPC / stdio / HTTP+SSE | Agent ↔ tools and resources |
| **A2A** (Agent-to-Agent) | Google → Linux Foundation | HTTP, SSE, JSON-RPC | Agent ↔ agent task delegation |
| **ACP** (Agent Communication Protocol) | Linux Foundation / IBM | REST (HTTP) | Multi-agent orchestration |
| **ANP** (Agent Network Protocol) | Cisco / community | HTTP + JSON-LD, W3C DIDs | Decentralised agent marketplace |

> A2A and ACP merged under the Linux Foundation in late 2025. The merged spec preserves ACP's RESTful simplicity and adds A2A's Agent Cards and task lifecycle.

### Protocol overlap with `dev` daemon endpoints

| Daemon capability | MCP | A2A/ACP | ANP |
|---|---|---|---|
| Tool invocation (`/run`, `/keys`) | ✓ primary | ✓ via tasks | ✓ |
| Session/state management (`/sessions`) | ✓ resources | ✓ task lifecycle | — |
| Async / streaming | ✓ SSE | ✓ SSE | — |
| Agent discovery | — | ✓ Agent Cards | ✓ DID + JSON-LD |
| Decentralised identity | — | — | ✓ W3C DIDs |
| Local / no-network | ✓ stdio | ✗ (HTTP required) | ✗ |

## Decision

**Implement the `dev` daemon as an MCP tool server.**

### Why MCP

- **Broadest client support** — Claude Code, OpenCode, PiAgent, Cursor, chat applications, and any MCP-capable host can consume it without configuration beyond adding the server to their MCP server list.
- **Right abstraction** — MCP's tool model (name, description, input schema, output) maps directly onto daemon endpoints. No new concepts required.
- **Local transport** — MCP's stdio transport (recommended for local servers) runs the server as a subprocess, sandboxed by the OS. No network, no auth story beyond filesystem permissions.
- **Resource model** — MCP resources map to pane content and history reads; MCP tools map to mutating operations (run, send, start/stop sessions).

### Why not A2A/ACP

A2A/ACP addresses agent-to-agent task delegation, not agent-to-tool invocation. The daemon is a tool server, not a peer agent. A2A would require session-level HTTP endpoints, Agent Cards, and task lifecycle management — all overhead not needed to expose a local tool set.

### Why not ANP

ANP is designed for decentralised, cross-organisation agent marketplaces. The `dev` daemon is local-only. ANP's DID-based identity and JSON-LD discovery are out of scope.

### Proposed MCP tool mapping

| MCP Tool | Daemon call | Description |
|---|---|---|
| `list_sessions` | `GET /sessions` | List active sessions and known projects |
| `start_session` | `POST /sessions` | Start a session for a project |
| `stop_session` | `DELETE /sessions/:name` | Stop a named session |
| `list_panes` | `GET /sessions/:name/panes` | Get pane count for a session |
| `get_pane_content` | `GET /sessions/:name/panes/:id/content` | Read current pane terminal output |
| `run_command` | `POST /sessions/:name/panes/:id/run` | Run a command and capture stdout/exit code |
| `send_message` | `POST /sessions/:name/panes/:id/keys` | Inject a message into a pane |
| `get_history` | `GET /sessions/:name/panes/:id/history` | List recent command history for a pane |
| `get_history_record` | `GET /sessions/:name/panes/:id/history/:marker_id` | Get a specific history record |

### Transport

Use **stdio** for local agents (Claude Code, OpenCode). The MCP server is launched as a subprocess; OS filesystem permissions on the Unix socket are the auth boundary.

If network access is needed (remote agent, chat UI), use **HTTP+SSE** with TLS and token auth. Do not expose the Unix socket directly over the network.

## Security analysis

### General MCP vulnerabilities

| Vulnerability | Severity | Description |
|---|---|---|
| Prompt injection | High | Hidden instructions in tool outputs or data hijack the agent. Benchmark: 72.8% success rate (o1-mini); Claude 3.7-Sonnet refused <3%. |
| Tool poisoning | High | Malicious tool descriptions contain hidden instructions that execute at invocation. |
| Rug pull | Medium | Tool description silently updated with malicious behaviour post-approval. |
| Cross-server exfiltration | High | One malicious MCP server poisons agent to steal credentials from other connected servers. |
| Credential aggregation | High | MCP servers that aggregate enterprise credentials become a single point of failure. |
| CVE-2025-49596 | Critical (CVSS 9.4) | Unauthenticated MCP Inspector allowed arbitrary command execution. |

### Risk profile for `dev` as a local MCP server

Most high-severity general issues **do not apply** here:

| Risk | Applies? | Reason |
|---|---|---|
| Credential aggregation | No | Daemon controls local tmux only; no external service credentials |
| Cross-server exfiltration | Low | Daemon is a self-contained tool set, not aggregating third-party servers |
| Tool poisoning / rug pull | No | We control the MCP server code; no third-party server registry |
| CVE-2025-49596 | No | Not using MCP Inspector in production |
| Prompt injection via tool output | **Yes** | Pane content returned by `get_pane_content` or `get_history` could contain adversarial instructions if an agent in that pane was manipulated |
| Command injection via `run_command` | **Yes** | If an agent constructs shell commands from untrusted input |

### Mitigations

| Risk | Mitigation |
|---|---|
| Prompt injection in pane content | Wrap pane content in explicit delimiters in MCP tool output; instruct the consuming agent to treat content as data, not instructions |
| Command injection | `run_and_capture` already uses `sh_single_quote()` escaping; MCP tool schema should enforce input constraints (e.g. reject shell metacharacters) |
| Unauthorised daemon access | Unix socket permissions restrict to the owning user; no additional auth needed for stdio transport |
| Network exposure | Do not expose daemon over TCP without TLS + bearer token auth; keep Unix socket local |
| Prompt injection via `send_message` | Validate/sanitise message content at the MCP tool layer before calling `POST /sessions/:name/panes/:id/keys` |

### Overall security verdict

The enterprise-grade MCP concerns — credential aggregation, third-party rug pulls, cross-server exfiltration — do not apply to a local, self-hosted daemon with a fixed, self-controlled tool set. The residual risks (prompt injection via pane content, command injection) are addressable with input validation and explicit data framing in tool outputs.

## Integration roadmap

**Near term — MCP tool server (this ADR)**
Expose daemon as MCP tool server via stdio. Agents call `run_command`, `send_message`, etc. as native tools. Thin proxy layer: MCP server receives JSON-RPC tool calls and forwards them as HTTP-over-UDS to the daemon.

**Medium term — Structured messaging**
Replace raw keystroke injection (`send_message` / `POST /sessions/:name/panes/:id/keys`) with a structured `POST /sessions/:name/messages` endpoint: message identity, sender, timestamp, optional structured body alongside text. Aligns with ACP/A2A task model. Eliminates prompt injection surface from `send_message`.

**Long term — Cross-machine discoverability**
If sessions need to be discoverable across machines or organisations: wrap sessions in A2A Agent Cards, expose daemon over HTTPS, implement task lifecycle state. Enables cross-network agent collaboration using the merged ACP/A2A standard.

## Alternatives considered

**Direct CLI invocation** — agents with shell access call `dev run-in` / `dev send`. Already works. Does not serve chat UIs or tool-use-only environments. Not sufficient for the non-negotiable requirement.

**OpenAI function schema** — define daemon endpoints as OpenAI-format function schemas. More portable across vendors than MCP but requires per-client integration. MCP is already converging as the cross-vendor standard; function schemas are an implementation detail of MCP tool definitions.

**Expose daemon over local HTTP (TCP)** — replace or supplement Unix socket with `127.0.0.1:<port>`. Makes the API callable by any HTTP agent without a protocol wrapper. Does not address tool discovery or structured invocation for non-HTTP-native agents (chat UIs). Can be combined with MCP's HTTP+SSE transport.

**A2A/ACP** — REST-native, structured task lifecycle. Better fit if the daemon were a peer agent rather than a tool server. Overhead not justified for the local tool-server use case.

**ANP** — decentralised, DID-based. Out of scope for local tooling.

## Consequences

- A new MCP server component is required (Rust or Python; thin proxy to daemon Unix socket).
- The MCP server is the only new network/IPC surface; the daemon itself is unchanged.
- Tool schemas must enforce input constraints to mitigate command injection.
- Pane content returned by `get_pane_content` and `get_history` must be wrapped with explicit data-frame delimiters in tool output to reduce prompt injection risk.
- stdio transport is the default for local use; HTTP+SSE transport is optional for remote/chat access.
- The MCP server is a separate binary/service from the daemon; it can be started by the agent host on demand (stdio model) or run persistently alongside the daemon (HTTP+SSE model).
