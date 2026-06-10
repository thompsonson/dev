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
        Some("doctor") => {
            let config_path = args
                .iter()
                .position(|a| *a == "--config")
                .and_then(|i| args.get(i + 1).copied())
                .map(std::path::PathBuf::from);
            cmd_doctor(config_path.as_deref())
        }
        Some("update") => {
            let check_only = raw.iter().any(|a| a == "--check");
            cmd_update(force, check_only)
        }
        Some("run-in") => {
            let tail: Vec<String> = args[1..].iter().map(|s| s.to_string()).collect();
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
  dev doctor [--config F] Check environment and config
  dev update              Check for and apply updates
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
  Per-project layouts are configured in ~/.config/dev/config.toml:

    [defaults]
    layout = \"default\"

    [project.atomicguard]
    layout = \"claude\"
    host = \"myserver\"

    [project.dotfiles]
    layout = \"claude\"
    path = \"~/.local/share/chezmoi\"

  Fields:
  - layout: default, claude
  - path:   optional custom directory (expands ~); omit for ~/Projects projects
  - host:   optional SSH hostname; omit for local projects
"
    );
}

fn parse_layout(s: &str) -> Layout {
    Layout::parse(s)
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
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn forward_remote(host: &str, args: &[&str]) -> Result<()> {
    let remote_cmd = format!(
        "dev {}",
        args.iter()
            .map(|a| sh_quote(a))
            .collect::<Vec<_>>()
            .join(" ")
    );

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = Command::new("ssh").args(["-t", host, &remote_cmd]).exec();
        bail!("ssh to {host}: {err}");
    }
    #[cfg(not(unix))]
    bail!("remote forwarding is not supported on this platform");
}

fn cmd_doctor(config_override: Option<&std::path::Path>) -> Result<()> {
    let c = Colors::new();
    let mut all_ok = true;

    macro_rules! ok {
        ($msg:expr) => {{
            eprintln!("  {}✓{} {}", c.green, c.nc, $msg);
        }};
    }
    macro_rules! fail {
        ($msg:expr) => {{
            eprintln!("  {}✗{} {}", c.red, c.nc, $msg);
            all_ok = false;
        }};
    }

    // tmux
    match Command::new("tmux").arg("-V").output() {
        Ok(o) if o.status.success() => {
            ok!(String::from_utf8_lossy(&o.stdout).trim().to_string());
        }
        _ => fail!("tmux: not found — install via your package manager"),
    }

    // ssh (writes its version to stderr)
    match Command::new("ssh").arg("-V").output() {
        Ok(o) => {
            let ver = String::from_utf8_lossy(&o.stderr).trim().to_string();
            if !ver.is_empty() {
                ok!(ver);
            } else {
                fail!("ssh: not found — install openssh");
            }
        }
        _ => fail!("ssh: not found — install openssh"),
    }

    // daemon socket
    let socket_path = dev_lib::daemon::default_socket_path()?;
    if socket_path.exists() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;
        match UnixStream::connect(&socket_path) {
            Ok(mut s) => {
                // minimal probe: send a request and look for any HTTP response
                let req = "GET /sessions HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
                let _ = s.write_all(req.as_bytes());
                let mut buf = [0u8; 32];
                match s.read(&mut buf) {
                    Ok(n) if n > 0 => ok!(format!(
                        "daemon socket {} responsive",
                        socket_path.display()
                    )),
                    _ => fail!(format!(
                        "daemon socket {} exists but not responsive",
                        socket_path.display()
                    )),
                }
            }
            Err(_) => fail!(format!(
                "daemon socket {} not responsive — run 'dev daemon' or 'bootstrap.sh --host'",
                socket_path.display()
            )),
        }
    } else {
        // Not a failure on client machines — only expected on hosts
        let is_host = Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", "dev-daemon.service"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if is_host {
            fail!(format!(
                "daemon socket missing (service claims active) — restart: systemctl --user restart dev-daemon.service"
            ));
        } else {
            eprintln!("  {}–{} daemon not running (client mode)", c.yellow, c.nc);
        }
    }

    // config
    let config_path = config_override
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(dev_lib::config::config_path);
    if config_path.exists() {
        let warnings = dev_lib::config::validate_config(&config_path);
        if warnings.is_empty() {
            ok!(format!("config {} valid", config_path.display()));
        } else {
            for w in &warnings {
                fail!(w.clone());
            }
        }
    } else if dev_lib::config::legacy_config_path().exists() {
        fail!(format!(
            "legacy config found at {}; migrate it to TOML at {}",
            dev_lib::config::legacy_config_path().display(),
            config_path.display()
        ));
    } else {
        eprintln!(
            "  {}–{} config not found at {} (using defaults)",
            c.yellow,
            c.nc,
            config_path.display()
        );
    }

    // defaults.host reachability
    let config = dev_lib::config::parse_config(&config_path)?;
    if let Some(host) = config.default_host() {
        match Command::new("ssh")
            .args([
                "-o",
                "ConnectTimeout=5",
                "-o",
                "BatchMode=yes",
                host,
                "true",
            ])
            .status()
        {
            Ok(s) if s.success() => ok!(format!("defaults.host={host} reachable")),
            _ => fail!(format!(
                "defaults.host={host} unreachable — check ssh config"
            )),
        }
    }

    // version check (best-effort, no failure on network error)
    let current = env!("DEV_VERSION");
    let channel = if current.contains("-dev.") {
        "dev"
    } else {
        "stable"
    };
    match resolve_latest_version(channel) {
        Ok(latest) if normalize_version(&latest) != normalize_version(current) => {
            eprintln!(
                "  {}–{} update available: {} -> {} (run 'dev update')",
                c.yellow, c.nc, current, latest
            );
        }
        _ => {}
    }

    if !all_ok {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_update(force: bool, check_only: bool) -> Result<()> {
    let current = env!("DEV_VERSION");
    let channel = if current.contains("-dev.") {
        "dev"
    } else {
        "stable"
    };

    let latest = resolve_latest_version(channel)?;

    if normalize_version(&latest) == normalize_version(current) {
        info(&format!("Already up to date ({})", current));
        return Ok(());
    }

    if check_only {
        eprintln!("Update available: {} -> {}", current, latest);
        std::process::exit(1);
    }

    eprintln!("Current: {}", current);
    eprintln!("Latest:  {}", latest);

    if !force {
        let c = Colors::new();
        eprint!("{}Apply update?{} [y/N] ", c.yellow, c.nc);
        io::stderr().flush()?;
        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            info("Cancelled");
            return Ok(());
        }
    }

    apply_update(&latest)?;

    // Restart daemon if it's running as a systemd service on this host
    if Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "dev-daemon.service"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        info("Restarting daemon...");
        Command::new("systemctl")
            .args(["--user", "restart", "dev-daemon.service"])
            .status()?;
    }

    info(&format!("Updated to {}", latest));
    Ok(())
}

/// Fetch the latest release tag for the given channel ("stable" or "dev").
fn resolve_latest_version(channel: &str) -> Result<String> {
    match channel {
        "stable" => {
            let body = github_api_get("releases/latest")?;
            let v: serde_json::Value = serde_json::from_str(&body)?;
            v["tag_name"]
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow::anyhow!("no tag_name in latest release response"))
        }
        _ => {
            // dev channel: find the newest prerelease sorted by created_at
            let body = github_api_get("releases")?;
            let releases: serde_json::Value = serde_json::from_str(&body)?;
            let Some(arr) = releases.as_array() else {
                bail!("unexpected releases response");
            };
            let mut prereleases: Vec<(&str, &str)> = arr
                .iter()
                .filter(|r| r["prerelease"].as_bool().unwrap_or(false))
                .filter_map(|r| {
                    let tag = r["tag_name"].as_str()?;
                    let created = r["created_at"].as_str()?;
                    Some((created, tag))
                })
                .collect();
            prereleases.sort_by(|a, b| b.0.cmp(a.0));
            prereleases
                .first()
                .map(|(_, tag)| tag.to_string())
                .ok_or_else(|| anyhow::anyhow!("no dev pre-release found"))
        }
    }
}

/// Call the GitHub API and return the response body.
fn github_api_get(path: &str) -> Result<String> {
    let url = format!("https://api.github.com/repos/thompsonson/dev/{path}");
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--retry",
            "3",
            "-H",
            "Accept: application/vnd.github.v3+json",
            &url,
        ])
        .output()
        .or_else(|_| {
            Command::new("wget")
                .args([
                    "-qO-",
                    "--header=Accept: application/vnd.github.v3+json",
                    &url,
                ])
                .output()
        })
        .context("need curl or wget to check for updates")?;
    if !out.status.success() {
        bail!("GitHub API request failed for {url}");
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Download bootstrap.sh from GitHub and run it with `--version <tag>`.
fn apply_update(version: &str) -> Result<()> {
    let bootstrap_url =
        "https://raw.githubusercontent.com/thompsonson/dev/main/scripts/bootstrap.sh";

    // Download bootstrap.sh to a temp file
    let tmp = std::env::temp_dir().join("dev-bootstrap.sh");
    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", bootstrap_url, "-o"])
        .arg(&tmp)
        .status()
        .or_else(|_| {
            Command::new("wget")
                .args(["-qO"])
                .arg(&tmp)
                .arg(bootstrap_url)
                .status()
        })
        .context("need curl or wget to download update")?;
    if !status.success() {
        bail!("failed to download bootstrap.sh");
    }

    let status = Command::new("bash")
        .arg(&tmp)
        .args(["--version", version])
        .status()?;
    let _ = std::fs::remove_file(&tmp);
    if !status.success() {
        bail!("update installation failed");
    }
    Ok(())
}

/// Strip a leading `v` for version comparisons.
fn normalize_version(v: &str) -> &str {
    v.strip_prefix('v').unwrap_or(v)
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

    if let Some(err) = resp.get("error").and_then(|v| v.as_str()) {
        bail!("{err}");
    }

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
            "connect to dev daemon at {} (is `dev daemon` running? if sessions are on a remote host, ssh there and run directly)",
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
