use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

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
    pub repository: Option<String>,
    pub responsibility: String,
}

#[derive(Debug, Clone)]
struct ProjectMetadata {
    path: Option<PathBuf>,
    repository: Option<String>,
    responsibility: String,
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
            .context("HOME directory not set")?;
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
                let metadata = self.session_metadata(&s.name);
                s.host = self.local_hostname.clone();
                s.project_path = metadata
                    .path
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string());
                s.repository = metadata.repository;
                s.responsibility = metadata.responsibility;
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
                let project_config = self
                    .config
                    .effective_project_config_with_fallback(&p.display_name, basename);
                let layout = project_config.layout.to_string();
                let repository = normalize_repository(
                    project_config
                        .repository
                        .or_else(|| git_origin_repository(&p.full_path)),
                );
                let responsibility = project_config
                    .responsibility
                    .unwrap_or_else(|| default_responsibility(&p.display_name));

                let host = match self.resolve_target(&p.display_name) {
                    Target::Remote(h) => Some(h),
                    Target::Local => None,
                };

                projects.push(ProjectInfo {
                    name: p.display_name.clone(),
                    path: p.full_path.to_string_lossy().to_string(),
                    layout,
                    host,
                    repository,
                    responsibility,
                });
            }
        }

        // Add config-only remote projects not found locally and without sessions.
        for (key, entry) in self.config.projects() {
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
                        repository: normalize_repository(entry.repository.clone()),
                        responsibility: entry
                            .responsibility
                            .clone()
                            .unwrap_or_else(|| default_responsibility(key)),
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
                .effective_project_config_with_fallback(project, &session_name)
                .layout
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
        // Check for remote forwarding (per-project host or defaults.host fallback).
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
                    if self
                        .config
                        .project(&p.display_name)
                        .and_then(|entry| entry.custom_path.as_ref())
                        .is_some()
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
                .effective_project_config_with_fallback(query, &session_name)
                .layout
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
    /// Per-project host takes precedence over defaults.host. Returns
    /// `Target::Local` when the resolved host matches the local machine.
    pub fn resolve_target(&self, project: &str) -> Target {
        let host = self.config.effective_project_config(project).host;
        match host {
            Some(h) if h != self.local_hostname => Target::Remote(h),
            _ => Target::Local,
        }
    }

    /// Returns the host that global commands (list, picker, kill-all) should
    /// be forwarded to. `Some(host)` when defaults.host is configured and
    /// resolves to a different machine; `None` on the host itself.
    pub fn remote_host(&self) -> Option<String> {
        let host = self.config.default_host()?.to_string();
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

    fn session_metadata(&self, session_name: &str) -> ProjectMetadata {
        let discovered = self.projects.iter().find(|p| {
            let basename = p
                .full_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&p.display_name);
            p.display_name == session_name || basename == session_name
        });

        if let Some(project) = discovered {
            let basename = project
                .full_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&project.display_name);
            let config = self
                .config
                .effective_project_config_with_fallback(&project.display_name, basename);
            return ProjectMetadata {
                path: Some(project.full_path.clone()),
                repository: normalize_repository(
                    config
                        .repository
                        .or_else(|| git_origin_repository(&project.full_path)),
                ),
                responsibility: config
                    .responsibility
                    .unwrap_or_else(|| default_responsibility(&project.display_name)),
            };
        }

        if let Some(entry) = self.config.project(session_name) {
            return ProjectMetadata {
                path: entry.custom_path.clone(),
                repository: normalize_repository(entry.repository.clone()),
                responsibility: entry
                    .responsibility
                    .clone()
                    .unwrap_or_else(|| default_responsibility(session_name)),
            };
        }

        ProjectMetadata {
            path: None,
            repository: None,
            responsibility: default_responsibility(session_name),
        }
    }
}

fn default_responsibility(name: &str) -> String {
    format!("Responsible for {name}")
}

fn git_origin_repository(path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(path)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn normalize_repository(repository: Option<String>) -> Option<String> {
    repository.map(|value| normalize_repository_value(&value))
}

fn normalize_repository_value(value: &str) -> String {
    let trimmed = value.trim();
    if let Some((host, path)) = parse_ssh_repository(trimmed) {
        return format!("https://{}/{}", host, strip_git_suffix(path));
    }
    if let Some(rest) = trimmed.strip_prefix("https://") {
        return format!("https://{}", strip_git_suffix(rest));
    }
    trimmed.to_string()
}

fn parse_ssh_repository(value: &str) -> Option<(&str, &str)> {
    let rest = value.strip_prefix("git@")?;
    let (host, path) = rest.split_once(':')?;
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some((host, path))
}

fn strip_git_suffix(value: &str) -> &str {
    value.strip_suffix(".git").unwrap_or(value)
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
            project_path: None,
            repository: None,
            responsibility: String::new(),
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
            project_path: None,
            repository: None,
            responsibility: String::new(),
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
        assert_eq!(output.sessions[0].responsibility, "Responsible for chops");
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
        let config = DevConfig::new(
            Layout::Default,
            None,
            [(
                "myproject".to_string(),
                ProjectEntry {
                    layout: Layout::Default,
                    custom_path: None,
                    host: Some("remotehost".to_string()),
                    repository: None,
                    responsibility: None,
                    sandbox: None,
                    worktrees: std::collections::HashMap::new(),
                },
            )]
            .into_iter()
            .collect(),
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
        let config = DevConfig::new(
            Layout::Default,
            None,
            [(
                "myproject".to_string(),
                ProjectEntry {
                    layout: Layout::Default,
                    custom_path: None,
                    host: Some("thisbox".to_string()),
                    repository: None,
                    responsibility: None,
                    sandbox: None,
                    worktrees: std::collections::HashMap::new(),
                },
            )]
            .into_iter()
            .collect(),
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
        let config = DevConfig::new(
            Layout::Default,
            Some("dev-host".to_string()),
            Default::default(),
        );
        let mgr = DevManager {
            config,
            projects: Vec::new(),
            projects_dir: PathBuf::from("/tmp"),
            tmux: Box::new(MockTmux::new()),
            local_hostname: "phone".to_string(),
        };
        assert_eq!(
            mgr.resolve_target("anything"),
            Target::Remote("dev-host".to_string())
        );
    }

    #[test]
    fn resolve_target_default_host_local_is_local() {
        let config = DevConfig::new(
            Layout::Default,
            Some("dev-host".to_string()),
            Default::default(),
        );
        let mgr = DevManager {
            config,
            projects: Vec::new(),
            projects_dir: PathBuf::from("/tmp"),
            tmux: Box::new(MockTmux::new()),
            local_hostname: "dev-host".to_string(),
        };
        assert_eq!(mgr.resolve_target("anything"), Target::Local);
    }

    #[test]
    fn start_creates_locally_regardless_of_host_config() {
        // Routing is the CLI's responsibility. The daemon always acts locally;
        // @host config entries are client-side hints and have no meaning here.
        use crate::config::{DevConfig, Layout, ProjectEntry};
        use crate::discovery::DiscoveredProject;

        let config = DevConfig::new(
            Layout::Default,
            None,
            [(
                "myproject".to_string(),
                ProjectEntry {
                    layout: Layout::Default,
                    custom_path: None,
                    host: Some("remotehost".to_string()),
                    repository: None,
                    responsibility: None,
                    sandbox: None,
                    worktrees: std::collections::HashMap::new(),
                },
            )]
            .into_iter()
            .collect(),
        );
        let mock = MockTmux::new();
        let mgr = DevManager {
            config,
            projects: vec![DiscoveredProject {
                display_name: "myproject".to_string(),
                full_path: PathBuf::from("/tmp/myproject"),
            }],
            projects_dir: PathBuf::from("/tmp"),
            tmux: Box::new(mock),
            local_hostname: "localhost".to_string(),
        };

        let session_name = mgr.start("myproject", None).unwrap();
        assert_eq!(session_name, "myproject");
    }

    #[test]
    fn list_enriches_sessions_with_configured_metadata() {
        use crate::config::{DevConfig, Layout, ProjectEntry};
        use crate::discovery::DiscoveredProject;

        let config = DevConfig::new(
            Layout::Default,
            None,
            [(
                "dev".to_string(),
                ProjectEntry {
                    layout: Layout::Default,
                    custom_path: None,
                    host: None,
                    repository: Some("git@github.com:thompsonson/dev.git".to_string()),
                    responsibility: Some("Maintain dev session workflows".to_string()),
                    sandbox: None,
                    worktrees: std::collections::HashMap::new(),
                },
            )]
            .into_iter()
            .collect(),
        );
        let mock = MockTmux::with_sessions(vec![SessionInfo {
            name: "dev".to_string(),
            host: String::new(),
            pane_count: 1,
            attached: false,
            last_activity: 0,
            layout: "default".to_string(),
            project_path: None,
            repository: None,
            responsibility: String::new(),
        }]);
        let mgr = DevManager {
            config,
            projects: vec![DiscoveredProject {
                display_name: "dev".to_string(),
                full_path: PathBuf::from("/tmp/dev"),
            }],
            projects_dir: PathBuf::from("/tmp"),
            tmux: Box::new(mock),
            local_hostname: "dev-host".to_string(),
        };

        let output = mgr.list().unwrap();
        let session = &output.sessions[0];
        assert_eq!(session.project_path.as_deref(), Some("/tmp/dev"));
        assert_eq!(
            session.repository.as_deref(),
            Some("https://github.com/thompsonson/dev")
        );
        assert_eq!(session.responsibility, "Maintain dev session workflows");
    }

    #[test]
    fn list_enriches_projects_with_configured_metadata() {
        use crate::config::{DevConfig, Layout, ProjectEntry};
        use crate::discovery::DiscoveredProject;

        let config = DevConfig::new(
            Layout::Default,
            None,
            [(
                "dev".to_string(),
                ProjectEntry {
                    layout: Layout::Default,
                    custom_path: None,
                    host: None,
                    repository: Some("https://github.com/thompsonson/dev.git".to_string()),
                    responsibility: Some("Maintain dev session workflows".to_string()),
                    sandbox: None,
                    worktrees: std::collections::HashMap::new(),
                },
            )]
            .into_iter()
            .collect(),
        );
        let mgr = DevManager {
            config,
            projects: vec![DiscoveredProject {
                display_name: "dev".to_string(),
                full_path: PathBuf::from("/tmp/dev"),
            }],
            projects_dir: PathBuf::from("/tmp"),
            tmux: Box::new(MockTmux::new()),
            local_hostname: "dev-host".to_string(),
        };

        let output = mgr.list().unwrap();
        let project = &output.projects[0];
        assert_eq!(
            project.repository.as_deref(),
            Some("https://github.com/thompsonson/dev")
        );
        assert_eq!(project.responsibility, "Maintain dev session workflows");
    }

    #[test]
    fn repository_normalization_preserves_unparseable_values() {
        assert_eq!(
            normalize_repository_value("git@gitlab.com:owner/repo.git"),
            "https://gitlab.com/owner/repo"
        );
        assert_eq!(
            normalize_repository_value("https://github.com/owner/repo.git"),
            "https://github.com/owner/repo"
        );
        assert_eq!(
            normalize_repository_value("file:///tmp/repo"),
            "file:///tmp/repo"
        );
    }

    #[test]
    fn list_uses_git_origin_repository_fallback() {
        use crate::discovery::DiscoveredProject;
        use std::process::Command as GitCommand;

        let tmp = tempfile::TempDir::new().unwrap();
        GitCommand::new("git")
            .arg("init")
            .arg(tmp.path())
            .output()
            .unwrap();
        GitCommand::new("git")
            .args(["-C"])
            .arg(tmp.path())
            .args(["remote", "add", "origin", "git@github.com:owner/repo.git"])
            .output()
            .unwrap();

        let mgr = DevManager {
            config: DevConfig::default(),
            projects: vec![DiscoveredProject {
                display_name: "repo".to_string(),
                full_path: tmp.path().to_path_buf(),
            }],
            projects_dir: tmp.path().parent().unwrap().to_path_buf(),
            tmux: Box::new(MockTmux::new()),
            local_hostname: "dev-host".to_string(),
        };

        let output = mgr.list().unwrap();
        assert_eq!(
            output.projects[0].repository.as_deref(),
            Some("https://github.com/owner/repo")
        );
    }
}
