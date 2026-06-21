use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Supported tmux layouts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    Default,
    Claude,
}

impl Layout {
    pub fn parse(s: &str) -> Self {
        match s {
            "claude" => Self::Claude,
            _ => Self::Default,
        }
    }
}

impl std::fmt::Display for Layout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default => write!(f, "default"),
            Self::Claude => write!(f, "claude"),
        }
    }
}

/// Raw TOML config. Mirrors `~/.config/dev/config.toml` exactly.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawDevConfig {
    #[serde(default)]
    pub defaults: RawDefaults,
    #[serde(default)]
    pub sandbox: RawSandboxDefaults,
    #[serde(default)]
    pub project: HashMap<String, RawProjectEntry>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawDefaults {
    pub layout: Option<Layout>,
    pub host: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawSandboxDefaults {
    pub backend: Option<String>,
    pub base_profile: Option<String>,
    pub profile_dir: Option<PathBuf>,
    #[serde(default)]
    pub sockets: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawProjectEntry {
    pub layout: Option<Layout>,
    pub path: Option<PathBuf>,
    pub host: Option<String>,
    pub repository: Option<String>,
    pub responsibility: Option<String>,
    #[serde(default)]
    pub sandbox: Option<RawProjectSandbox>,
    #[serde(default)]
    pub worktree: HashMap<String, RawWorktreeEntry>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawProjectSandbox {
    #[serde(default)]
    pub write: Vec<PathBuf>,
    #[serde(default)]
    pub read: Vec<PathBuf>,
    #[serde(default)]
    pub sockets: Vec<PathBuf>,
    pub base_profile: Option<String>,
    pub profile_name: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawWorktreeEntry {
    pub layout: Option<Layout>,
    pub path: Option<PathBuf>,
    pub host: Option<String>,
    pub repository: Option<String>,
    pub responsibility: Option<String>,
}

/// A domain project config entry. Values are effective for the main worktree.
#[derive(Debug, Clone)]
pub struct ProjectEntry {
    pub layout: Layout,
    pub custom_path: Option<PathBuf>,
    pub host: Option<String>,
    pub repository: Option<String>,
    pub responsibility: Option<String>,
    pub sandbox: Option<ProjectSandbox>,
    /// Parsed now to establish the ADR 002 domain model; consumed by the
    /// URI/worktree work in #59/#60, not by the current project-only runtime.
    pub worktrees: HashMap<String, WorktreeEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxDefaults {
    pub backend: String,
    pub base_profile: String,
    pub profile_dir: Option<PathBuf>,
    pub sockets: Vec<PathBuf>,
}

impl Default for SandboxDefaults {
    fn default() -> Self {
        Self {
            backend: "nono".to_string(),
            base_profile: "always-further/opencode".to_string(),
            profile_dir: None,
            sockets: vec![PathBuf::from("/run/user/1000/dev.sock")],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSandbox {
    pub write: Vec<PathBuf>,
    pub read: Vec<PathBuf>,
    pub sockets: Vec<PathBuf>,
    pub base_profile: Option<String>,
    pub profile_name: Option<String>,
}

/// A domain worktree config entry. Values are effective after inheritance.
#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    pub layout: Layout,
    pub custom_path: Option<PathBuf>,
    pub host: Option<String>,
    pub repository: Option<String>,
    pub responsibility: Option<String>,
}

/// Effective config for a concrete project/worktree session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSessionConfig {
    pub layout: Layout,
    pub host: Option<String>,
    pub custom_path: Option<PathBuf>,
    pub repository: Option<String>,
    pub responsibility: Option<String>,
}

/// Domain config consumed by application code.
#[derive(Debug, Clone)]
pub struct DevConfig {
    default_layout: Layout,
    default_host: Option<String>,
    sandbox_defaults: SandboxDefaults,
    projects: HashMap<String, ProjectEntry>,
}

impl Default for DevConfig {
    fn default() -> Self {
        Self {
            default_layout: Layout::Default,
            default_host: None,
            sandbox_defaults: SandboxDefaults::default(),
            projects: HashMap::new(),
        }
    }
}

impl DevConfig {
    /// Build a domain config from already-effective entries.
    ///
    /// Prefer `from_raw`/`from_toml_str` for real config loading; this
    /// constructor is for tests and manual construction where project entries
    /// have already had defaults folded in.
    pub fn new(
        default_layout: Layout,
        default_host: Option<String>,
        projects: HashMap<String, ProjectEntry>,
    ) -> Self {
        Self::new_with_sandbox(
            default_layout,
            default_host,
            SandboxDefaults::default(),
            projects,
        )
    }

    pub fn new_with_sandbox(
        default_layout: Layout,
        default_host: Option<String>,
        sandbox_defaults: SandboxDefaults,
        projects: HashMap<String, ProjectEntry>,
    ) -> Self {
        Self {
            default_layout,
            default_host,
            sandbox_defaults,
            projects,
        }
    }

    pub fn from_raw(raw: RawDevConfig, home: &Path) -> Self {
        let default_layout = raw.defaults.layout.unwrap_or(Layout::Default);
        let default_host = raw.defaults.host;
        let sandbox_defaults = SandboxDefaults {
            backend: raw.sandbox.backend.unwrap_or_else(|| "nono".to_string()),
            base_profile: raw
                .sandbox
                .base_profile
                .unwrap_or_else(|| "always-further/opencode".to_string()),
            profile_dir: raw.sandbox.profile_dir.map(|p| expand_home(p, home)),
            sockets: if raw.sandbox.sockets.is_empty() {
                SandboxDefaults::default().sockets
            } else {
                raw.sandbox
                    .sockets
                    .into_iter()
                    .map(|p| expand_home(p, home))
                    .collect()
            },
        };
        let mut projects = HashMap::new();

        for (name, raw_project) in raw.project {
            let project_layout = raw_project
                .layout
                .clone()
                .unwrap_or_else(|| default_layout.clone());
            let project_host = raw_project.host.clone().or_else(|| default_host.clone());
            let project_path = raw_project.path.map(|p| expand_home(p, home));
            let sandbox = raw_project.sandbox.map(|sandbox| ProjectSandbox {
                write: sandbox
                    .write
                    .into_iter()
                    .map(|p| expand_home(p, home))
                    .collect(),
                read: sandbox
                    .read
                    .into_iter()
                    .map(|p| expand_home(p, home))
                    .collect(),
                sockets: sandbox
                    .sockets
                    .into_iter()
                    .map(|p| expand_home(p, home))
                    .collect(),
                base_profile: sandbox.base_profile,
                profile_name: sandbox.profile_name,
            });
            let mut worktrees = HashMap::new();

            for (worktree_name, raw_worktree) in raw_project.worktree {
                worktrees.insert(
                    worktree_name,
                    WorktreeEntry {
                        layout: raw_worktree
                            .layout
                            .unwrap_or_else(|| project_layout.clone()),
                        custom_path: raw_worktree.path.map(|p| expand_home(p, home)),
                        host: raw_worktree.host.or_else(|| project_host.clone()),
                        repository: raw_worktree
                            .repository
                            .or_else(|| raw_project.repository.clone()),
                        responsibility: raw_worktree
                            .responsibility
                            .or_else(|| raw_project.responsibility.clone()),
                    },
                );
            }

            projects.insert(
                name,
                ProjectEntry {
                    layout: project_layout,
                    custom_path: project_path,
                    host: project_host,
                    repository: raw_project.repository,
                    responsibility: raw_project.responsibility,
                    sandbox,
                    worktrees,
                },
            );
        }

        Self {
            default_layout,
            default_host,
            sandbox_defaults,
            projects,
        }
    }

    pub fn from_toml_str(content: &str, home: &Path) -> Result<Self> {
        let raw: RawDevConfig = toml::from_str(content).context("parse TOML config")?;
        Ok(Self::from_raw(raw, home))
    }

    pub fn default_layout(&self) -> &Layout {
        &self.default_layout
    }

    pub fn default_host(&self) -> Option<&str> {
        self.default_host.as_deref()
    }

    pub fn sandbox_defaults(&self) -> &SandboxDefaults {
        &self.sandbox_defaults
    }

    pub fn projects(&self) -> &HashMap<String, ProjectEntry> {
        &self.projects
    }

    pub fn project(&self, name: &str) -> Option<&ProjectEntry> {
        self.projects.get(name)
    }

    pub fn effective_project_config(&self, project: &str) -> ResolvedSessionConfig {
        match self.projects.get(project) {
            Some(entry) => ResolvedSessionConfig {
                layout: entry.layout.clone(),
                host: entry.host.clone(),
                custom_path: entry.custom_path.clone(),
                repository: entry.repository.clone(),
                responsibility: entry.responsibility.clone(),
            },
            None => ResolvedSessionConfig {
                layout: self.default_layout.clone(),
                host: self.default_host.clone(),
                custom_path: None,
                repository: None,
                responsibility: None,
            },
        }
    }

    pub fn effective_project_config_with_fallback(
        &self,
        project: &str,
        fallback_project: &str,
    ) -> ResolvedSessionConfig {
        if let Some(entry) = self
            .projects
            .get(project)
            .or_else(|| self.projects.get(fallback_project))
        {
            return ResolvedSessionConfig {
                layout: entry.layout.clone(),
                host: entry.host.clone(),
                custom_path: entry.custom_path.clone(),
                repository: entry.repository.clone(),
                responsibility: entry.responsibility.clone(),
            };
        }
        ResolvedSessionConfig {
            layout: self.default_layout.clone(),
            host: self.default_host.clone(),
            custom_path: None,
            repository: None,
            responsibility: None,
        }
    }

    pub fn effective_session_config(
        &self,
        project: &str,
        worktree: Option<&str>,
    ) -> ResolvedSessionConfig {
        let project_config = self.effective_project_config(project);
        let Some(worktree) = worktree else {
            return project_config;
        };
        let Some(entry) = self.projects.get(project) else {
            return project_config;
        };
        let Some(worktree_entry) = entry.worktrees.get(worktree) else {
            return project_config;
        };
        ResolvedSessionConfig {
            layout: worktree_entry.layout.clone(),
            host: worktree_entry.host.clone(),
            custom_path: worktree_entry.custom_path.clone(),
            repository: worktree_entry.repository.clone(),
            responsibility: worktree_entry.responsibility.clone(),
        }
    }
}

/// Parse a layout string strictly — errors on unknown values.
pub fn parse_layout(s: &str) -> Result<Layout> {
    match s {
        "default" => Ok(Layout::Default),
        "claude" => Ok(Layout::Claude),
        other => anyhow::bail!("unknown layout: {other}"),
    }
}

/// Default TOML config file path.
pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".config/dev/config.toml")
}

/// Legacy INI config file path.
pub fn legacy_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".config/dev/config")
}

/// Validate the config file at `path`. Returns a list of human-readable
/// warning strings (empty = all clear). Does not error on a missing file.
pub fn validate_config(path: &Path) -> Vec<String> {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => return vec![format!("config: cannot read file: {e}")],
    };

    match toml::from_str::<RawDevConfig>(&content) {
        Ok(_) => Vec::new(),
        Err(e) => vec![format!("config: TOML parse error: {e}")],
    }
}

/// Parse config from the file at the given path.
pub fn parse_config(path: &Path) -> Result<DevConfig> {
    let home = dirs::home_dir().unwrap_or_default();
    if !path.exists() {
        let legacy = legacy_path_for(path);
        if legacy.exists() {
            anyhow::bail!(
                "legacy config found at {}; migrate it to TOML at {}",
                legacy.display(),
                path.display()
            );
        }
        return Ok(DevConfig::default());
    }
    let content = std::fs::read_to_string(path)?;
    DevConfig::from_toml_str(&content, &home)
}

fn legacy_path_for(path: &Path) -> PathBuf {
    if path.file_name().and_then(|n| n.to_str()) == Some("config.toml") {
        path.with_file_name("config")
    } else {
        legacy_config_path()
    }
}

fn expand_home(path: PathBuf, home: &Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path;
    };
    if s == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return home.join(rest);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/testuser")
    }

    #[test]
    fn empty_config_uses_domain_defaults() {
        let config = DevConfig::from_toml_str("", &home()).unwrap();
        assert_eq!(config.default_layout(), &Layout::Default);
        assert_eq!(config.default_host(), None);
        assert!(config.projects().is_empty());
    }

    #[test]
    fn parses_defaults_table() {
        let config = DevConfig::from_toml_str(
            r#"
            [defaults]
            layout = "claude"
            host = "pop-mini"
            "#,
            &home(),
        )
        .unwrap();
        assert_eq!(config.default_layout(), &Layout::Claude);
        assert_eq!(config.default_host(), Some("pop-mini"));
    }

    #[test]
    fn project_inherits_defaults() {
        let config = DevConfig::from_toml_str(
            r#"
            [defaults]
            layout = "claude"
            host = "pop-mini"

            [project.atomicguard]
            "#,
            &home(),
        )
        .unwrap();
        let resolved = config.effective_project_config("atomicguard");
        assert_eq!(resolved.layout, Layout::Claude);
        assert_eq!(resolved.host.as_deref(), Some("pop-mini"));
    }

    #[test]
    fn project_overrides_defaults_and_expands_path() {
        let config = DevConfig::from_toml_str(
            r#"
            [defaults]
            layout = "default"
            host = "pop-mini"

            [project.dotfiles]
            layout = "claude"
            host = "laptop"
            path = "~/.local/share/chezmoi"
            "#,
            &home(),
        )
        .unwrap();
        let resolved = config.effective_project_config("dotfiles");
        assert_eq!(resolved.layout, Layout::Claude);
        assert_eq!(resolved.host.as_deref(), Some("laptop"));
        assert_eq!(
            resolved.custom_path.as_deref(),
            Some(Path::new("/home/testuser/.local/share/chezmoi"))
        );
    }

    #[test]
    fn project_accepts_optional_repository_and_responsibility_metadata() {
        let config = DevConfig::from_toml_str(
            r#"
            [project.dev]
            repository = "git@github.com:thompsonson/dev.git"
            responsibility = "Maintain dev session workflows"
            "#,
            &home(),
        )
        .unwrap();
        let resolved = config.effective_project_config("dev");
        assert_eq!(
            resolved.repository.as_deref(),
            Some("git@github.com:thompsonson/dev.git")
        );
        assert_eq!(
            resolved.responsibility.as_deref(),
            Some("Maintain dev session workflows")
        );
    }

    #[test]
    fn parses_sandbox_defaults_and_project_policy() {
        let config = DevConfig::from_toml_str(
            r#"
            [sandbox]
            backend = "nono"
            base_profile = "always-further/opencode"
            profile_dir = "~/.config/dev/sandbox/profiles"
            sockets = ["/run/user/1000/dev.sock"]

            [project.manta-site]
            path = "~/Projects/manta/manta-site"

            [project.manta-site.sandbox]
            write = ["."]
            read = ["~/Projects/manta"]
            "#,
            &home(),
        )
        .unwrap();

        assert_eq!(config.sandbox_defaults().backend, "nono");
        assert_eq!(
            config.sandbox_defaults().profile_dir.as_deref(),
            Some(Path::new("/home/testuser/.config/dev/sandbox/profiles"))
        );
        let sandbox = config
            .project("manta-site")
            .unwrap()
            .sandbox
            .as_ref()
            .unwrap();
        assert_eq!(sandbox.write, vec![PathBuf::from(".")]);
        assert_eq!(
            sandbox.read,
            vec![PathBuf::from("/home/testuser/Projects/manta")]
        );
    }

    #[test]
    fn worktree_inherits_from_project_then_overrides_specific_fields() {
        let config = DevConfig::from_toml_str(
            r#"
            [defaults]
            layout = "default"
            host = "pop-mini"

            [project.atomicguard]
            layout = "claude"

            [project.atomicguard.worktree.fix-guards]
            layout = "default"
            path = "~/Projects/atomicguard.worktrees/fix-guards"
            "#,
            &home(),
        )
        .unwrap();
        let resolved = config.effective_session_config("atomicguard", Some("fix-guards"));
        assert_eq!(resolved.layout, Layout::Default);
        assert_eq!(resolved.host.as_deref(), Some("pop-mini"));
        assert_eq!(
            resolved.custom_path.as_deref(),
            Some(Path::new(
                "/home/testuser/Projects/atomicguard.worktrees/fix-guards"
            ))
        );
    }

    #[test]
    fn worktree_inherits_project_metadata() {
        let config = DevConfig::from_toml_str(
            r#"
            [project.dev]
            repository = "https://github.com/thompsonson/dev"
            responsibility = "Maintain dev"

            [project.dev.worktree.fix-send]
            path = "~/Projects/dev.worktrees/fix-send"
            "#,
            &home(),
        )
        .unwrap();
        let resolved = config.effective_session_config("dev", Some("fix-send"));
        assert_eq!(
            resolved.repository.as_deref(),
            Some("https://github.com/thompsonson/dev")
        );
        assert_eq!(resolved.responsibility.as_deref(), Some("Maintain dev"));
    }

    #[test]
    fn unknown_project_uses_defaults() {
        let config = DevConfig::from_toml_str(
            r#"
            [defaults]
            layout = "claude"
            host = "pop-mini"
            "#,
            &home(),
        )
        .unwrap();
        let resolved = config.effective_project_config("missing");
        assert_eq!(resolved.layout, Layout::Claude);
        assert_eq!(resolved.host.as_deref(), Some("pop-mini"));
        assert!(resolved.custom_path.is_none());
    }

    #[test]
    fn fallback_project_preserves_current_query_then_session_name_resolution() {
        let config = DevConfig::from_toml_str(
            r#"
            [defaults]
            layout = "default"

            [project.repo]
            layout = "claude"
            "#,
            &home(),
        )
        .unwrap();
        let resolved = config.effective_project_config_with_fallback("org/repo", "repo");
        assert_eq!(resolved.layout, Layout::Claude);
    }

    #[test]
    fn fallback_project_prefers_query_key_over_session_name() {
        let config = DevConfig::from_toml_str(
            r#"
            [project.org-repo]
            layout = "claude"

            [project.repo]
            layout = "default"
            "#,
            &home(),
        )
        .unwrap();
        let resolved = config.effective_project_config_with_fallback("org-repo", "repo");
        assert_eq!(resolved.layout, Layout::Claude);
    }

    #[test]
    fn validate_clean_toml_config() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
            [defaults]
            layout = "claude"
            host = "pop-mini"

            [project.myproj]
            layout = "default"
            "#
        )
        .unwrap();
        let warnings = validate_config(f.path());
        assert!(
            warnings.is_empty(),
            "expected no warnings, got: {:?}",
            warnings
        );
    }

    #[test]
    fn validate_unknown_layout_value() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "[defaults]\nlayout = \"fancy\"").unwrap();
        let warnings = validate_config(f.path());
        assert!(warnings.iter().any(|w| w.contains("unknown variant")));
    }

    #[test]
    fn validate_unknown_top_level_table() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "[defualts]\nlayout = \"claude\"").unwrap();
        let warnings = validate_config(f.path());
        assert!(warnings.iter().any(|w| w.contains("unknown field")));
    }

    #[test]
    fn validate_unknown_defaults_field() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "[defaults]\nlayuot = \"claude\"").unwrap();
        let warnings = validate_config(f.path());
        assert!(warnings.iter().any(|w| w.contains("unknown field")));
    }

    #[test]
    fn validate_unknown_project_field() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "[project.dev]\nhsot = \"pop-mini\"").unwrap();
        let warnings = validate_config(f.path());
        assert!(warnings.iter().any(|w| w.contains("unknown field")));
    }

    #[test]
    fn validate_unknown_worktree_field() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "[project.dev.worktree.fix]\nlayuot = \"claude\"").unwrap();
        let warnings = validate_config(f.path());
        assert!(warnings.iter().any(|w| w.contains("unknown field")));
    }

    #[test]
    fn validate_missing_file_is_clean() {
        let p = std::path::Path::new("/tmp/dev-config-does-not-exist-xyz.toml");
        let warnings = validate_config(p);
        assert!(warnings.is_empty());
    }

    #[test]
    fn parse_config_errors_when_legacy_ini_exists_without_toml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let toml_path = tmp.path().join("config.toml");
        std::fs::write(tmp.path().join("config"), "default_layout=claude\n").unwrap();

        let err = parse_config(&toml_path).unwrap_err().to_string();
        assert!(err.contains("legacy config found"));
        assert!(err.contains("config.toml"));
    }

    #[test]
    fn parse_layout_known() {
        assert_eq!(parse_layout("default").unwrap(), Layout::Default);
        assert_eq!(parse_layout("claude").unwrap(), Layout::Claude);
        assert!(parse_layout("weird").is_err());
    }
}
