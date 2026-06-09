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

## 3. `POST /sessions` silently creates a local session for a remote-configured project — `dev-lib/src/daemon.rs:245`

`DevManager::start` never calls `resolve_target`, so a project configured with `@remotehost` gets a local tmux session created instead of being forwarded or rejected. The daemon returns `201` and the caller has no idea the session is in the wrong place.

**Fix:** Call `resolve_target` in `start` (or in the daemon handler) and return `DevError::RemoteProject` for remote-targeted projects.

---

## 4. HTTP status ignored in `cmd_run_in` — `dev-cli/src/main.rs:620`

`http_over_uds` parses the JSON body regardless of HTTP status code. A `500 {"error": "session not found"}` from the daemon is treated as a `200`; `resp.get("stdout")` returns `None`, and the output is a blank line with `exit=-1` — no indication of the actual error.

**Fix:** Parse and check the HTTP status line in `http_over_uds`; surface the `"error"` field on non-200 responses.

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

Every other command checks `mgr.remote_host()` and SSH-forwards. `run-in` silently hits the local daemon socket, which is usually empty on a client machine. The resulting "connect to dev daemon" error looks like a daemon problem, not a routing problem.

**Fix:** Either forward `run-in` via the same SSH pattern as other commands, or emit a clear error: `"run-in is not supported over a remote host; use --local or ssh directly"`.

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
