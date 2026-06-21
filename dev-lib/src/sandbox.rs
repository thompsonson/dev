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
        .unwrap_or(&discovered.display_name);
    let entry = config
        .project(&discovered.display_name)
        .or_else(|| config.project(basename));
    let sandbox = entry.and_then(|entry| entry.sandbox.as_ref());

    let name = sandbox
        .and_then(|sandbox| sandbox.profile_name.clone())
        .unwrap_or_else(|| format!("dev-{basename}-opencode"));
    let base_profile = sandbox
        .and_then(|sandbox| sandbox.base_profile.clone())
        .unwrap_or_else(|| defaults.base_profile.clone());
    let profile_dir = defaults
        .profile_dir
        .clone()
        .unwrap_or_else(default_profile_dir);
    let path = profile_dir.join(format!("{name}.json"));

    let read = sandbox
        .map(|sandbox| absolutize_all(&sandbox.read, &discovered.full_path))
        .unwrap_or_default();
    let sockets = sandbox
        .filter(|sandbox| !sandbox.sockets.is_empty())
        .map(|sandbox| sandbox.sockets.clone())
        .unwrap_or_else(|| defaults.sockets.clone());
    let allow = sandbox_write_allow(sandbox, &discovered.full_path);

    let json = json!({
        "extends": base_profile,
        "meta": {
            "name": name,
            "description": format!("Dev-managed sandbox for {basename}"),
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
            "unix_socket": sockets,
        }
    });

    Ok(SandboxProfile { name, path, json })
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
                "manta-site".to_string(),
                ProjectEntry {
                    layout: Layout::Default,
                    custom_path: None,
                    host: None,
                    repository: None,
                    responsibility: None,
                    sandbox: Some(ProjectSandbox {
                        write: vec![PathBuf::from(".")],
                        read: vec![PathBuf::from("/home/mt/Projects/manta")],
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
            display_name: "manta-site".to_string(),
            full_path: PathBuf::from("/home/mt/Projects/manta/manta-site"),
        }];

        let profile = build_nono_profile(&config, &projects, "manta-site").unwrap();
        assert_eq!(profile.name, "dev-manta-site-opencode");
        assert_eq!(profile.json["extends"], "always-further/opencode");
        assert_eq!(profile.json["filesystem"]["allow"], json!([]));
        assert_eq!(
            profile.json["filesystem"]["read"],
            json!(["/home/mt/Projects/manta"])
        );
        assert_eq!(
            profile.json["filesystem"]["unix_socket"],
            json!(["/run/user/1000/dev.sock"])
        );
    }
}
