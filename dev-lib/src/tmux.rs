use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use tmux_interface::{
    DisplayMessage, HasSession, KillServer, KillSession, ListPanes, ListSessions, NewSession,
    RunShell, SelectPane, SendKeys, SplitWindow, Tmux, TmuxOutput,
};

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
    pub host: String,
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
    /// Send keys to a pane. When `enter` is true, an Enter keystroke is
    /// appended — suitable for running a command. When false, the keys are
    /// sent verbatim — suitable for special keys or partial input.
    fn send_keys(&self, target: &str, keys: &str, enter: bool) -> Result<()>;
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

/// Real tmux implementation using the [`tmux_interface`] typed command builders.
///
/// Every call goes through `Tmux::with_command(...)` or `Tmux::new().add_command(...)`
/// which invoke the `tmux` binary once per batch — no direct `std::process::Command`
/// in this module. Multi-step operations (`create_session` with a claude layout,
/// `run_and_capture`'s marker setup) are batched into a single tmux invocation.
pub struct RealTmux;

impl RealTmux {
    /// Run a single tmux command and bail on non-zero exit with the stderr.
    fn run_one<'a, T>(cmd: T) -> Result<TmuxOutput>
    where
        T: Into<tmux_interface::TmuxCommand<'a>>,
    {
        let out = Tmux::with_command(cmd)
            .output()
            .context("Failed to run tmux")?;
        if !out.success() {
            let stderr = String::from_utf8_lossy(&out.0.stderr).trim().to_string();
            bail!("tmux command failed: {stderr}");
        }
        Ok(out)
    }

    fn stdout_trimmed(out: &TmuxOutput) -> String {
        String::from_utf8_lossy(&out.0.stdout).trim().to_string()
    }
}

impl TmuxBackend for RealTmux {
    fn has_session(&self, name: &str) -> Result<bool> {
        let target = format!("={name}");
        let status = Tmux::with_command(HasSession::new().target_session(target))
            .status()
            .context("Failed to run tmux")?;
        Ok(status.success())
    }

    fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        // Swallow errors (no server running, no sessions) into an empty list
        // — matches the original shell-out semantics.
        let out =
            Tmux::with_command(ListSessions::new().format(
                "#{session_name}|#{session_windows}|#{session_attached}|#{session_activity}",
            ))
            .output();
        let text = match out {
            Ok(o) if o.success() => Self::stdout_trimmed(&o),
            _ => return Ok(Vec::new()),
        };
        if text.is_empty() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();
        for line in text.lines() {
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
                host: String::new(),
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
        let out = Self::run_one(ListPanes::new().target(target).format("#{pane_index}"))?;
        Ok(Self::stdout_trimmed(&out).lines().count())
    }

    fn create_session(&self, name: &str, path: &Path, layout: &Layout) -> Result<()> {
        let path_str = path.to_string_lossy().into_owned();

        // Create detached session (standalone — its success is a prereq for
        // the layout batch below, so we don't combine them).
        Self::run_one(
            NewSession::new()
                .detached()
                .session_name(name.to_string())
                .start_directory(path_str.clone()),
        )?;

        if *layout == Layout::Claude {
            // Split → select left → type `claude` → select right, all in one
            // tmux invocation (four add_commands, one fork+exec).
            let left = format!("{name}:1.1");
            let right = format!("{name}:1.2");
            let out = Tmux::new()
                .add_command(
                    SplitWindow::new()
                        .horizontal()
                        .target_pane(name.to_string())
                        .start_directory(path_str),
                )
                .add_command(SelectPane::new().target_pane(left.clone()))
                .add_command(SendKeys::new().target_pane(left).key("claude").key("Enter"))
                .add_command(SelectPane::new().target_pane(right))
                .output()
                .context("Failed to run tmux claude layout batch")?;
            if !out.success() {
                bail!(
                    "claude layout setup failed: {}",
                    String::from_utf8_lossy(&out.0.stderr).trim()
                );
            }
        }

        Ok(())
    }

    fn kill_session(&self, name: &str) -> Result<()> {
        let target = format!("={name}");
        Self::run_one(KillSession::new().target_session(target))?;
        Ok(())
    }

    fn kill_server(&self) -> Result<()> {
        Self::run_one(KillServer::new())?;
        Ok(())
    }

    fn send_keys(&self, target: &str, keys: &str, enter: bool) -> Result<()> {
        let mut cmd = SendKeys::new()
            .target_pane(target.to_string())
            .key(keys.to_string());
        if enter {
            cmd = cmd.key("Enter");
        }
        Self::run_one(cmd)?;
        Ok(())
    }

    fn split_window_horizontal(&self, _session: &str, path: &Path) -> Result<()> {
        let path_str = path.to_string_lossy().into_owned();
        Self::run_one(
            SplitWindow::new()
                .horizontal()
                .before()
                .start_directory(path_str),
        )?;
        Ok(())
    }

    fn select_pane(&self, target: &str) -> Result<()> {
        Self::run_one(SelectPane::new().target_pane(target.to_string()))?;
        Ok(())
    }

    fn session_count(&self) -> Result<usize> {
        let out = Tmux::with_command(ListSessions::new()).output();
        match out {
            Ok(o) if o.success() => Ok(Self::stdout_trimmed(&o).lines().count()),
            _ => Ok(0),
        }
    }

    fn run_and_capture(
        &self,
        target: &str,
        command: &str,
        timeout: Duration,
    ) -> Result<CommandOutput> {
        // Short UUID kept as the opaque identifier we return (history tracking)
        // — the marker-sandwich parsing it used to drive is gone.
        let marker_id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let target_owned = target.to_string();

        // Staging files: stdout, stderr, exit code. The daemon owns this cache
        // dir and garbage-collects stale entries on its next run.
        let cache = runs_cache_dir()?;
        let stdout_path = cache.join(format!("{marker_id}.out"));
        let stderr_path = cache.join(format!("{marker_id}.err"));
        let exit_path = cache.join(format!("{marker_id}.exit"));
        // Best-effort clean slate.
        let _ = std::fs::remove_file(&stdout_path);
        let _ = std::fs::remove_file(&stderr_path);
        let _ = std::fs::remove_file(&exit_path);

        // Query the pane's current working directory so commands run in the
        // same place the user's interactive shell is sitting — important for
        // things like `cargo test` that depend on being inside a crate.
        let cwd_out = Self::run_one(
            DisplayMessage::new()
                .print()
                .target_pane(target_owned.clone())
                .message("#{pane_current_path}"),
        )?;
        let pane_cwd = Self::stdout_trimmed(&cwd_out);

        // Build a /bin/sh one-liner:
        //   cd '<cwd>' && ( <cmd> ) > '<out>' 2> '<err>'; echo $? > '<exit>'
        //
        // `( ... )` subshell means the command's own stdout/stderr go into
        // the staging files instead of the pane — the pane sees nothing at
        // all because we dispatch via `tmux run-shell -b`, not `send-keys`.
        //
        // Semantic change from the previous marker-sandwich implementation:
        // commands now run in a /bin/sh subshell under tmux server's
        // environment, not in the pane's interactive shell. cd/export/alias
        // changes do NOT propagate to the pane. This is the right trade-off
        // for agent-driven command execution (AtomicGuard effectors, chops
        // web-ui run buttons); if you want commands to mutate the pane's
        // shell state, use `send_keys` directly.
        let wrapped = format!(
            "cd {cwd} && ( {cmd} ) > {out} 2> {err}; echo $? > {exit}",
            cwd = sh_single_quote(&pane_cwd),
            cmd = command,
            out = sh_single_quote(&stdout_path.to_string_lossy()),
            err = sh_single_quote(&stderr_path.to_string_lossy()),
            exit = sh_single_quote(&exit_path.to_string_lossy()),
        );

        // `run-shell -b` dispatches the command to tmux server's /bin/sh and
        // returns immediately. The pane is completely untouched.
        Self::run_one(RunShell::new().background().shell_command(wrapped))
            .context("failed to dispatch run-shell")?;

        // Poll for the exit file. Its existence is the completion signal —
        // no pane scraping, no marker parsing.
        let started = Instant::now();
        let poll = Duration::from_millis(50);
        loop {
            if exit_path.exists() {
                let stdout = std::fs::read_to_string(&stdout_path).unwrap_or_default();
                let exit_raw = std::fs::read_to_string(&exit_path).unwrap_or_default();
                let exit_code = exit_raw.trim().parse::<i32>().ok();
                // Best-effort cleanup.
                let _ = std::fs::remove_file(&stdout_path);
                let _ = std::fs::remove_file(&stderr_path);
                let _ = std::fs::remove_file(&exit_path);
                return Ok(CommandOutput {
                    stdout,
                    exit_code,
                    duration_ms: started.elapsed().as_millis(),
                    marker_id,
                });
            }
            if started.elapsed() >= timeout {
                // Cleanup staging files even on timeout. The orphaned
                // background command will keep running; that's a deliberate
                // choice matching the previous behaviour — we don't try to
                // kill it because we don't know what it was doing.
                let _ = std::fs::remove_file(&stdout_path);
                let _ = std::fs::remove_file(&stderr_path);
                let _ = std::fs::remove_file(&exit_path);
                bail!("run_and_capture timed out after {:?}", timeout);
            }
            sleep(poll);
        }
    }
}

/// Per-daemon cache directory for `run_and_capture` staging files. Lives under
/// `$XDG_CACHE_HOME/dev-daemon/runs` (fallback `~/.cache/...`).
fn runs_cache_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))
        .context("no cache dir available (neither XDG_CACHE_HOME nor HOME is set)")?;
    let dir = base.join("dev-daemon").join("runs");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating cache dir {}", dir.display()))?;
    Ok(dir)
}

/// POSIX single-quote escape a string for safe embedding in a `/bin/sh`
/// command. Wraps in `'...'` and replaces internal `'` with `'\''`.
fn sh_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod sh_quote_tests {
    use super::sh_single_quote;

    #[test]
    fn plain_string_wrapped_in_quotes() {
        assert_eq!(sh_single_quote("hello"), "'hello'");
    }

    #[test]
    fn path_with_spaces() {
        assert_eq!(
            sh_single_quote("/tmp/path with space/foo.txt"),
            "'/tmp/path with space/foo.txt'"
        );
    }

    #[test]
    fn embedded_single_quote_escaped() {
        // POSIX idiom: close quote, literal-escaped quote, reopen quote.
        assert_eq!(sh_single_quote("it's"), r#"'it'\''s'"#);
    }

    #[test]
    fn empty_string_still_quoted() {
        assert_eq!(sh_single_quote(""), "''");
    }

    #[test]
    fn multiple_single_quotes() {
        assert_eq!(sh_single_quote("a'b'c"), r#"'a'\''b'\''c'"#);
    }
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

    impl Default for MockTmux {
        fn default() -> Self {
            Self::new()
        }
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
                host: String::new(),
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

        fn send_keys(&self, _target: &str, _keys: &str, _enter: bool) -> Result<()> {
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
