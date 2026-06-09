# Code Review Findings — 2026-06-09

Identified during post-sprint tidy-up review. Ranked by severity.

---

## 1. Shell injection in `forward_remote` — `dev-cli/src/main.rs:554`

`args.join(" ")` is passed as a single shell string to `ssh -t host "dev ..."`. SSH runs it via the remote shell, so a session name containing metacharacters executes arbitrary commands on the remote host.

**Concrete:** `dev stop "foo; touch /tmp/pwned"` → remote shell runs both commands.

**Fix:** Shell-escape each arg before joining:
```rust
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
let remote_cmd = format!("dev {}", args.iter().map(sh_quote).collect::<Vec<_>>().join(" "));
```

---

## 2. `dev --local run-in` sends `"run-in"` as the session name — `dev-cli/src/main.rs:124`

When a global flag precedes `run-in` (e.g. `dev --local run-in mysession ls`), `raw.iter().skip(1)` skips the flag not `"run-in"`, so `tail = ["run-in", "mysession", "ls"]` and `cmd_run_in` treats `"run-in"` as the session name.

**Fix:** Use the already-filtered `args[1..]` instead of going back to `raw`:
```rust
Some("run-in") => {
    let tail: Vec<String> = args[1..].iter().map(|s| s.to_string()).collect();
    cmd_run_in(&tail)
}
```

---

## 3. ~~`POST /sessions` silently creates a local session for a remote-configured project~~ — CLOSED

### Resolution: not a bug

`@host` in the config is a client-side routing hint. It tells the CLI which daemon to talk to. By the time a request arrives at a daemon, the client has already decided that *this* daemon is the right place — the daemon executes locally and has no business checking `@host`.

`DevManager::start` correctly has no routing check. The routing is the CLI's responsibility; `cmd_start` already handles it before calling `mgr.start()` in-process.

`DevManager::open` returning `remote_host` is not a model for `start` — it does so because the CLI needs to SSH-forward the tmux *attach* step after session creation, which is a separate concern.

---

## 4. HTTP status ignored in `cmd_run_in` — `dev-cli/src/main.rs:620`

### Context

`http_over_uds` is a hand-rolled HTTP/1.1 client over a Unix domain socket (no `reqwest` or `hyper` — raw bytes). It:
1. Opens a `UnixStream` to the daemon socket
2. Writes a raw HTTP request
3. Reads the full response
4. Splits on `\r\n\r\n` to find the body
5. JSON-parses the body and returns `serde_json::Value`

The status code is parsed but thrown away (line 681 parses only the body). A `500 {"error": "session not found"}` and a `200 {"stdout": "..."}` both return `Ok(Value)`. `cmd_run_in` then calls `.get("stdout")` on the error response, gets `None`, prints a blank line with `exit=-1`.

### Design question (TDD forcing function)
Return type should be `(u16, serde_json::Value)` so callers can branch on status. The status line is already in the raw bytes (`HTTP/1.1 500 ...`) — needs to be parsed and threaded through. Every command that calls `http_over_uds` benefits.

### Fix
Parse and return the HTTP status line in `http_over_uds`; surface the `"error"` field on non-200 responses in all callers.

---

## 5. `@` in a path silently breaks host parsing — `dev-lib/src/config.rs:98`

`rsplit_once('@')` takes the *last* `@`. A config line like `proj=default:~/some@dir` (path contains `@`, no host intended) silently parses `custom_path = "~/some"` and `host = "dir"`. No warning, and the project gets routed to `ssh dir`.

**Fix:** Require the `@host` suffix to follow the layout/path portion with no ambiguity — e.g. split on layout first, then check if the remainder (after `:`) contains `@` only at the end, or document that `@` is reserved in paths and validate accordingly.

---

## 6. `?lines=` not validated as an integer — `dev-lib/src/daemon.rs:280`

The raw query string value is formatted directly into `-{n}` and passed to `tmux capture-pane`. A non-numeric value produces a confusing tmux error rather than a clean `400`.

**Fix:**
```rust
if let Some(n) = lines {
    let n: u32 = n.parse().map_err(|_| anyhow::anyhow!("lines must be a positive integer"))?;
    // return 400 on error
    start_arg = format!("-{n}");
    args.push("-S");
    args.push(&start_arg);
}
```

---

## 7. `run-in` always runs locally even with `default_host` set — `dev-cli/src/main.rs:124`

### Resolution

`run-in` operates on the local daemon socket by design — UDS is inherently local. When HTTPS transport is added, remote dispatch becomes a real feature (POST to remote daemon directly). Patching routing logic now would be immediately replaced.

**Applied:** improved the connection error message to hint at the remote host scenario:
> `connect to dev daemon at <path> (is 'dev daemon' running? if sessions are on a remote host, ssh there and run directly)`

Remote dispatch deferred to the HTTPS transport work.

---

## 8. `DaemonState` holds a separate `RealTmux` alongside `DevManager` — `dev-lib/src/daemon.rs:37`

The daemon uses `state.tmux` (a bare `RealTmux`) for low-level pane ops and `state.manager` (which already owns a `Box<dyn TmuxBackend>`) for high-level ops. The daemon is untestable with a mock backend because you'd need to swap both independently.

**Fix:** Remove `tmux` from `DaemonState`; expose pane-level operations through `DevManager` or access the backend via `manager`'s existing field.

---

## 9. Silent fallback to empty `projects_dir` — `dev-lib/src/api.rs:62`

`dirs::home_dir().unwrap_or_default()` falls back to `""` when `HOME` is unset (containers, some service contexts). Discovery silently returns zero projects. Should bail with a clear error like the socket path resolution does.

**Fix:**
```rust
let projects_dir = dirs::home_dir()
    .map(|h| h.join("Projects"))
    .context("HOME directory not set")?;
```

---

## 10. `parse_layout` implemented twice — `dev-cli/src/main.rs:186` and `dev-lib/src/daemon.rs:251`

Two separate implementations; the daemon's version is better (returns `Result`). A new layout variant requires changes in three files.

**Fix:** Move a single `parse_layout(s: &str) -> Result<Layout>` into `dev-lib::config` where `Layout` is defined, and use it from both call sites.

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
| 5 | `proj=default:~/some@dir` errors or treats whole string as path | `dev-lib/config.rs` |
| 6 | Non-integer `?lines=abc` returns 400 | `dev-lib/daemon.rs` |
| 7 | `run-in` with `default_host` set and no `--local` returns clear error | `dev-cli` |
| 8 | Daemon pane ops work with `MockTmux` (enabled by finding 8 refactor) | `dev-lib/daemon.rs` |
| 9 | `list_projects` returns error when HOME is unset | `dev-lib/api.rs` |
| 10 | Single `parse_layout` in `dev-lib::config`; delete duplicate | `dev-lib/config.rs` |

### TDD priority (write tests first to drive interface decisions)

1. **Finding 3** — Forces: what does `DevManager::start` return for a remote project? What HTTP status does the daemon emit?
2. **Finding 7** — Forces: what is the `run-in` contract when `default_host` is set? Error or forward?
3. **Finding 4** — Forces: what does `http_over_uds` return? Locks the type used by all `dev-cli` commands.
