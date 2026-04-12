//! Unix-socket JSON daemon that exposes `DevManager` + `TmuxBackend` over
//! a minimal HTTP/1.1-like protocol at `~/.local/run/dev.sock`.
//!
//! Single-threaded on purpose: one connection at a time. Every handler is
//! synchronous (shells out to `tmux`), and the expected consumers are a
//! single AtomicGuard effector and a single chops agent — concurrency is
//! not worth the complexity here.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::api::DevManager;
use crate::tmux::{CommandOutput, RealTmux, TmuxBackend};

const HISTORY_CAP_PER_PANE: usize = 50;

/// One recorded command execution inside a tmux pane.
#[derive(Debug, Clone, Serialize)]
pub struct HistoryRecord {
    pub marker_id: String,
    pub command: String,
    pub stdout: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub at_unix_secs: u64,
}

/// Mutable daemon state — kept behind `&mut self` in the single-threaded loop.
pub struct DaemonState {
    manager: DevManager,
    tmux: RealTmux,
    history: HashMap<String, VecDeque<HistoryRecord>>,
}

impl DaemonState {
    pub fn new() -> Result<Self> {
        Ok(Self {
            manager: DevManager::new()?,
            tmux: RealTmux,
            history: HashMap::new(),
        })
    }

    fn push_history(&mut self, key: String, rec: HistoryRecord) {
        let buf = self.history.entry(key).or_default();
        if buf.len() == HISTORY_CAP_PER_PANE {
            buf.pop_front();
        }
        buf.push_back(rec);
    }
}

/// Resolve the default socket path: `$XDG_RUNTIME_DIR/dev.sock` if set,
/// else `~/.local/run/dev.sock`. The parent directory is created if missing.
pub fn default_socket_path() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        if !xdg.is_empty() {
            return Ok(PathBuf::from(xdg).join("dev.sock"));
        }
    }
    let home = dirs::home_dir().context("HOME not set")?;
    Ok(home.join(".local").join("run").join("dev.sock"))
}

/// Run the daemon. Blocks forever (or until a listener error).
///
/// Removes any stale socket at `path` first, creates the parent dir, then
/// enters an accept loop. Each connection is handled synchronously.
pub fn run(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating socket dir {}", parent.display()))?;
    }
    // Best-effort: remove a stale socket from a previous run. If a live
    // daemon is listening, the bind below will fail with EADDRINUSE and the
    // user will see a clear error.
    let _ = std::fs::remove_file(path);

    let listener =
        UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
    eprintln!("dev daemon listening on {}", path.display());

    let mut state = DaemonState::new()?;

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("accept error: {e}");
                continue;
            }
        };
        if let Err(e) = handle_connection(&mut state, stream) {
            eprintln!("connection error: {e}");
        }
    }
    Ok(())
}

// ---------- HTTP framing ----------

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    query: String,
    body: Vec<u8>,
}

fn read_request(stream: &mut UnixStream) -> Result<Request> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        bail!("malformed request line: {:?}", request_line);
    }
    let method = parts[0].to_string();
    let full_path = parts[1].to_string();
    let (path, query) = match full_path.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (full_path, String::new()),
    };

    let mut content_length: usize = 0;
    loop {
        let mut header = String::new();
        let n = reader.read_line(&mut header)?;
        if n == 0 || header == "\r\n" || header == "\n" {
            break;
        }
        if let Some(v) = header
            .strip_prefix("Content-Length:")
            .or_else(|| header.strip_prefix("content-length:"))
        {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    Ok(Request {
        method,
        path,
        query,
        body,
    })
}

fn write_response(stream: &mut UnixStream, status: u16, body: &Value) -> Result<()> {
    let body_bytes = serde_json::to_vec(body)?;
    let status_text = match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body_bytes.len()
    )?;
    stream.write_all(&body_bytes)?;
    stream.flush()?;
    Ok(())
}

// ---------- Routing ----------

fn handle_connection(state: &mut DaemonState, mut stream: UnixStream) -> Result<()> {
    let req = match read_request(&mut stream) {
        Ok(r) => r,
        Err(e) => {
            let _ = write_response(&mut stream, 400, &json!({"error": e.to_string()}));
            return Ok(());
        }
    };

    let result = route(state, &req);
    match result {
        Ok((status, body)) => write_response(&mut stream, status, &body),
        Err(e) => write_response(&mut stream, 500, &json!({"error": e.to_string()})),
    }
}

fn route(state: &mut DaemonState, req: &Request) -> Result<(u16, Value)> {
    let segs: Vec<&str> = req.path.trim_matches('/').split('/').collect();

    match (req.method.as_str(), segs.as_slice()) {
        ("GET", ["sessions"]) => handle_list_sessions(state),
        ("POST", ["sessions"]) => handle_start_session(state, &req.body),
        ("DELETE", ["sessions", name]) => handle_stop_session(state, name),
        ("GET", ["sessions", name, "panes"]) => handle_list_panes(state, name),
        ("GET", ["sessions", name, "panes", pane, "content"]) => {
            handle_pane_content(state, name, pane, &req.query)
        }
        ("POST", ["sessions", name, "panes", pane, "keys"]) => {
            handle_send_keys(state, name, pane, &req.body)
        }
        ("POST", ["sessions", name, "panes", pane, "run"]) => {
            handle_run(state, name, pane, &req.body)
        }
        ("GET", ["sessions", name, "panes", pane, "history"]) => {
            handle_history_list(state, name, pane)
        }
        ("GET", ["sessions", name, "panes", pane, "history", marker]) => {
            handle_history_get(state, name, pane, marker)
        }
        _ => Ok((
            404,
            json!({"error": "not found", "method": req.method, "path": req.path}),
        )),
    }
}

// ---------- Handlers ----------

fn handle_list_sessions(state: &mut DaemonState) -> Result<(u16, Value)> {
    let out = state.manager.list()?;
    Ok((200, serde_json::to_value(out)?))
}

#[derive(Deserialize)]
struct StartBody {
    project: String,
    #[serde(default)]
    layout: Option<String>,
}

fn handle_start_session(state: &mut DaemonState, body: &[u8]) -> Result<(u16, Value)> {
    let b: StartBody = serde_json::from_slice(body).context("parse start body")?;
    let layout = b.layout.as_deref().map(parse_layout).transpose()?;
    let name = state.manager.start(&b.project, layout)?;
    Ok((201, json!({"session": name})))
}

fn parse_layout(s: &str) -> Result<crate::config::Layout> {
    match s {
        "default" => Ok(crate::config::Layout::Default),
        "claude" => Ok(crate::config::Layout::Claude),
        other => bail!("unknown layout: {other}"),
    }
}

fn handle_stop_session(state: &mut DaemonState, name: &str) -> Result<(u16, Value)> {
    state.manager.stop(name)?;
    Ok((200, json!({"stopped": name})))
}

fn handle_list_panes(state: &mut DaemonState, name: &str) -> Result<(u16, Value)> {
    let count = state.tmux.list_panes(name)?;
    Ok((200, json!({"session": name, "pane_count": count})))
}

fn handle_pane_content(
    _state: &mut DaemonState,
    name: &str,
    pane: &str,
    query: &str,
) -> Result<(u16, Value)> {
    let lines = query_param(query, "lines");
    let target = format!("{name}:{pane}");
    let mut args = vec!["capture-pane", "-p", "-t"];
    args.push(&target);
    let start_arg;
    if let Some(n) = lines.as_ref() {
        start_arg = format!("-{n}");
        args.push("-S");
        args.push(&start_arg);
    }
    // Shell out directly — we want the unaltered pane content, not the
    // error-on-nonzero wrapping the other methods do.
    let output = std::process::Command::new("tmux").args(&args).output()?;
    if !output.status.success() {
        bail!(
            "capture-pane failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok((
        200,
        json!({
            "content": String::from_utf8_lossy(&output.stdout).to_string()
        }),
    ))
}

#[derive(Deserialize)]
struct KeysBody {
    keys: String,
}

fn handle_send_keys(
    state: &mut DaemonState,
    name: &str,
    pane: &str,
    body: &[u8],
) -> Result<(u16, Value)> {
    let b: KeysBody = serde_json::from_slice(body).context("parse keys body")?;
    let target = format!("{name}:{pane}");
    state.tmux.send_keys(&target, &b.keys)?;
    Ok((202, json!({"sent": b.keys.len()})))
}

#[derive(Deserialize)]
struct RunBody {
    command: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

fn handle_run(
    state: &mut DaemonState,
    name: &str,
    pane: &str,
    body: &[u8],
) -> Result<(u16, Value)> {
    let b: RunBody = serde_json::from_slice(body).context("parse run body")?;
    let target = format!("{name}:{pane}");
    let timeout = Duration::from_millis(b.timeout_ms.unwrap_or(30_000));
    let out: CommandOutput = state.tmux.run_and_capture(&target, &b.command, timeout)?;

    let rec = HistoryRecord {
        marker_id: out.marker_id.clone(),
        command: b.command.clone(),
        stdout: out.stdout.clone(),
        exit_code: out.exit_code,
        duration_ms: out.duration_ms,
        at_unix_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    state.push_history(format!("{name}:{pane}"), rec);

    Ok((
        200,
        json!({
            "stdout": out.stdout,
            "exit_code": out.exit_code,
            "duration_ms": out.duration_ms,
            "marker_id": out.marker_id,
        }),
    ))
}

fn handle_history_list(state: &mut DaemonState, name: &str, pane: &str) -> Result<(u16, Value)> {
    let key = format!("{name}:{pane}");
    let items: Vec<&HistoryRecord> = state
        .history
        .get(&key)
        .map(|b| b.iter().collect())
        .unwrap_or_default();
    Ok((200, serde_json::to_value(items)?))
}

fn handle_history_get(
    state: &mut DaemonState,
    name: &str,
    pane: &str,
    marker: &str,
) -> Result<(u16, Value)> {
    let key = format!("{name}:{pane}");
    let found = state
        .history
        .get(&key)
        .and_then(|b| b.iter().find(|r| r.marker_id == marker));
    match found {
        Some(r) => Ok((200, serde_json::to_value(r)?)),
        None => Ok((404, json!({"error": "marker not found"}))),
    }
}

// ---------- helpers ----------

fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(v.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_param_present() {
        assert_eq!(
            query_param("lines=500&foo=bar", "lines"),
            Some("500".into())
        );
        assert_eq!(query_param("foo=bar", "lines"), None);
        assert_eq!(query_param("", "lines"), None);
    }

    #[test]
    fn parse_layout_known() {
        assert!(matches!(
            parse_layout("default").unwrap(),
            crate::config::Layout::Default
        ));
        assert!(matches!(
            parse_layout("claude").unwrap(),
            crate::config::Layout::Claude
        ));
        assert!(parse_layout("weird").is_err());
    }
}
