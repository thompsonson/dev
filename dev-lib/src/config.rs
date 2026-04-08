use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// Supported tmux layouts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    Default,
    Claude,
}

impl Layout {
    fn parse(s: &str) -> Self {
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

/// A per-project configuration entry.
#[derive(Debug, Clone)]
pub struct ProjectEntry {
    pub layout: Layout,
    pub custom_path: Option<PathBuf>,
    pub host: Option<String>,
}

/// Parsed dev configuration.
#[derive(Debug, Clone)]
pub struct DevConfig {
    pub default_layout: Layout,
    pub projects: HashMap<String, ProjectEntry>,
}

impl Default for DevConfig {
    fn default() -> Self {
        Self {
            default_layout: Layout::Default,
            projects: HashMap::new(),
        }
    }
}

/// Default config file path.
pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".config/dev/config")
}

/// Parse config from a string (testable, no I/O).
///
/// Format: `key=layout[:path][@host]`
/// Special key: `default_layout=<layout>`
pub fn parse_config_str(content: &str, home: &Path) -> DevConfig {
    let mut config = DevConfig::default();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        if key == "default_layout" {
            config.default_layout = Layout::parse(value);
            continue;
        }

        // Parse value: layout[:path][@host]
        // First split off @host if present
        let (before_host, host) = match value.rsplit_once('@') {
            Some((before, host)) => (before, Some(host.to_string())),
            None => (value, None),
        };

        // Then split off :path if present
        let (layout_str, custom_path) = match before_host.split_once(':') {
            Some((layout, path)) => {
                let expanded = if path.starts_with('~') {
                    home.join(path.trim_start_matches("~/"))
                } else {
                    PathBuf::from(path)
                };
                (layout, Some(expanded))
            }
            None => (before_host, None),
        };

        config.projects.insert(
            key.to_string(),
            ProjectEntry {
                layout: Layout::parse(layout_str),
                custom_path,
                host,
            },
        );
    }

    config
}

/// Parse config from the file at the given path.
pub fn parse_config(path: &Path) -> Result<DevConfig> {
    let home = dirs::home_dir().unwrap_or_default();
    if !path.exists() {
        return Ok(DevConfig::default());
    }
    let content = std::fs::read_to_string(path)?;
    Ok(parse_config_str(&content, &home))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/testuser")
    }

    #[test]
    fn empty_config() {
        let config = parse_config_str("", &home());
        assert_eq!(config.default_layout, Layout::Default);
        assert!(config.projects.is_empty());
    }

    #[test]
    fn comments_and_blank_lines() {
        let config = parse_config_str("# comment\n\n  # another\n", &home());
        assert!(config.projects.is_empty());
    }

    #[test]
    fn default_layout() {
        let config = parse_config_str("default_layout=claude", &home());
        assert_eq!(config.default_layout, Layout::Claude);
    }

    #[test]
    fn simple_project() {
        let config = parse_config_str("myproject=claude", &home());
        let entry = &config.projects["myproject"];
        assert_eq!(entry.layout, Layout::Claude);
        assert!(entry.custom_path.is_none());
        assert!(entry.host.is_none());
    }

    #[test]
    fn project_with_host() {
        let config = parse_config_str("myproject=claude@myserver", &home());
        let entry = &config.projects["myproject"];
        assert_eq!(entry.layout, Layout::Claude);
        assert!(entry.custom_path.is_none());
        assert_eq!(entry.host.as_deref(), Some("myserver"));
    }

    #[test]
    fn project_with_path() {
        let config = parse_config_str("dotfiles=claude:~/.local/share/chezmoi", &home());
        let entry = &config.projects["dotfiles"];
        assert_eq!(entry.layout, Layout::Claude);
        assert_eq!(
            entry.custom_path.as_deref(),
            Some(Path::new("/home/testuser/.local/share/chezmoi"))
        );
        assert!(entry.host.is_none());
    }

    #[test]
    fn project_with_path_and_host() {
        let config = parse_config_str("proj=default:/opt/proj@remotehost", &home());
        let entry = &config.projects["proj"];
        assert_eq!(entry.layout, Layout::Default);
        assert_eq!(entry.custom_path.as_deref(), Some(Path::new("/opt/proj")));
        assert_eq!(entry.host.as_deref(), Some("remotehost"));
    }

    #[test]
    fn multiple_entries() {
        let input = "\
default_layout=default
# projects
chops=claude
dotfiles=claude:~/.local/share/chezmoi
remote=default@server1
";
        let config = parse_config_str(input, &home());
        assert_eq!(config.default_layout, Layout::Default);
        assert_eq!(config.projects.len(), 3);
        assert_eq!(config.projects["chops"].layout, Layout::Claude);
        assert_eq!(config.projects["remote"].host.as_deref(), Some("server1"));
    }
}
