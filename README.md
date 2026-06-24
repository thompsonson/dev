# dev

A tmux session manager and control plane. Two things in one repo:

1. **A CLI** (`dev`) — open, list, and kill persistent tmux sessions for your projects. A Rust port of the original [`dot_local/bin/executable_dev`](https://github.com/thompsonson/dotfiles) bash script.
2. **A local daemon** (`dev daemon`) — exposes that same tmux control surface as a Unix-socket JSON API, so other tools (effectors, agents, UIs) can drive tmux without reimplementing subprocess plumbing or MQTT routing.

If you just want to SSH into a box and `dev my-project` into a session that survives disconnects, you only need §1 (User). If you're building an effector or an agent that needs to run a command from a session's current project directory and capture its output, you want §2 (Integrator).

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
dev peek <session>            Print latest pane content without interacting
dev inspect <session>         Print JSON session metadata, git state, and pane content
dev run-in <target> <cmd>     Run a background command from the pane cwd and capture output
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
manta-site  default  1      3m
```

Each session and project entry also includes coordination metadata when known:

- `project_path`: local project directory for active sessions, or `null` when unknown.
- `repository`: informational HTTPS repository URL from config or `git remote get-url origin`.
- `responsibility`: what that session/project is responsible for.

Agents and clients can use `dev list` as the live discovery mechanism, choose a peer by `repository` or `responsibility`, then send a message:

```bash
dev send manta-site "From dev: can you check the site build failure?"
```

Use `gh` for GitHub operations against the reported repository when needed.

For read-only session state, use `dev peek` or `dev inspect`:

```bash
dev peek manta-site --lines 40
dev inspect manta-site | jq
dev inspect manta-site --full | jq
```

`peek` returns raw visible pane text. `inspect` combines session metadata, git state, and pane content without typing into the target session.

### Choosing `send` vs `run-in`

| Use case | Command |
|---|---|
| Ask an active Claude/OpenCode TUI agent | `dev send <session> "..."` |
| Run a shell command and capture stdout/exit | `dev run-in <session> "cmd"` |
| Ask a new non-interactive agent process | `dev run-in <session> 'opencode run "...prompt..."'` |
| Read state without interacting | `dev peek`, `dev inspect` |

Use `dev send` when the target pane is already running an interactive agent such as Claude Code or OpenCode:

```bash
dev send manta-site "Please run the standard tests and report the result."
```

`send` injects visible text into the target pane and presses Enter. The active TUI sees it.

Use `dev run-in` when you need deterministic command execution and captured output:

```bash
dev run-in manta-site "git status --short"
dev run-in manta-site "python manage.py test" --timeout 120
dev run-in manta-site "git status --short" --json
```

`run-in` does not type into the visible pane and does not communicate with an already-running TUI agent. It reads the target pane's current working directory, runs the command in a background `/bin/sh` via tmux, captures stdout and exit code, and leaves the pane untouched.

If an agent CLI supports non-interactive execution and exits, `run-in` can start a separate agent process and capture its answer:

```bash
dev run-in manta-site 'opencode run "What tests should I run for this repo?"'
dev run-in manta-site 'claude -p "Summarize current git state"'
```

This starts a new process. It does not ask the already-running TUI agent in the pane. For that, use `dev send`.

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

[project.dev]
path = "~/Projects/thompsonson/dev"
repository = "https://github.com/thompsonson/dev"
responsibility = "Maintain the dev CLI, daemon, bootstrap, release, and session-control workflows"
```

`layout` is `default` or `claude`; `path` lets a project key point at a custom directory; `host` forwards the command via SSH to another machine. `repository` and `responsibility` are optional metadata surfaced by `dev list` for agents and clients.

---

## Integrator guide

### Why a daemon

Lots of tools need to "run this command using that pane's current working directory and tell me what happened." Each one tends to reinvent the same few tmux subprocess calls, then invents its own way to capture output (which tmux's fire-and-forget `send-keys` does not give you). The `dev` daemon centralises that:

- **One process**, one implementation of command output capture via daemon-managed background execution and staging files, one place to fix bugs in it.
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

- **`run-in` output is command stdout, not pane capture.** The daemon uses `tmux run-shell -b` to start a background `/bin/sh`, redirects stdout/stderr to daemon-managed staging files, records the exit code, and returns stdout plus exit status. The visible pane is untouched.
- **`run-in` starts in the target pane's current working directory.** It does not run inside the pane's interactive shell. Aliases, shell functions, exports, and directory changes from that shell do not carry over unless they are part of the command itself.
- **stderr is not currently returned by the CLI response.** The daemon stages it separately for execution bookkeeping, but the response shape exposes stdout, exit code, duration, and marker id.
- **Timeouts are enforced by polling the daemon staging files.** On timeout the daemon returns HTTP 500 with an error and the background command may keep running; the daemon does not kill it for you.
- **`duration_ms` is wall-clock time inside the daemon**, measured from dispatch to completion detection. It includes polling latency; treat it as a lower bound.
- **History is per-pane, keyed by `"<session>:<pane>"`.** `run-in` records command results against the target pane key even though execution happens in a background shell. If you rename a session or rebuild a pane, history for the old key stays in memory until the daemon restarts or is evicted by the 50-record cap.

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
