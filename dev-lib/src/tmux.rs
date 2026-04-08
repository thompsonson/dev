use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::Layout;

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
    }

    impl MockTmux {
        pub fn new() -> Self {
            Self {
                sessions: RefCell::new(Vec::new()),
                created: RefCell::new(Vec::new()),
                killed: RefCell::new(Vec::new()),
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
    }
}
