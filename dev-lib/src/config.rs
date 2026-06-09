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
    /// Host to forward to when no per-project host is set. Written by
    /// `bootstrap.sh --client HOST`; used so bare `dev` on a thin client
    /// transparently attaches to sessions on the always-on host.
    pub default_host: Option<String>,
    pub projects: HashMap<String, ProjectEntry>,
}

impl Default for DevConfig {
    fn default() -> Self {
        Self {
            default_layout: Layout::Default,
            default_host: None,
            projects: HashMap::new(),
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

/// Default config file path.
pub fn config_path() -> PathBuf {
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

    let known_layouts = ["default", "claude"];
    let mut warnings = Vec::new();

    for (i, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warnings.push(format!("config line {}: missing '=' separator", i + 1));
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        if key == "default_layout" {
            if !known_layouts.contains(&value) {
                warnings.push(format!(
                    "config: unknown layout '{value}' for default_layout (known: default, claude)"
                ));
            }
            continue;
        }

        if key == "default_host" {
            continue;
        }

        // Warn on keys that look like typos of the two special keys.
        if key.starts_with("default_") {
            warnings.push(format!(
                "config: unknown key '{key}' — did you mean 'default_layout' or 'default_host'?"
            ));
            continue;
        }

        // Project entry: layout[:path][@host]
        let before_host = value.rsplit_once('@').map_or(value, |(b, _)| b);
        let layout_str = before_host.split_once(':').map_or(before_host, |(l, _)| l);
        if !known_layouts.contains(&layout_str) {
            warnings.push(format!(
                "config: unknown layout '{layout_str}' for project '{key}' (known: default, claude)"
            ));
        }
    }

    warnings
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
        if key == "default_host" {
            config.default_host = Some(value.to_string());
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
    fn default_host() {
        let config = parse_config_str("default_host=pop-mini", &home());
        assert_eq!(config.default_host.as_deref(), Some("pop-mini"));
    }

    #[test]
    fn default_host_with_other_keys() {
        let input = "default_layout=claude\ndefault_host=pop-mini\nmyproject=default\n";
        let config = parse_config_str(input, &home());
        assert_eq!(config.default_host.as_deref(), Some("pop-mini"));
        assert_eq!(config.default_layout, Layout::Claude);
        assert!(config.projects.contains_key("myproject"));
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

    #[test]
    fn validate_clean_config() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(
            f,
            "default_layout=claude\ndefault_host=pop-mini\nmyproj=claude\n"
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
        writeln!(f, "default_layout=fancy").unwrap();
        let warnings = validate_config(f.path());
        assert!(warnings
            .iter()
            .any(|w| w.contains("unknown layout 'fancy'")));
    }

    #[test]
    fn validate_typo_of_default_key() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "default_layot=claude").unwrap();
        let warnings = validate_config(f.path());
        assert!(warnings.iter().any(|w| w.contains("default_layot")));
    }

    #[test]
    fn validate_missing_file_is_clean() {
        let p = std::path::Path::new("/tmp/dev-config-does-not-exist-xyz");
        let warnings = validate_config(p);
        assert!(warnings.is_empty());
    }

    #[test]
    fn validate_unknown_project_layout() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "myproject=badlayout").unwrap();
        let warnings = validate_config(f.path());
        assert!(warnings
            .iter()
            .any(|w| w.contains("unknown layout 'badlayout'")));
    }
}
