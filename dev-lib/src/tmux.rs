use std::path::Path;
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::config::Layout;

/// Result of running a command inside a tmux pane and capturing its output.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CommandOutput {
    pub stdout: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub marker_id: String,
}

/// Information about an active tmux session.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionInfo {
    pub name: String,
    pub pane_count: usize,
    pub attached: bool,
    pub last_activity: u64,
    pub layout: String,
}

/// Trait abstracting tmux operations for testability.
pub trait TmuxBackend {
    fn has_session(&self, name: &str) -> Result<bool>;
    fn list_sessions(&self) -> Result<Vec<SessionInfo>>;
    fn list_panes(&self, session: &str) -> Result<usize>;
    fn create_session(&self, name: &str, path: &Path, layout: &Layout) -> Result<()>;
    fn kill_session(&self, name: &str) -> Result<()>;
    fn kill_server(&self) -> Result<()>;
    fn send_keys(&self, target: &str, keys: &str) -> Result<()>;
    fn split_window_horizontal(&self, session: &str, path: &Path) -> Result<()>;
    fn select_pane(&self, target: &str) -> Result<()>;
    fn session_count(&self) -> Result<usize>;

    /// Run a command inside a tmux pane and synchronously capture stdout + exit code.
    ///
    /// Uses the "marker sandwich" technique: clears the pane history, sends a
    /// unique start marker, sends the command, then sends an end marker that
    /// also captures `$?`. Polls `capture-pane` until the end marker appears
    /// or `timeout` elapses.
    fn run_and_capture(
        &self,
        target: &str,
        command: &str,
        timeout: Duration,
    ) -> Result<CommandOutput>;
}

/// Real tmux implementation via subprocess commands.
pub struct RealTmux;

impl RealTmux {
    fn run(args: &[&str]) -> Result<String> {
        let output = Command::new("tmux")
            .args(args)
            .output()
            .context("Failed to run tmux")?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            bail!("tmux {} failed: {}", args.first().unwrap_or(&""), stderr);
        }
    }

    fn run_ok(args: &[&str]) -> Result<bool> {
        let output = Command::new("tmux")
            .args(args)
            .output()
            .context("Failed to run tmux")?;
        Ok(output.status.success())
    }
}

impl TmuxBackend for RealTmux {
    fn has_session(&self, name: &str) -> Result<bool> {
        let target = format!("={name}");
        Self::run_ok(&["has-session", "-t", &target])
    }

    fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let output = Command::new("tmux")
            .args([
                "list-sessions",
                "-F",
                "#{session_name}|#{session_windows}|#{session_attached}|#{session_activity}",
            ])
            .output();

        let output = match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            // No server running or no sessions
            _ => return Ok(Vec::new()),
        };

        if output.is_empty() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();
        for line in output.lines() {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() < 4 {
                continue;
            }
            let name = parts[0].to_string();
            let attached = parts[2].parse::<u32>().unwrap_or(0) > 0;
            let last_activity = parts[3].parse::<u64>().unwrap_or(0);

            let pane_count = self.list_panes(&name).unwrap_or(1);
            let layout = if pane_count >= 2 {
                "claude".to_string()
            } else {
                "default".to_string()
            };

            sessions.push(SessionInfo {
                name,
                pane_count,
                attached,
                last_activity,
                layout,
            });
        }

        Ok(sessions)
    }

    fn list_panes(&self, session: &str) -> Result<usize> {
        let target = format!("{session}:1");
        let output = Self::run(&["list-panes", "-t", &target, "-F", "#{pane_index}"])?;
        Ok(output.lines().count())
    }

    fn create_session(&self, name: &str, path: &Path, layout: &Layout) -> Result<()> {
        let path_str = path.to_string_lossy();

        // Create detached session
        Self::run(&["new-session", "-d", "-s", name, "-c", &path_str])?;

        if *layout == Layout::Claude {
            // Split vertically, run claude in left pane, focus right pane
            Self::run(&["split-window", "-h", "-t", name, "-c", &path_str])?;
            let left = format!("{name}:1.1");
            Self::run(&["select-pane", "-t", &left])?;
            Self::run(&["send-keys", "-t", &left, "claude", "Enter"])?;
            let right = format!("{name}:1.2");
            Self::run(&["select-pane", "-t", &right])?;
        }

        Ok(())
    }

    fn kill_session(&self, name: &str) -> Result<()> {
        let target = format!("={name}");
        Self::run(&["kill-session", "-t", &target])?;
        Ok(())
    }

    fn kill_server(&self) -> Result<()> {
        Self::run(&["kill-server"])?;
        Ok(())
    }

    fn send_keys(&self, target: &str, keys: &str) -> Result<()> {
        Self::run(&["send-keys", "-t", target, keys, "Enter"])?;
        Ok(())
    }

    fn split_window_horizontal(&self, _session: &str, path: &Path) -> Result<()> {
        let path_str = path.to_string_lossy();
        Self::run(&["split-window", "-hb", "-c", &path_str])?;
        Ok(())
    }

    fn select_pane(&self, target: &str) -> Result<()> {
        Self::run(&["select-pane", "-t", target])?;
        Ok(())
    }

    fn session_count(&self) -> Result<usize> {
        let output = Command::new("tmux").args(["list-sessions"]).output();

        match output {
            Ok(o) if o.status.success() => {
                Ok(String::from_utf8_lossy(&o.stdout).trim().lines().count())
            }
            _ => Ok(0),
        }
    }

    fn run_and_capture(
        &self,
        target: &str,
        command: &str,
        timeout: Duration,
    ) -> Result<CommandOutput> {
        // Short UUID — 12 hex chars is plenty for uniqueness inside a single pane.
        let marker_id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let start_marker = format!("START_{marker_id}");
        let end_prefix = format!("END_{marker_id}");
        let end_cmd = format!("echo \"{end_prefix} $?\"");

        Self::run(&["clear-history", "-t", target])?;
        Self::run(&["send-keys", "-t", target, &format!("echo {start_marker}"), "Enter"])?;
        Self::run(&["send-keys", "-t", target, command, "Enter"])?;
        Self::run(&["send-keys", "-t", target, &end_cmd, "Enter"])?;

        let started = Instant::now();
        loop {
            let capture = Self::run(&["capture-pane", "-p", "-S", "-", "-t", target])?;

            if let Some((stdout, exit_code)) = parse_captured(&capture, &start_marker, &end_prefix)
            {
                return Ok(CommandOutput {
                    stdout,
                    exit_code: Some(exit_code),
                    duration_ms: started.elapsed().as_millis(),
                    marker_id,
                });
            }

            if started.elapsed() >= timeout {
                bail!(
                    "run_and_capture timed out after {:?} waiting for marker {}",
                    timeout,
                    end_prefix
                );
            }

            sleep(Duration::from_millis(100));
        }
    }
}

/// Slice a tmux `capture-pane` output between the start/end markers and parse
/// the exit code from the end-marker line. Returns `None` if the end marker has
/// not been emitted yet.
///
/// The end-marker line must match exactly `"{end_prefix} <digits>"` — this
/// prevents a false early match if the command itself echoes `end_prefix`
/// somewhere in its output (the extra ` <digits>` suffix disambiguates).
fn parse_captured(capture: &str, start_marker: &str, end_prefix: &str) -> Option<(String, i32)> {
    let lines: Vec<&str> = capture.lines().collect();

    // Find the last line that looks like "END_<uuid> <digits>" — scanning from
    // the bottom so we pick up the most recent completion.
    let mut end_idx = None;
    let mut exit_code = 0i32;
    for (i, line) in lines.iter().enumerate().rev() {
        let trimmed = line.trim_end();
        if let Some(rest) = trimmed.strip_prefix(end_prefix) {
            let rest = rest.trim_start();
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '-') {
                if let Ok(n) = rest.parse::<i32>() {
                    end_idx = Some(i);
                    exit_code = n;
                    break;
                }
            }
        }
    }
    let end_idx = end_idx?;

    // Find the matching start marker above the end marker. We look for a line
    // whose trailing content equals `start_marker` — tmux may prefix with the
    // shell prompt echo, so an exact-line match is too strict.
    let start_idx = lines[..end_idx]
        .iter()
        .rposition(|line| line.trim_end().ends_with(start_marker))?;

    // Command output lives strictly between start and end. The line right
    // after START is typically the command echo itself, which we keep — post-
    // guards can strip it if needed; the raw stream is more useful for debug.
    let stdout = lines[start_idx + 1..end_idx].join("\n");
    Some((stdout, exit_code))
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::cell::RefCell;

    /// Mock tmux backend for testing.
    pub struct MockTmux {
        pub sessions: RefCell<Vec<SessionInfo>>,
        pub created: RefCell<Vec<(String, String, String)>>, // name, path, layout
        pub killed: RefCell<Vec<String>>,
        pub run_calls: RefCell<Vec<(String, String)>>, // target, command
        pub run_stdout: RefCell<String>,
        pub run_exit_code: RefCell<i32>,
    }

    impl MockTmux {
        pub fn new() -> Self {
            Self {
                sessions: RefCell::new(Vec::new()),
                created: RefCell::new(Vec::new()),
                killed: RefCell::new(Vec::new()),
                run_calls: RefCell::new(Vec::new()),
                run_stdout: RefCell::new(String::new()),
                run_exit_code: RefCell::new(0),
            }
        }

        pub fn with_sessions(sessions: Vec<SessionInfo>) -> Self {
            let mock = Self::new();
            *mock.sessions.borrow_mut() = sessions;
            mock
        }
    }

    impl TmuxBackend for MockTmux {
        fn has_session(&self, name: &str) -> Result<bool> {
            Ok(self.sessions.borrow().iter().any(|s| s.name == name))
        }

        fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
            Ok(self.sessions.borrow().clone())
        }

        fn list_panes(&self, session: &str) -> Result<usize> {
            Ok(self
                .sessions
                .borrow()
                .iter()
                .find(|s| s.name == session)
                .map(|s| s.pane_count)
                .unwrap_or(1))
        }

        fn create_session(&self, name: &str, path: &Path, layout: &Layout) -> Result<()> {
            let pane_count = if *layout == Layout::Claude { 2 } else { 1 };
            self.sessions.borrow_mut().push(SessionInfo {
                name: name.to_string(),
                pane_count,
                attached: false,
                last_activity: 0,
                layout: layout.to_string(),
            });
            self.created.borrow_mut().push((
                name.to_string(),
                path.to_string_lossy().to_string(),
                layout.to_string(),
            ));
            Ok(())
        }

        fn kill_session(&self, name: &str) -> Result<()> {
            self.sessions.borrow_mut().retain(|s| s.name != name);
            self.killed.borrow_mut().push(name.to_string());
            Ok(())
        }

        fn kill_server(&self) -> Result<()> {
            self.sessions.borrow_mut().clear();
            Ok(())
        }

        fn send_keys(&self, _target: &str, _keys: &str) -> Result<()> {
            Ok(())
        }

        fn split_window_horizontal(&self, _session: &str, _path: &Path) -> Result<()> {
            Ok(())
        }

        fn select_pane(&self, _target: &str) -> Result<()> {
            Ok(())
        }

        fn session_count(&self) -> Result<usize> {
            Ok(self.sessions.borrow().len())
        }

        fn run_and_capture(
            &self,
            target: &str,
            command: &str,
            _timeout: Duration,
        ) -> Result<CommandOutput> {
            self.run_calls
                .borrow_mut()
                .push((target.to_string(), command.to_string()));
            Ok(CommandOutput {
                stdout: self.run_stdout.borrow().clone(),
                exit_code: Some(*self.run_exit_code.borrow()),
                duration_ms: 1,
                marker_id: "mock-marker".to_string(),
            })
        }
    }
}

#[cfg(test)]
mod parse_tests {
    use super::parse_captured;

    #[test]
    fn parses_stdout_and_exit_code() {
        let capture = "\
$ echo START_abc123
START_abc123
$ echo hello
hello
$ echo \"END_abc123 $?\"
END_abc123 0
";
        let (stdout, code) = parse_captured(capture, "START_abc123", "END_abc123").unwrap();
        assert_eq!(code, 0);
        assert!(stdout.contains("hello"));
        // The literal end-marker line ("END_abc123 0") must be excluded, but a
        // command-echo line containing the marker text (e.g. `$ echo "END_abc123 $?"`)
        // stays in — it's real pane output between the markers.
        assert!(!stdout.lines().any(|l| l.trim_end() == "END_abc123 0"));
        assert!(!stdout.lines().any(|l| l.trim_end() == "START_abc123"));
    }

    #[test]
    fn captures_non_zero_exit() {
        let capture = "START_xyz\nboom\nEND_xyz 1\n";
        let (_, code) = parse_captured(capture, "START_xyz", "END_xyz").unwrap();
        assert_eq!(code, 1);
    }

    #[test]
    fn returns_none_when_end_marker_absent() {
        let capture = "START_xyz\nstill running\n";
        assert!(parse_captured(capture, "START_xyz", "END_xyz").is_none());
    }

    #[test]
    fn ignores_end_prefix_without_digit_suffix() {
        // Command echoes the marker text but without the "<space><digits>" tail —
        // must not be mistaken for the real end marker.
        let capture = "\
START_xyz
user said END_xyz which is cool
END_xyz 0
";
        let (stdout, code) = parse_captured(capture, "START_xyz", "END_xyz").unwrap();
        assert_eq!(code, 0);
        assert!(stdout.contains("user said END_xyz which is cool"));
    }

    #[test]
    fn picks_most_recent_end_marker() {
        let capture = "\
START_xyz
first
END_xyz 0
noise
START_xyz
second
END_xyz 7
";
        let (stdout, code) = parse_captured(capture, "START_xyz", "END_xyz").unwrap();
        assert_eq!(code, 7);
        assert!(stdout.contains("second"));
        assert!(!stdout.contains("first"));
    }
}
