use std::io::{self, BufRead, Write};
use std::process::Command;

use anyhow::{bail, Context, Result};

use dev_lib::api::{DevManager, Target};
use dev_lib::config::Layout;

// ANSI colors (only when stdout is a tty)
struct Colors {
    bold: &'static str,
    red: &'static str,
    green: &'static str,
    yellow: &'static str,
    cyan: &'static str,
    nc: &'static str,
}

impl Colors {
    fn new() -> Self {
        if atty::is(atty::Stream::Stdout) {
            Self {
                bold: "\x1b[1m",
                red: "\x1b[0;31m",
                green: "\x1b[0;32m",
                yellow: "\x1b[1;33m",
                cyan: "\x1b[1;36m",
                nc: "\x1b[0m",
            }
        } else {
            Self {
                bold: "",
                red: "",
                green: "",
                yellow: "",
                cyan: "",
                nc: "",
            }
        }
    }
}

fn die(msg: &str) -> ! {
    let c = Colors::new();
    eprintln!("{}Error: {}{}", c.red, msg, c.nc);
    std::process::exit(1);
}

fn info(msg: &str) {
    let c = Colors::new();
    eprintln!("{}{}{}", c.cyan, msg, c.nc);
}

fn main() {
    if let Err(e) = run() {
        die(&e.to_string());
    }
}

fn run() -> Result<()> {
    let raw: Vec<String> = std::env::args().skip(1).collect();

    // Version check before tmux guard — no tmux needed to print a version.
    if raw.first().map(|s| s.as_str()) == Some("version")
        || raw.iter().any(|a| a == "--version" || a == "-V")
    {
        println!("{}", env!("DEV_VERSION"));
        return Ok(());
    }

    // Ensure tmux is available
    if Command::new("tmux").arg("-V").output().is_err() {
        bail!("tmux is not installed");
    }

    // Extract global flags before command dispatch.
    //   --local   run against the local machine even when default_host is set
    //   --force / -y   skip confirmation prompts (kill-all)
    let local = raw.iter().any(|a| a == "--local");
    let force = raw.iter().any(|a| a == "--force" || a == "-y");
    let args: Vec<&str> = raw
        .iter()
        .filter(|a| *a != "--local" && *a != "--force" && *a != "-y")
        .map(|a| a.as_str())
        .collect();

    match args.first().copied() {
        None => cmd_picker(local),
        Some("help" | "--help" | "-h") => {
            cmd_help();
            Ok(())
        }
        Some("list") => cmd_list(local),
        Some("start") => {
            let project = args.get(1).copied().unwrap_or_else(|| {
                die("Usage: dev start <project> [layout]");
            });
            let layout = args.get(2).map(|s| parse_layout(s));
            cmd_start(project, layout)
        }
        Some("stop") => {
            let session = args.get(1).copied().unwrap_or_else(|| {
                die("Usage: dev stop <session>");
            });
            cmd_stop(session, local)
        }
        Some("detach") => cmd_detach(),
        Some("kill") => {
            let name = args.get(1).copied().unwrap_or_else(|| {
                die("Usage: dev kill <session-name>");
            });
            cmd_kill(name)
        }
        Some("kill-all") => cmd_kill_all(local, force),
        Some("claude") => {
            let project = args.get(1).copied().unwrap_or_else(|| {
                die("Usage: dev claude <project>");
            });
            cmd_open(project, Some(Layout::Claude))
        }
        Some("layout") => cmd_layout(args.get(1).copied()),
        Some("daemon") => cmd_daemon(),
        Some("run-in") => {
            let tail: Vec<String> = raw
                .iter()
                .skip(1)
                .filter(|a| *a != "--local" && *a != "--force" && *a != "-y")
                .cloned()
                .collect();
            cmd_run_in(&tail)
        }
        Some(project) => cmd_open(project, None),
    }
}

fn cmd_help() {
    print!(
        "\
dev - Persistent tmux session manager for multi-device development

USAGE
  dev                     Interactive picker (fzf or numbered fallback)
  dev <project>           Create or attach to session for <project>
  dev claude <project>    Force claude+shell layout (vertical split)
  dev layout [name]       Show or change layout (claude = add claude pane)
  dev list                JSON output of sessions and projects
  dev start <project>     Start a session without attaching
  dev stop <session>      Stop (kill) a session
  dev detach              Detach from current tmux session
  dev kill <name>         Kill a session
  dev kill-all            Kill all sessions (with confirmation)
  dev version             Print version and exit
  dev help                Show this help

FLAGS
  --local                 Run against the local machine even when default_host is set
                          (applies to: dev, list, stop, kill-all)
  --force / -y            Skip confirmation prompt (applies to: kill-all)

LAYOUTS
  default                 Single shell pane in the project directory
  claude                  Vertical split: claude (left) + shell (right)

PROJECT DISCOVERY
  Projects are auto-discovered from ~/Projects (up to 3 levels deep).
  A project is any directory containing a .git folder.
  If names collide, the category/project form is used.

CONFIGURATION
  Per-project layouts are configured in ~/.config/dev/config:

    default_layout=default
    atomicguard=claude@myserver
    web-app-deploy=claude@myserver
    my-local-thing=default
    dotfiles=claude:~/.local/share/chezmoi

  Format: project=layout[:path][@host]
  - layout: default, claude
  - :path:  optional custom directory (expands ~); omit for ~/Projects projects
  - @host:  optional SSH hostname; omit for local projects
"
    );
}

fn parse_layout(s: &str) -> Layout {
    match s {
        "claude" => Layout::Claude,
        _ => Layout::Default,
    }
}

// --- Commands ----------------------------------------------------------------

fn cmd_list(local: bool) -> Result<()> {
    let mgr = DevManager::new()?;
    if !local {
        if let Some(host) = mgr.remote_host() {
            return forward_remote(&host, &["list"]);
        }
    }
    let output = mgr.list()?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

fn cmd_start(project: &str, layout: Option<Layout>) -> Result<()> {
    let mgr = DevManager::new()?;
    if let Target::Remote(host) = mgr.resolve_target(project) {
        return forward_remote(&host, &["start", project]);
    }
    let session_name = mgr.start(project, layout)?;
    info(&format!("Session '{}' ready", session_name));
    Ok(())
}

fn cmd_stop(session: &str, local: bool) -> Result<()> {
    let mgr = DevManager::new()?;
    if !local {
        if let Some(host) = mgr.remote_host() {
            return forward_remote(&host, &["stop", session]);
        }
    }
    mgr.stop(session)?;
    info(&format!("Session '{}' stopped", session));
    Ok(())
}

fn cmd_open(query: &str, force_layout: Option<Layout>) -> Result<()> {
    let mgr = DevManager::new()?;
    let result = mgr.open(query, force_layout)?;

    if let Some(host) = result.remote_host {
        return forward_remote(&host, &[query]);
    }

    if result.created {
        info(&format!("Created session '{}'", result.session_name));
    }

    // Attach or switch
    attach(&result.session_name)
}

fn cmd_detach() -> Result<()> {
    if std::env::var("TMUX").is_err() {
        bail!("Not inside a tmux session");
    }
    let status = Command::new("tmux").arg("detach-client").status()?;
    if !status.success() {
        bail!("Failed to detach");
    }
    Ok(())
}

fn cmd_kill(name: &str) -> Result<()> {
    let mgr = DevManager::new()?;
    if let Target::Remote(host) = mgr.resolve_target(name) {
        return forward_remote(&host, &["kill", name]);
    }
    mgr.stop(name)?;
    info(&format!("Session '{}' killed", name));
    Ok(())
}

fn cmd_kill_all(local: bool, force: bool) -> Result<()> {
    let mgr = DevManager::new()?;
    if !local {
        if let Some(host) = mgr.remote_host() {
            let mut fwd_args = vec!["kill-all"];
            if force {
                fwd_args.push("--force");
            }
            return forward_remote(&host, &fwd_args);
        }
    }
    let output = mgr.list()?;
    let count = output.sessions.len();

    if count == 0 {
        info("No sessions to kill");
        return Ok(());
    }

    if !force {
        let c = Colors::new();
        eprintln!("{}This will kill {} session(s):{}", c.yellow, count, c.nc);
        for s in &output.sessions {
            eprintln!("  {} ({})", s.name, s.layout);
        }
        eprint!("Confirm? [y/N] ");
        io::stderr().flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            info("Cancelled");
            return Ok(());
        }
    }
    mgr.kill_all()?;
    info("All sessions killed");

    Ok(())
}

fn cmd_layout(name: Option<&str>) -> Result<()> {
    if std::env::var("TMUX").is_err() {
        bail!("Not inside a tmux session. Use 'dev claude <project>' to create a new session.");
    }

    let pane_count_output = Command::new("tmux")
        .args(["list-panes", "-F", "#{pane_index}"])
        .output()?;
    let pane_count = String::from_utf8_lossy(&pane_count_output.stdout)
        .trim()
        .lines()
        .count();

    match name {
        None => {
            if pane_count >= 2 {
                info(&format!(
                    "Current layout: claude+shell ({} panes)",
                    pane_count
                ));
            } else {
                info("Current layout: default (1 pane)");
                eprintln!("  Use 'dev layout claude' to split");
            }
            Ok(())
        }
        Some("claude") => {
            if pane_count >= 2 {
                info("Already in claude layout");
                return Ok(());
            }
            // Split: new pane on left, run claude
            let status = Command::new("tmux")
                .args(["split-window", "-hb", "-c", "#{pane_current_path}"])
                .status()?;
            if !status.success() {
                bail!("Failed to split window");
            }
            let status = Command::new("tmux")
                .args(["send-keys", "claude", "Enter"])
                .status()?;
            if !status.success() {
                bail!("Failed to start claude");
            }
            // Focus right pane (shell)
            Command::new("tmux").args(["select-pane", "-R"]).status()?;
            Ok(())
        }
        Some(other) => bail!("Unknown layout: {}", other),
    }
}

fn cmd_picker(local: bool) -> Result<()> {
    let mgr = DevManager::new()?;
    if !local {
        if let Some(host) = mgr.remote_host() {
            return forward_remote(&host, &[]);
        }
    }
    let output = mgr.list()?;

    if output.sessions.is_empty() && output.projects.is_empty() {
        bail!("No sessions or projects found");
    }

    // Try fzf first, fall back to numbered list
    if Command::new("fzf").arg("--version").output().is_ok() {
        picker_fzf(&mgr, &output)
    } else {
        picker_fallback(&mgr, &output)
    }
}

// --- Picker ------------------------------------------------------------------

fn picker_fzf(_mgr: &DevManager, output: &dev_lib::api::ListOutput) -> Result<()> {
    let mut entries: Vec<String> = Vec::new();

    for s in &output.sessions {
        let status = if s.attached { "(attached)" } else { "" };
        let layout = if s.layout == "claude" {
            "claude+shell"
        } else {
            "shell"
        };
        entries.push(format!(
            "[session]  {:<22} {:<14} {}",
            s.name, layout, status
        ));
    }

    for p in &output.projects {
        let host_info = p
            .host
            .as_ref()
            .map(|h| format!("  @{h}"))
            .unwrap_or_default();
        entries.push(format!("[project]  {:<22} {}{}", p.name, p.path, host_info));
    }

    let input = entries.join("\n");
    let child = Command::new("fzf")
        .args([
            "--ansi",
            "--header=  dev: select a session or project",
            "--prompt=  > ",
            "--height=40%",
            "--reverse",
            "--no-sort",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()?;

    let mut child = child;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes())?;
    }

    let output_result = child.wait_with_output()?;
    if !output_result.status.success() {
        return Ok(()); // User cancelled
    }

    let selection = String::from_utf8_lossy(&output_result.stdout)
        .trim()
        .to_string();
    let parts: Vec<&str> = selection.splitn(3, char::is_whitespace).collect();
    if parts.len() < 2 {
        return Ok(());
    }

    let type_tag = parts[0];
    // The name is the first non-empty token after the tag
    let name = selection
        .trim_start_matches(type_tag)
        .split_whitespace()
        .next()
        .unwrap_or("");

    if type_tag == "[session]" {
        attach(name)
    } else if type_tag == "[project]" {
        cmd_open(name, None)
    } else {
        Ok(())
    }
}

fn picker_fallback(_mgr: &DevManager, output: &dev_lib::api::ListOutput) -> Result<()> {
    let c = Colors::new();
    let mut names: Vec<(String, String)> = Vec::new(); // (type, name)

    if !output.sessions.is_empty() {
        eprintln!("\n{} ACTIVE SESSIONS{}", c.bold, c.nc);
        for s in &output.sessions {
            let idx = names.len() + 1;
            let status = if s.attached { "(attached)" } else { "" };
            let layout = if s.layout == "claude" {
                "claude+shell"
            } else {
                "shell"
            };
            eprintln!(
                "  {}{:2}){} {:<22} {:<14} {}",
                c.green, idx, c.nc, s.name, layout, status
            );
            names.push(("session".to_string(), s.name.clone()));
        }
    }

    if !output.projects.is_empty() {
        eprintln!("\n{} AVAILABLE PROJECTS{}", c.bold, c.nc);
        for p in &output.projects {
            let idx = names.len() + 1;
            let host_info = p
                .host
                .as_ref()
                .map(|h| format!("  @{h}"))
                .unwrap_or_default();
            eprintln!(
                "  {}{:2}){} {:<22} {}{}",
                c.green, idx, c.nc, p.name, p.path, host_info
            );
            names.push(("project".to_string(), p.name.clone()));
        }
    }

    eprintln!();
    eprint!("  Select [1-{}]: ", names.len());
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;

    let choice: usize = input.trim().parse().unwrap_or(0);
    if choice < 1 || choice > names.len() {
        bail!("Invalid selection");
    }

    let (ref type_tag, ref name) = names[choice - 1];
    if type_tag == "session" {
        attach(name)
    } else {
        cmd_open(name, None)
    }
}

// --- Helpers -----------------------------------------------------------------

fn attach(session: &str) -> Result<()> {
    if std::env::var("TMUX").is_ok() {
        // Inside tmux: switch client
        let status = Command::new("tmux")
            .args(["switch-client", "-t", session])
            .status()?;
        if !status.success() {
            bail!("Failed to switch to session '{}'", session);
        }
    } else {
        // Outside tmux: attach (replaces current process on Unix)
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let err = Command::new("tmux")
                .args(["attach-session", "-t", session])
                .exec();
            // exec only returns on error
            bail!("Failed to attach: {}", err);
        }
        #[cfg(not(unix))]
        {
            let status = Command::new("tmux")
                .args(["attach-session", "-t", session])
                .status()?;
            if !status.success() {
                bail!("Failed to attach to session '{}'", session);
            }
        }
    }
    Ok(())
}

/// Forward a command to a remote host via SSH, replacing the current process.
/// On Unix this never returns on success (`exec`). On non-Unix or on SSH
/// failure it returns an `Err` which the caller propagates normally.
fn forward_remote(host: &str, args: &[&str]) -> Result<()> {
    let dev_args = args.join(" ");
    let remote_cmd = format!("dev {dev_args}");

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = Command::new("ssh").args(["-t", host, &remote_cmd]).exec();
        bail!("ssh to {host}: {err}");
    }
    #[cfg(not(unix))]
    bail!("remote forwarding is not supported on this platform");
}

fn cmd_daemon() -> Result<()> {
    let path = dev_lib::daemon::default_socket_path()?;
    info(&format!("Starting dev daemon on {}", path.display()));
    dev_lib::daemon::run(&path)
}

fn cmd_run_in(args: &[String]) -> Result<()> {
    // Parse: dev run-in <session>[:<window>.<pane>] <command...> [--timeout N] [--json]
    let mut positional: Vec<&str> = Vec::new();
    let mut timeout_ms: u64 = 30_000;
    let mut json_out = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--json" => {
                json_out = true;
                i += 1;
            }
            "--timeout" => {
                let v = args
                    .get(i + 1)
                    .unwrap_or_else(|| die("--timeout requires a value"));
                let secs: u64 = v
                    .parse()
                    .unwrap_or_else(|_| die("--timeout must be an integer (seconds)"));
                timeout_ms = secs * 1000;
                i += 2;
            }
            _ => {
                positional.push(a);
                i += 1;
            }
        }
    }
    if positional.len() < 2 {
        die("Usage: dev run-in <session>[:<window>.<pane>] <command> [--timeout N] [--json]");
    }
    let target = positional[0];
    let command = positional[1..].join(" ");

    // Split "session" or "session:window.pane"
    let (session, pane) = match target.split_once(':') {
        Some((s, p)) => (s, p),
        None => (target, "1.1"),
    };

    let body = serde_json::json!({
        "command": command,
        "timeout_ms": timeout_ms,
    });
    let path = format!("/sessions/{session}/panes/{pane}/run");
    let resp = http_over_uds("POST", &path, Some(&body))?;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&resp)?);
    } else {
        let stdout = resp.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        let exit = resp.get("exit_code").and_then(|v| v.as_i64()).unwrap_or(-1);
        let dur = resp
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        print!("{stdout}");
        if !stdout.ends_with('\n') {
            println!();
        }
        eprintln!("exit={exit} duration_ms={dur}");
        if exit != 0 {
            std::process::exit(exit as i32);
        }
    }
    Ok(())
}

fn http_over_uds(
    method: &str,
    path: &str,
    body: Option<&serde_json::Value>,
) -> Result<serde_json::Value> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    let socket_path = dev_lib::daemon::default_socket_path()?;
    let mut stream = UnixStream::connect(&socket_path).with_context(|| {
        format!(
            "connect to dev daemon at {} (is `dev daemon` running?)",
            socket_path.display()
        )
    })?;

    let body_bytes = match body {
        Some(v) => serde_json::to_vec(v)?,
        None => Vec::new(),
    };
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body_bytes.len()
    );
    stream.write_all(req.as_bytes())?;
    if !body_bytes.is_empty() {
        stream.write_all(&body_bytes)?;
    }
    stream.flush()?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    // Split headers from body on the CRLFCRLF boundary.
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("malformed response"))?;
    let body_start = split + 4;
    let body_slice = &raw[body_start..];
    let v: serde_json::Value = serde_json::from_slice(body_slice)?;
    Ok(v)
}
