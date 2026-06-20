# dev

A tmux session manager and control plane. Two things in one repo:

1. **A CLI** (`dev`) — open, list, and kill persistent tmux sessions for your projects. A Rust port of the original [`dot_local/bin/executable_dev`](https://github.com/thompsonson/dotfiles) bash script.
2. **A local daemon** (`dev daemon`) — exposes that same tmux control surface as a Unix-socket JSON API, so other tools (effectors, agents, UIs) can drive tmux without reimplementing subprocess plumbing or MQTT routing.

If you just want to SSH into a box and `dev my-project` into a session that survives disconnects, you only need §1 (User). If you're building an effector or an agent that needs to run a command in a tmux pane and capture its output, you want §2 (Integrator).

---

## Table of contents

- [User guide](#user-guide)
  - [Install](#install)
  - [Commands](#commands)
  - [Project config](#project-config)
- [Integrator guide](#integrator-guide)
  - [Why a daemon](#why-a-daemon)
  - [Starting the daemon](#starting-the-daemon)
  - [Transport](#transport)
  - [Endpoint reference](#endpoint-reference)
  - [Client examples](#client-examples)
  - [Semantics worth knowing](#semantics-worth-knowing)
- [Development](#development)

---

## User guide

### Install

```bash
git clone git@github.com:thompsonson/dev.git
cd dev
scripts/install.sh              # build + install to ~/.local/bin/dev (no role)
scripts/install.sh --host       # daemon host: also install + start the systemd --user service (Linux)
scripts/install.sh --client HOST# client: install + record defaults.host=HOST, no daemon
scripts/install.sh --uninstall  # remove binary (and unit, if present)
```

`--systemd` is kept as a deprecated alias for `--host`.

### Roles: one host, many clients

`dev` is a distributed utility. Sessions live on a single always-on **host**;
your other devices are **clients** that drive it over SSH (Tailscale gives them
a stable name and reachability). A typical fleet:

| Machine | Command | Result |
|---|---|---|
| `pop-mini` (always-on) | `scripts/install.sh --host` | binary + `dev daemon` under systemd --user |
| laptop | `scripts/install.sh --client pop-mini` | binary + `defaults.host = "pop-mini"` |
| phone (Termux) | `pkg install rust tmux openssh && scripts/install.sh --client pop-mini` | same, no systemd |

The client role writes `defaults.host` to `~/.config/dev/config.toml`, so `dev <project>`
on the laptop or phone targets `pop-mini`. On Termux there is no systemd, so the
host role is rejected there — a phone is always a client.

### Install from a release (no clone, no cargo)

`scripts/install.sh` builds from source. To skip the toolchain entirely — the
practical path on a phone — use the bootstrap installer, which downloads a
prebuilt binary from the latest GitHub Release (CI publishes static musl Linux
binaries that also run under Termux, plus native macOS binaries):

```bash
# laptop / pop-mini
curl -fsSL https://raw.githubusercontent.com/thompsonson/dev/main/scripts/bootstrap.sh | bash

# as a client of pop-mini
curl -fsSL https://raw.githubusercontent.com/thompsonson/dev/main/scripts/bootstrap.sh | DEV_HOST=pop-mini bash
```

On a **phone/tablet (Termux)**, no chezmoi required:

```bash
pkg install tmux openssh
curl -fsSL https://raw.githubusercontent.com/thompsonson/dev/main/scripts/bootstrap.sh | DEV_HOST=pop-mini bash
```

The bootstrap detects OS/arch, verifies the published SHA-256, installs to
`$PREFIX/bin` on Termux (else `~/.local/bin`), and records `defaults.host`.
If the static binary ever fails to run on a given Android, fall back to
`pkg install rust && cargo install --git https://github.com/thompsonson/dev dev-cli`.

For dotfiles, drop the same one-liner into a chezmoi `run_once_install-dev.sh`
once chezmoi is set up; until then the curl command is the chezmoi-independent
method.

Releases are cut by pushing a tag: `git tag v0.1.0 && git push origin v0.1.0`
triggers `.github/workflows/release.yml`.

The install script is a thin wrapper around `cargo build --release` + `install`. If you'd rather do it by hand:

```bash
cargo build --release
install -m 0755 target/release/dev ~/.local/bin/dev
```

Requires `tmux` on `PATH`. Tested on Linux and macOS. The `--systemd` flag is Linux-only; the unit template lives at [`contrib/systemd/dev-daemon.service`](contrib/systemd/dev-daemon.service).

### Commands

```
dev                           Interactive picker (fzf or numbered fallback)
dev <project>                 Create or attach a session for <project>
dev claude <project>          Force the "claude" split layout
dev start <project> [layout]  Create a session without attaching
dev stop <session>            Kill a session
dev list                      Print sessions and known projects as JSON
dev detach                    Detach from the current tmux session
dev kill <name>               Kill a session by name
dev kill-all                  Kill every session (with confirmation)
dev layout [name]             Print or switch the default layout
dev daemon                    Run the Unix-socket API server
dev run-in <target> <cmd>     Run a command in a pane and capture its output
dev send <target> <msg...>    Send a message to a pane (visible to the agent)
dev help                      Full help text
```

Project discovery walks `~/Projects/` up to 3 levels deep, treating any directory containing `.git` as a project. Sessions survive disconnects.

`dev list` outputs JSON. Use `jq` to slice it — for example, sessions as a table ordered by most recent activity:

```bash
dev list | jq -r '
  def ago: if . < 60 then "\(.)s" elif . < 3600 then "\(./60|floor)m" elif . < 86400 then "\(./3600|floor)h" else "\(./86400|floor)d" end;
  ["NAME","LAYOUT","PANES","ACTIVE"],
  (.sessions | sort_by(.last_activity) | reverse | .[] |
    [.name, .layout, (.pane_count|tostring), (now - .last_activity | floor | ago)])
  | @tsv
' | column -t -s $'\t' | awk 'NR==1{print; gsub(/[^ ]/,"-"); print} NR>1'
```

```
NAME        LAYOUT   PANES  ACTIVE
----------  -------  -----  ------
dev         default  1      56s
web-app  default  1      3m
```

### Project config

Per-project layouts live in `~/.config/dev/config.toml`:

```toml
[defaults]
layout = "default"

[project.atomicguard]
layout = "claude"

[project.dotfiles]
layout = "claude"
path = "~/.local/share/chezmoi"

[project.some-remote]
layout = "default"
host = "other-host"
```

`layout` is `default` or `claude`; `path` lets a project key point at a custom directory; `host` forwards the command via SSH to another machine.

---

## Integrator guide

### Why a daemon

Lots of tools need to "run this command inside that tmux pane and tell me what happened." Each one tends to reinvent the same few tmux subprocess calls, then invents its own way to capture output (which tmux's fire-and-forget `send-keys` does not give you). The `dev` daemon centralises that:

- **One process**, one implementation of pane-output capture (the marker-sandwich technique), one place to fix bugs in it.
- **A local Unix socket** — no network, no auth story, just filesystem permissions on `~/.local/run/dev.sock`.
- **Two consumer styles on the same backend:** observe + interact (list sessions, capture content, send keys to a pane — for UIs and agents) and run + capture (synchronous request/response with exit code — for effectors that need a structured result to feed a guard). `dev send <session> <message>` and `dev run-in <session> <command>` are the CLI interfaces for these two styles respectively.

If you're plumbing tmux from a Python effector, a Rust agent, or a web UI, talk to the daemon instead of shelling out to `tmux` directly.

### Starting the daemon

```bash
dev daemon
# dev daemon listening on /run/user/1000/dev.sock
```

Socket path resolution:

1. `$XDG_RUNTIME_DIR/dev.sock` if the env var is set (Linux systemd user sessions have this).
2. `~/.local/run/dev.sock` otherwise.

The daemon runs in the foreground. It removes a stale socket on startup and errors clearly if another `dev daemon` is already listening. No PID file, no detach — run it under your preferred supervisor. The repo ships a `systemd --user` unit for that:

```bash
scripts/install.sh --systemd
journalctl --user -u dev-daemon.service -f
```

Other supervisor options that work fine: a `tmux` pane, a shell job, `dev` itself if you're feeling recursive.

### Transport

HTTP/1.1 over a Unix domain socket. One request per connection, `Connection: close`. The daemon is single-threaded on purpose — each handler blocks on `tmux` subprocesses, and the expected workload is a handful of clients making a few calls per second. Concurrency would be real engineering for zero measurable benefit at this scale.

Requests take JSON bodies, responses are JSON. On a handled error the daemon returns HTTP 500 with `{"error": "..."}`; on an unknown route, HTTP 404 with `{"error": "not found", ...}`.

### Endpoint reference

| Method | Path | Body | Returns |
|---|---|---|---|
| `GET` | `/sessions` | — | `{sessions: [...], projects: [...]}` |
| `POST` | `/sessions` | `{project, layout?}` | `201 {session}` |
| `DELETE` | `/sessions/:name` | — | `{stopped}` |
| `GET` | `/sessions/:name/panes` | — | `{session, pane_count}` |
| `GET` | `/sessions/:name/panes/:id/content?lines=N` | — | `{content}` |
| `POST` | `/sessions/:name/panes/:id/keys` | `{keys}` | `202 {sent}` |
| **`POST`** | **`/sessions/:name/panes/:id/run`** | **`{command, timeout_ms?}`** | **`{stdout, exit_code, duration_ms, marker_id}`** |
| `GET` | `/sessions/:name/panes/:id/history` | — | `[HistoryRecord, ...]` |
| `GET` | `/sessions/:name/panes/:id/history/:marker_id` | — | `HistoryRecord` |

`:id` is a tmux pane target like `1.1` or `1.2`. `layout` on `POST /sessions` is `"default"` or `"claude"` and may be omitted.

`HistoryRecord` shape:

```json
{
  "marker_id": "ad7a0ca189e3",
  "command": "cargo test",
  "stdout": "...",
  "exit_code": 0,
  "duration_ms": 4523,
  "at_unix_secs": 1775983903
}
```

History is an **in-memory ring buffer** of the last 50 executions per pane. It does not survive daemon restarts — if you need durable history, persist it yourself from the client.

### Client examples

#### curl

```bash
# list sessions
curl --unix-socket ~/.local/run/dev.sock http://localhost/sessions

# run a command and capture the result
curl --unix-socket ~/.local/run/dev.sock \
     -X POST -H 'Content-Type: application/json' \
     -d '{"command":"cargo test","timeout_ms":60000}' \
     http://localhost/sessions/myproject/panes/1.1/run

# re-read a past execution by marker_id
curl --unix-socket ~/.local/run/dev.sock \
     http://localhost/sessions/myproject/panes/1.1/history/ad7a0ca189e3
```

#### Python (stdlib only — no extra deps)

```python
import http.client, json, socket

class UnixHTTPConnection(http.client.HTTPConnection):
    def __init__(self, path, timeout=30):
        super().__init__("localhost", timeout=timeout)
        self._path = path
    def connect(self):
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(self.timeout)
        s.connect(self._path)
        self.sock = s

def run_in(session, pane, command, timeout_s=30,
           socket_path="/home/me/.local/run/dev.sock"):
    conn = UnixHTTPConnection(socket_path, timeout=timeout_s + 5)
    try:
        conn.request(
            "POST",
            f"/sessions/{session}/panes/{pane}/run",
            body=json.dumps({"command": command, "timeout_ms": timeout_s * 1000}),
            headers={"Content-Type": "application/json"},
        )
        return json.loads(conn.getresponse().read())
    finally:
        conn.close()

result = run_in("myproject", "1.1", "cargo test")
print(result["exit_code"], len(result["stdout"]))
```

#### Rust (via the CLI)

For a Rust consumer inside the same repo, depend on `dev-lib` and call `daemon::default_socket_path()` + a plain `UnixStream`, or just shell out to the CLI:

```rust
use std::process::Command;

let out = Command::new("dev")
    .args(["run-in", "myproject:1.1", "cargo test", "--json"])
    .output()?;
let resp: serde_json::Value = serde_json::from_slice(&out.stdout)?;
```

The CLI is itself a thin client of the daemon, so behaviour matches exactly.

### Semantics worth knowing

- **`stdout` is raw pane capture, not clean process output.** The daemon's `run_and_capture` works by sending start/end marker lines around your command and slicing `tmux capture-pane` output between them. That slice includes shell prompt redraws, the command echo, and anything else the terminal rendered — it is what a human would see in that pane. Post-guards that want just the command output should strip prompt lines themselves, or run against a pane using a minimal shell (`sh -c`) with a prompt like `PS1=`.
- **Marker matching is strict.** The end marker is matched as `END_<uuid> <digits>` on a whole line — a command that echoes the marker text in its own output cannot cause a false early match (regression-tested in `dev-lib/src/tmux.rs`).
- **`exit_code` is the shell's `$?`.** It comes from `echo "END_<uuid> $?"` after the command, so you get the exact exit status of the last statement the shell executed — including shell builtins and pipeline tails.
- **Timeouts are enforced by polling `capture-pane` every 100ms** for the end marker. On timeout the daemon returns HTTP 500 with an error and the command keeps running inside the pane — the daemon does not kill the command for you. If you need hard cancellation, `POST /keys` with `C-c` or similar.
- **`duration_ms` is wall-clock time inside the daemon**, measured from the first `send-keys` to the marker sighting. It includes polling latency; treat it as a lower bound.
- **History is per-pane, keyed by `"<session>:<pane>"`.** If you rename a session or rebuild a pane, history for the old key stays in memory until the daemon restarts or is evicted by the 50-record cap.
- **No concurrent `run`s on the same pane.** The daemon handles one request at a time, but even so, two back-to-back `run_and_capture` calls against the same pane would race each other's markers if you fanned out across connections. Serialise at the caller if you care.

---

## Development

```bash
cargo build
cargo test --all
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

CI runs the same four commands on ubuntu-latest and macos-latest via `.github/workflows/ci.yml`.

Pre-commit hooks (`fmt`, `cargo-check`, `clippy`, plus whitespace/EOF/YAML/TOML basics) are configured in `.pre-commit-config.yaml`. Install once per clone:

```bash
pipx install pre-commit  # if you don't already have it
pre-commit install
```

Layout:

```
dev-lib/   library crate — DevManager, TmuxBackend, daemon server
dev-cli/   binary crate — `dev` entrypoint (CLI + daemon + run-in client)
```

The `TmuxBackend` trait lives in `dev-lib/src/tmux.rs` with two implementations: `RealTmux` (shells out) and `MockTmux` (test double). Tests that touch tmux go through `MockTmux`; there are no live-tmux integration tests in the suite — manual smoke via the CLI fills that gap.

### Related

This repo is the tmux control plane referenced in [thompsonson/atomicguard#131](https://github.com/thompsonson/atomicguard/issues/131). Consumers currently in flight:

- **AtomicGuard** — `TmuxSessionEffector` (UDS client, Python) — tracked in thompsonson/atomicguard#133.
- **chops** — `agent-core` + `plugin-runner` rewiring to call the daemon instead of shelling out. Not yet scheduled.
