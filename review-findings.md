# Code Review Findings — 2026-06-09

Identified during post-sprint tidy-up review. Ranked by severity.

---

## 1. Shell injection in `forward_remote` — `dev-cli/src/main.rs:554` — FIXED

`args.join(" ")` was passed as a single shell string to `ssh -t host "dev ..."`. SSH runs it via the remote shell, so a session name containing metacharacters could execute arbitrary commands on the remote host.

**Applied:** added `sh_quote` to `dev-cli/src/main.rs`; each arg is now POSIX single-quote escaped before joining.

---

## 2. `dev --local run-in` sends `"run-in"` as the session name — `dev-cli/src/main.rs:124` — FIXED

Used `args[1..]` instead of `raw.iter().skip(1).filter(...)`. `args` already has global flags stripped, so `args[1..]` correctly skips `"run-in"` itself.

---

## 3. ~~`POST /sessions` silently creates a local session for a remote-configured project~~ — CLOSED

### Resolution: not a bug

`@host` in the config is a client-side routing hint. It tells the CLI which daemon to talk to. By the time a request arrives at a daemon, the client has already decided that *this* daemon is the right place — the daemon executes locally and has no business checking `@host`.

`DevManager::start` correctly has no routing check. The routing is the CLI's responsibility; `cmd_start` already handles it before calling `mgr.start()` in-process.

`DevManager::open` returning `remote_host` is not a model for `start` — it does so because the CLI needs to SSH-forward the tmux *attach* step after session creation, which is a separate concern.

---

## 4. HTTP status ignored in `cmd_run_in` — `dev-cli/src/main.rs:620`

### Resolution

The hand-rolled `http_over_uds` client goes away when HTTPS transport lands — changing its return type now gets thrown away. The daemon also uses `500` for "session not found" where `404` would be correct; fixing status code handling before fixing the status codes themselves is premature.

The `{"error": "..."}` body shape is stable across transports. Applied a one-liner guard in `cmd_run_in` that surfaces the error field regardless of status code:

```rust
if let Some(err) = resp.get("error").and_then(|v| v.as_str()) {
    bail!("{err}");
}
```

Proper status codes and `http_over_uds` return type deferred to the HTTPS transport work.

---

## 5. ~~`@` in a path silently breaks host parsing~~ — CLOSED

### Resolution: migrate to TOML

The custom config format has inherent ambiguity around `@` in paths. Fixing the parser is low value — the config format will be replaced with TOML, which makes path and host separate keys and eliminates the ambiguity entirely. Deferred to that migration.

---

## 6. ~~`?lines=` not validated as an integer~~ — CLOSED

### Resolution: deferred to HTTPS transport

The endpoint is not called from the CLI — only reachable by direct UDS callers. Input validation and proper `400` responses will be handled when the transport moves to HTTPS and a proper framework takes over query parameter parsing.

---

## 7. `run-in` always runs locally even with `default_host` set — `dev-cli/src/main.rs:124`

### Resolution

`run-in` operates on the local daemon socket by design — UDS is inherently local. When HTTPS transport is added, remote dispatch becomes a real feature (POST to remote daemon directly). Patching routing logic now would be immediately replaced.

**Applied:** improved the connection error message to hint at the remote host scenario:
> `connect to dev daemon at <path> (is 'dev daemon' running? if sessions are on a remote host, ssh there and run directly)`

Remote dispatch deferred to the HTTPS transport work.

---

## 8. ~~`DaemonState` holds a separate `RealTmux` alongside `DevManager`~~ — CLOSED

### Resolution: deferred to HTTPS transport

`DaemonState` will be rewritten when HTTPS transport lands (new framework, new handler structure). Refactoring it now gets thrown away.

---

## 9. Silent fallback to empty `projects_dir` — `dev-lib/src/api.rs:62` — FIXED

Replaced `unwrap_or_default()` with `.context("HOME directory not set")?`. Returns a clear error when `HOME` is unset rather than silently discovering zero projects.

---

## 10. `parse_layout` implemented twice — `dev-cli/src/main.rs:186` and `dev-lib/src/daemon.rs:251` — FIXED

Moved `pub fn parse_layout(s: &str) -> Result<Layout>` into `dev-lib::config` and made `Layout::parse` public. Daemon uses `config::parse_layout`; CLI uses `Layout::parse` (lenient, defaults to Default for unknown values).

---

## Test inventory

### Existing tests (32 total, all in `dev-lib`, none in `dev-cli`)

| File | Count | Covers |
|------|-------|--------|
| `config.rs` | 10 | Config parsing, layout/host/path syntax |
| `api.rs` | 8 | Session ops, `resolve_target` |
| `resolve.rs` | 5 | Three-tier name matching |
| `discovery.rs` | 7 | Project discovery, depth, collisions |
| `daemon.rs` | 2 | Query param parsing, `parse_layout` |
| `tmux.rs` | 5 | `sh_quote` correctness + `MockTmux` infrastructure |

### Tests to write per finding

| # | Test to write | Where |
|---|---------------|-------|
| 1 | `forward_remote` shell-escapes each arg before joining | `dev-cli` |
| 2 | Arg parsing with `dev --local run-in mysession ls` uses correct tail | `dev-cli` |
| 3 | ~~closed — not a bug~~ | — |
| 4 | `http_over_uds` returns `(status, body)`; non-200 surfaces `"error"` field | `dev-cli` |
| 5 | ~~closed — config migrating to TOML~~ | — |
| 6 | ~~closed — deferred to HTTPS transport~~ | — |
| 7 | `run-in` with `default_host` set and no `--local` returns clear error | `dev-cli` |
| 8 | Daemon pane ops work with `MockTmux` (enabled by finding 8 refactor) | `dev-lib/daemon.rs` |
| 9 | `list_projects` returns error when HOME is unset | `dev-lib/api.rs` |
| 10 | Single `parse_layout` in `dev-lib::config`; delete duplicate | `dev-lib/config.rs` |

### TDD priority (write tests first to drive interface decisions)

1. **Finding 3** — Forces: what does `DevManager::start` return for a remote project? What HTTP status does the daemon emit?
2. **Finding 7** — Forces: what is the `run-in` contract when `default_host` is set? Error or forward?
3. **Finding 4** — Forces: what does `http_over_uds` return? Locks the type used by all `dev-cli` commands.
