use std::path::PathBuf;

use anyhow::Result;

use crate::config::{self, DevConfig, Layout};
use crate::discovery::{self, DiscoveredProject};
use crate::error::DevError;
use crate::resolve;
use crate::tmux::{RealTmux, SessionInfo, TmuxBackend};

/// Where a project's session runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Local,
    Remote(String),
}

/// JSON-serializable output for the `list` command.
#[derive(Debug, serde::Serialize)]
pub struct ListOutput {
    pub sessions: Vec<SessionInfo>,
    pub projects: Vec<ProjectInfo>,
}

/// A project available to start (not currently an active session).
#[derive(Debug, serde::Serialize)]
pub struct ProjectInfo {
    pub name: String,
    pub path: String,
    pub layout: String,
    pub host: Option<String>,
}

/// Result of resolving and preparing to open a project.
pub struct OpenResult {
    /// The session name to attach to.
    pub session_name: String,
    /// Whether the session was newly created (false = already existed).
    pub created: bool,
    /// Remote host if the project should be forwarded via SSH.
    pub remote_host: Option<String>,
}

/// High-level session manager.
pub struct DevManager {
    config: DevConfig,
    projects: Vec<DiscoveredProject>,
    projects_dir: PathBuf,
    tmux: Box<dyn TmuxBackend>,
    local_hostname: String,
}

impl DevManager {
    /// Create a new DevManager with real tmux backend.
    pub fn new() -> Result<Self> {
        Self::with_backend(Box::new(RealTmux))
    }

    /// Create a DevManager with a custom backend (for testing).
    pub fn with_backend(tmux: Box<dyn TmuxBackend>) -> Result<Self> {
        let config = config::parse_config(&config::config_path())?;
        let projects_dir = dirs::home_dir()
            .map(|h| h.join("Projects"))
            .unwrap_or_default();
        let projects = discovery::discover_projects(&projects_dir, &config);
        let local_hostname = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_default();
        Ok(Self {
            config,
            projects,
            projects_dir,
            tmux,
            local_hostname,
        })
    }

    /// List active sessions and available projects.
    pub fn list(&self) -> Result<ListOutput> {
        let sessions: Vec<_> = self
            .tmux
            .list_sessions()?
            .into_iter()
            .map(|mut s| {
                s.host = self.local_hostname.clone();
                s
            })
            .collect();
        let session_names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();

        let mut projects: Vec<ProjectInfo> = Vec::new();

        // Add discovered projects that don't have active sessions
        for p in &self.projects {
            let basename = p
                .full_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&p.display_name);

            if !session_names.contains(&basename)
                && !session_names.contains(&p.display_name.as_str())
            {
                let layout = self
                    .config
                    .projects
                    .get(&p.display_name)
                    .or_else(|| self.config.projects.get(basename))
                    .map(|e| e.layout.to_string())
                    .unwrap_or_else(|| self.config.default_layout.to_string());

                let host = match self.resolve_target(&p.display_name) {
                    Target::Remote(h) => Some(h),
                    Target::Local => None,
                };

                projects.push(ProjectInfo {
                    name: p.display_name.clone(),
                    path: p.full_path.to_string_lossy().to_string(),
                    layout,
                    host,
                });
            }
        }

        // Add config-only remote projects not found locally and without sessions.
        for (key, entry) in &self.config.projects {
            if let Some(ref host) = entry.host {
                if host != &self.local_hostname
                    && !session_names.contains(&key.as_str())
                    && !projects.iter().any(|p| p.name == *key)
                {
                    projects.push(ProjectInfo {
                        name: key.clone(),
                        path: String::new(),
                        layout: entry.layout.to_string(),
                        host: Some(host.clone()),
                    });
                }
            }
        }

        Ok(ListOutput { sessions, projects })
    }

    /// Start a session for a project without attaching.
    pub fn start(&self, project: &str, layout: Option<Layout>) -> Result<String> {
        // Check if session already exists
        if self.tmux.has_session(project)? {
            return Ok(project.to_string());
        }

        // Resolve project path
        let discovered = resolve::resolve_project(project, &self.projects);
        let (session_name, project_path) = match discovered {
            Some(p) => {
                let basename = p
                    .full_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&p.display_name)
                    .to_string();
                (basename, p.full_path.clone())
            }
            None => return Err(DevError::ProjectNotFound(project.to_string()).into()),
        };

        // Check if session exists under resolved name
        if self.tmux.has_session(&session_name)? {
            return Ok(session_name);
        }

        let layout = layout.unwrap_or_else(|| {
            self.config
                .projects
                .get(project)
                .or_else(|| self.config.projects.get(&session_name))
                .map(|e| e.layout.clone())
                .unwrap_or_else(|| self.config.default_layout.clone())
        });

        self.tmux
            .create_session(&session_name, &project_path, &layout)?;
        Ok(session_name)
    }

    /// Stop (kill) a session by name.
    pub fn stop(&self, session: &str) -> Result<()> {
        if self.tmux.has_session(session)? {
            self.tmux.kill_session(session)?;
        }
        Ok(())
    }

    /// Open a project: resolve, create if needed, return info for CLI to attach.
    pub fn open(&self, query: &str, force_layout: Option<Layout>) -> Result<OpenResult> {
        // Check for remote forwarding (per-project host or default_host fallback).
        if let Target::Remote(host) = self.resolve_target(query) {
            return Ok(OpenResult {
                session_name: query.to_string(),
                created: false,
                remote_host: Some(host),
            });
        }

        // Check if session already exists under the query name
        if self.tmux.has_session(query)? {
            return Ok(OpenResult {
                session_name: query.to_string(),
                created: false,
                remote_host: None,
            });
        }

        // Resolve project
        let discovered = resolve::resolve_project(query, &self.projects);
        let (session_name, project_path) = match discovered {
            Some(p) => {
                // Use config key for custom-path projects, basename otherwise
                let name = if p.full_path != self.projects_dir.join(&p.display_name) {
                    // Check if this is a custom-path project (key in config)
                    if self.config.projects.contains_key(&p.display_name)
                        && self.config.projects[&p.display_name].custom_path.is_some()
                    {
                        p.display_name.clone()
                    } else {
                        p.full_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(&p.display_name)
                            .to_string()
                    }
                } else {
                    p.full_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&p.display_name)
                        .to_string()
                };
                (name, p.full_path.clone())
            }
            None => return Err(DevError::ProjectNotFound(query.to_string()).into()),
        };

        // Check remote for resolved name too (already checked query above;
        // this handles the case where the resolved basename differs).
        if session_name != query {
            if let Target::Remote(host) = self.resolve_target(&session_name) {
                return Ok(OpenResult {
                    session_name: session_name.clone(),
                    created: false,
                    remote_host: Some(host),
                });
            }
        }

        // Check if session exists under resolved name
        if self.tmux.has_session(&session_name)? {
            return Ok(OpenResult {
                session_name,
                created: false,
                remote_host: None,
            });
        }

        let layout = force_layout.unwrap_or_else(|| {
            self.config
                .projects
                .get(query)
                .or_else(|| self.config.projects.get(&session_name))
                .map(|e| e.layout.clone())
                .unwrap_or_else(|| self.config.default_layout.clone())
        });

        self.tmux
            .create_session(&session_name, &project_path, &layout)?;

        Ok(OpenResult {
            session_name,
            created: true,
            remote_host: None,
        })
    }

    /// Kill all sessions. Returns the count killed.
    pub fn kill_all(&self) -> Result<usize> {
        let count = self.tmux.session_count()?;
        if count > 0 {
            self.tmux.kill_server()?;
        }
        Ok(count)
    }

    /// Resolve where a project's session runs: locally or on a remote host.
    /// Per-project `@host` takes precedence over `default_host`. Returns
    /// `Target::Local` when the resolved host matches the local machine.
    pub fn resolve_target(&self, project: &str) -> Target {
        let host = self
            .config
            .projects
            .get(project)
            .and_then(|e| e.host.clone())
            .or_else(|| self.config.default_host.clone());
        match host {
            Some(h) if h != self.local_hostname => Target::Remote(h),
            _ => Target::Local,
        }
    }

    /// Returns the host that global commands (list, picker, kill-all) should
    /// be forwarded to. `Some(host)` when `default_host` is configured and
    /// resolves to a different machine; `None` on the host itself.
    pub fn remote_host(&self) -> Option<String> {
        let host = self.config.default_host.clone()?;
        if host == self.local_hostname {
            None
        } else {
            Some(host)
        }
    }

    /// Get the list of discovered projects (for CLI picker).
    pub fn discovered_projects(&self) -> &[DiscoveredProject] {
        &self.projects
    }

    /// Get config reference.
    pub fn config(&self) -> &DevConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::mock::MockTmux;

    // Note: These tests use MockTmux and don't touch the filesystem for tmux,
    // but DevManager::new() reads config and discovers projects from real paths.
    // For isolated tests, we'd need to inject config + projects too.
    // For now, test the logic that doesn't depend on filesystem state.

    #[test]
    fn stop_nonexistent_session_is_ok() {
        let mock = MockTmux::new();
        let mgr = DevManager {
            config: DevConfig::default(),
            projects: Vec::new(),
            projects_dir: PathBuf::from("/tmp/nonexistent"),
            tmux: Box::new(mock),
            local_hostname: String::new(),
        };
        // Should not error
        mgr.stop("nonexistent").unwrap();
    }

    #[test]
    fn stop_existing_session() {
        let mock = MockTmux::with_sessions(vec![SessionInfo {
            name: "myproject".to_string(),
            host: String::new(),
            pane_count: 1,
            attached: false,
            last_activity: 0,
            layout: "default".to_string(),
        }]);
        let mgr = DevManager {
            config: DevConfig::default(),
            projects: Vec::new(),
            projects_dir: PathBuf::from("/tmp/nonexistent"),
            tmux: Box::new(mock),
            local_hostname: String::new(),
        };
        mgr.stop("myproject").unwrap();
    }

    #[test]
    fn list_returns_sessions() {
        let mock = MockTmux::with_sessions(vec![SessionInfo {
            name: "chops".to_string(),
            host: String::new(),
            pane_count: 2,
            attached: true,
            last_activity: 1000,
            layout: "claude".to_string(),
        }]);
        let mgr = DevManager {
            config: DevConfig::default(),
            projects: Vec::new(),
            projects_dir: PathBuf::from("/tmp/nonexistent"),
            tmux: Box::new(mock),
            local_hostname: String::new(),
        };
        let output = mgr.list().unwrap();
        assert_eq!(output.sessions.len(), 1);
        assert_eq!(output.sessions[0].name, "chops");
    }

    #[test]
    fn kill_all_empty() {
        let mock = MockTmux::new();
        let mgr = DevManager {
            config: DevConfig::default(),
            projects: Vec::new(),
            projects_dir: PathBuf::from("/tmp/nonexistent"),
            tmux: Box::new(mock),
            local_hostname: String::new(),
        };
        assert_eq!(mgr.kill_all().unwrap(), 0);
    }

    #[test]
    fn resolve_target_explicit_host_is_remote() {
        use crate::config::{DevConfig, Layout, ProjectEntry};
        let mut config = DevConfig::default();
        config.projects.insert(
            "myproject".to_string(),
            ProjectEntry {
                layout: Layout::Default,
                custom_path: None,
                host: Some("remotehost".to_string()),
            },
        );
        let mgr = DevManager {
            config,
            projects: Vec::new(),
            projects_dir: PathBuf::from("/tmp"),
            tmux: Box::new(MockTmux::new()),
            local_hostname: "localhost".to_string(),
        };
        assert_eq!(
            mgr.resolve_target("myproject"),
            Target::Remote("remotehost".to_string())
        );
    }

    #[test]
    fn resolve_target_local_host_is_local() {
        use crate::config::{DevConfig, Layout, ProjectEntry};
        let mut config = DevConfig::default();
        config.projects.insert(
            "myproject".to_string(),
            ProjectEntry {
                layout: Layout::Default,
                custom_path: None,
                host: Some("thisbox".to_string()),
            },
        );
        let mgr = DevManager {
            config,
            projects: Vec::new(),
            projects_dir: PathBuf::from("/tmp"),
            tmux: Box::new(MockTmux::new()),
            local_hostname: "thisbox".to_string(),
        };
        assert_eq!(mgr.resolve_target("myproject"), Target::Local);
    }

    #[test]
    fn resolve_target_default_host_fallback() {
        let config = DevConfig {
            default_host: Some("pop-mini".to_string()),
            ..DevConfig::default()
        };
        let mgr = DevManager {
            config,
            projects: Vec::new(),
            projects_dir: PathBuf::from("/tmp"),
            tmux: Box::new(MockTmux::new()),
            local_hostname: "phone".to_string(),
        };
        assert_eq!(
            mgr.resolve_target("anything"),
            Target::Remote("pop-mini".to_string())
        );
    }

    #[test]
    fn resolve_target_default_host_local_is_local() {
        let config = DevConfig {
            default_host: Some("pop-mini".to_string()),
            ..DevConfig::default()
        };
        let mgr = DevManager {
            config,
            projects: Vec::new(),
            projects_dir: PathBuf::from("/tmp"),
            tmux: Box::new(MockTmux::new()),
            local_hostname: "pop-mini".to_string(),
        };
        assert_eq!(mgr.resolve_target("anything"), Target::Local);
    }
}
