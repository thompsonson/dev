use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::config::{DevConfig, ProjectSandbox};
use crate::discovery::DiscoveredProject;
use crate::resolve;

#[derive(Debug, Clone)]
pub struct SandboxProfile {
    pub name: String,
    pub path: PathBuf,
    pub json: Value,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SandboxStatus {
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_generated: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub write: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sockets: Vec<PathBuf>,
    pub runtime: String,
}

struct ResolvedSandbox<'a> {
    sandbox: &'a ProjectSandbox,
    project_path: &'a Path,
    basename: String,
    profile_name: String,
    base_profile: String,
    profile_path: PathBuf,
    sockets: Vec<PathBuf>,
}

pub fn default_profile_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".config/dev/sandbox/profiles")
}

pub fn build_nono_profile(
    config: &DevConfig,
    projects: &[DiscoveredProject],
    project: &str,
) -> Result<SandboxProfile> {
    let resolved = resolve_sandbox(config, projects, project)?;

    let read = absolutize_all(&resolved.sandbox.read, resolved.project_path);
    let allow = [
        sandbox_write_allow(Some(resolved.sandbox), resolved.project_path),
        absolutize_all(&resolved.sandbox.allow, resolved.project_path),
    ]
    .concat();

    let json = json!({
        "extends": resolved.base_profile,
        "meta": {
            "name": resolved.profile_name,
            "description": format!("Dev-managed sandbox for {}", resolved.basename),
        },
        "groups": {
            "include": []
        },
        "workdir": {
            "access": "readwrite"
        },
        "filesystem": {
            "allow": allow,
            "read": read,
            "unix_socket": resolved.sockets,
        }
    });

    Ok(SandboxProfile {
        name: resolved.profile_name,
        path: resolved.profile_path,
        json,
    })
}

pub fn sandbox_status(
    config: &DevConfig,
    projects: &[DiscoveredProject],
    project: &str,
) -> SandboxStatus {
    let Ok(resolved) = resolve_sandbox(config, projects, project) else {
        return SandboxStatus {
            configured: false,
            backend: None,
            base_profile: None,
            profile_name: None,
            profile_path: None,
            profile_generated: None,
            write: Vec::new(),
            read: Vec::new(),
            allow: Vec::new(),
            sockets: Vec::new(),
            runtime: "unknown".to_string(),
        };
    };

    SandboxStatus {
        configured: true,
        backend: Some(config.sandbox_defaults().backend.clone()),
        base_profile: Some(resolved.base_profile),
        profile_name: Some(resolved.profile_name),
        profile_generated: Some(resolved.profile_path.exists()),
        profile_path: Some(resolved.profile_path),
        write: resolved.sandbox.write.clone(),
        read: absolutize_all(&resolved.sandbox.read, resolved.project_path),
        allow: absolutize_all(&resolved.sandbox.allow, resolved.project_path),
        sockets: resolved.sockets,
        runtime: "unknown".to_string(),
    }
}

pub fn write_profile(profile: &SandboxProfile) -> Result<()> {
    let Some(parent) = profile.path.parent() else {
        anyhow::bail!("profile path has no parent: {}", profile.path.display());
    };
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating profile dir {}", parent.display()))?;
    let body = serde_json::to_string_pretty(&profile.json)?;
    std::fs::write(&profile.path, format!("{body}\n"))
        .with_context(|| format!("writing profile {}", profile.path.display()))
}

fn sandbox_write_allow(sandbox: Option<&ProjectSandbox>, project_path: &Path) -> Vec<PathBuf> {
    let Some(sandbox) = sandbox else {
        return Vec::new();
    };
    sandbox
        .write
        .iter()
        .filter(|path| path.as_os_str() != ".")
        .map(|path| absolutize(path, project_path))
        .collect()
}

fn resolve_sandbox<'a>(
    config: &'a DevConfig,
    projects: &'a [DiscoveredProject],
    project: &str,
) -> Result<ResolvedSandbox<'a>> {
    let defaults = config.sandbox_defaults();
    if defaults.backend != "nono" {
        anyhow::bail!("unsupported sandbox backend: {}", defaults.backend);
    }

    let discovered = resolve::resolve_project(project, projects)
        .ok_or_else(|| anyhow::anyhow!("project '{project}' not found"))?;
    let basename = discovered
        .full_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&discovered.display_name)
        .to_string();
    let entry = config
        .project(&discovered.display_name)
        .or_else(|| config.project(&basename))
        .ok_or_else(|| anyhow::anyhow!("project '{project}' has no config entry"))?;
    let sandbox = entry
        .sandbox
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("project '{project}' has no sandbox config"))?;

    let profile_name = sandbox
        .profile_name
        .clone()
        .unwrap_or_else(|| format!("dev-{basename}-opencode"));
    let base_profile = sandbox
        .base_profile
        .clone()
        .unwrap_or_else(|| defaults.base_profile.clone());
    let profile_dir = defaults
        .profile_dir
        .clone()
        .unwrap_or_else(default_profile_dir);
    let profile_path = profile_dir.join(format!("{profile_name}.json"));
    let sockets = if sandbox.sockets.is_empty() {
        defaults.sockets.clone()
    } else {
        sandbox.sockets.clone()
    };

    Ok(ResolvedSandbox {
        sandbox,
        project_path: &discovered.full_path,
        basename,
        profile_name,
        base_profile,
        profile_path,
        sockets,
    })
}

fn absolutize_all(paths: &[PathBuf], project_path: &Path) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|path| absolutize(path, project_path))
        .collect()
}

fn absolutize(path: &Path, project_path: &Path) -> PathBuf {
    if path.as_os_str() == "." {
        return project_path.to_path_buf();
    }
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_path.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DevConfig, Layout, ProjectEntry, ProjectSandbox, SandboxDefaults};
    use std::collections::HashMap;

    #[test]
    fn build_nono_profile_uses_project_sandbox_policy() {
        let config = DevConfig::new_with_sandbox(
            Layout::Default,
            None,
            SandboxDefaults::default(),
            [(
                "web-app".to_string(),
                ProjectEntry {
                    layout: Layout::Default,
                    custom_path: None,
                    host: None,
                    repository: None,
                    responsibility: None,
                    sandbox: Some(ProjectSandbox {
                        write: vec![PathBuf::from(".")],
                        read: vec![PathBuf::from("/home/testuser/Projects/team-a")],
                        allow: vec![PathBuf::from("/home/testuser/.config/gh")],
                        sockets: Vec::new(),
                        base_profile: None,
                        profile_name: None,
                    }),
                    worktrees: HashMap::new(),
                },
            )]
            .into_iter()
            .collect(),
        );
        let projects = vec![DiscoveredProject {
            display_name: "web-app".to_string(),
            full_path: PathBuf::from("/home/testuser/Projects/team-a/web-app"),
        }];

        let profile = build_nono_profile(&config, &projects, "web-app").unwrap();
        assert_eq!(profile.name, "dev-web-app-opencode");
        assert_eq!(profile.json["extends"], "always-further/opencode");
        assert_eq!(
            profile.json["filesystem"]["allow"],
            json!(["/home/testuser/.config/gh"])
        );
        assert_eq!(
            profile.json["filesystem"]["read"],
            json!(["/home/testuser/Projects/team-a"])
        );
        assert_eq!(
            profile.json["filesystem"]["unix_socket"],
            json!(["/run/user/1000/dev.sock"])
        );
    }

    #[test]
    fn sandbox_status_reports_unconfigured_project() {
        let config = DevConfig::default();
        let projects = vec![DiscoveredProject {
            display_name: "dev".to_string(),
            full_path: PathBuf::from("/home/testuser/Projects/thompsonson/dev"),
        }];

        let status = sandbox_status(&config, &projects, "dev");
        assert!(!status.configured);
        assert_eq!(status.runtime, "unknown");
        assert!(status.profile_generated.is_none());
    }

    #[test]
    fn sandbox_status_reports_configured_profile_state() {
        let tmp = tempfile::TempDir::new().unwrap();
        let defaults = SandboxDefaults {
            profile_dir: Some(tmp.path().to_path_buf()),
            ..SandboxDefaults::default()
        };
        let config = DevConfig::new_with_sandbox(
            Layout::Default,
            None,
            defaults,
            [(
                "web-app".to_string(),
                ProjectEntry {
                    layout: Layout::Default,
                    custom_path: None,
                    host: None,
                    repository: None,
                    responsibility: None,
                    sandbox: Some(ProjectSandbox {
                        write: vec![PathBuf::from(".")],
                        read: vec![PathBuf::from("/home/testuser/Projects/team-a")],
                        allow: Vec::new(),
                        sockets: Vec::new(),
                        base_profile: None,
                        profile_name: None,
                    }),
                    worktrees: HashMap::new(),
                },
            )]
            .into_iter()
            .collect(),
        );
        let projects = vec![DiscoveredProject {
            display_name: "web-app".to_string(),
            full_path: PathBuf::from("/home/testuser/Projects/team-a/web-app"),
        }];

        let status = sandbox_status(&config, &projects, "web-app");
        assert!(status.configured);
        assert_eq!(status.backend.as_deref(), Some("nono"));
        assert_eq!(status.profile_name.as_deref(), Some("dev-web-app-opencode"));
        assert_eq!(status.profile_generated, Some(false));

        let profile_path = status.profile_path.clone().unwrap();
        std::fs::write(&profile_path, "{}").unwrap();
        let status = sandbox_status(&config, &projects, "web-app");
        assert_eq!(status.profile_generated, Some(true));
        assert_eq!(status.write, vec![PathBuf::from(".")]);
        assert_eq!(
            status.read,
            vec![PathBuf::from("/home/testuser/Projects/team-a")]
        );
    }
}
