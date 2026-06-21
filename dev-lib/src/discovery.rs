use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::DevConfig;

const MAX_DEPTH: usize = 3;

/// A discovered project with its display name and full path.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoveredProject {
    pub display_name: String,
    pub full_path: PathBuf,
}

/// Discover projects by scanning for `.git` directories up to `MAX_DEPTH` levels.
/// Handles name collisions by using the relative path as display name.
/// Merges custom-path entries from config.
pub fn discover_projects(base: &Path, config: &DevConfig) -> Vec<DiscoveredProject> {
    let mut entries: Vec<(String, PathBuf)> = Vec::new();

    if base.is_dir() {
        find_git_repos(base, base, 0, &mut entries);
        entries.sort_by(|a, b| a.0.cmp(&b.0));
    }

    // Count basenames for collision detection
    let mut name_count: HashMap<String, usize> = HashMap::new();
    for (rel_path, _) in &entries {
        let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
        *name_count.entry(name.to_string()).or_default() += 1;
    }

    let mut projects: Vec<DiscoveredProject> = entries
        .into_iter()
        .map(|(rel_path, full_path)| {
            let name = rel_path.rsplit('/').next().unwrap_or(&rel_path).to_string();
            let display_name = if name_count.get(&name).copied().unwrap_or(0) > 1 {
                rel_path
            } else {
                name
            };
            DiscoveredProject {
                display_name,
                full_path,
            }
        })
        .collect();

    // Append custom-path entries from config that aren't already discovered
    for (key, entry) in config.projects() {
        if let Some(ref custom_path) = entry.custom_path {
            if custom_path.is_dir() && !projects.iter().any(|p| p.display_name == *key) {
                projects.push(DiscoveredProject {
                    display_name: key.clone(),
                    full_path: custom_path.clone(),
                });
            }
        }
    }

    projects
}

/// Recursively find directories containing `.git`, up to max_depth.
fn find_git_repos(base: &Path, current: &Path, depth: usize, results: &mut Vec<(String, PathBuf)>) {
    if depth >= MAX_DEPTH {
        return;
    }

    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };

    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.is_dir() {
                Some(path)
            } else {
                None
            }
        })
        .collect();
    dirs.sort();

    for dir in dirs {
        if dir.join(".git").exists() {
            let rel = dir
                .strip_prefix(base)
                .unwrap_or(&dir)
                .to_string_lossy()
                .to_string();
            results.push((rel, dir.clone()));
        } else {
            // Only recurse into non-git directories (don't descend into submodules)
            find_git_repos(base, &dir, depth + 1, results);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Layout, ProjectEntry};
    use tempfile::TempDir;

    fn setup_projects(tmp: &TempDir, paths: &[&str]) {
        for path in paths {
            let dir = tmp.path().join(path);
            std::fs::create_dir_all(dir.join(".git")).unwrap();
        }
    }

    #[test]
    fn discovers_flat_projects() {
        let tmp = TempDir::new().unwrap();
        setup_projects(&tmp, &["alpha", "beta", "gamma"]);
        let config = DevConfig::default();
        let projects = discover_projects(tmp.path(), &config);
        let names: Vec<&str> = projects.iter().map(|p| p.display_name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn discovers_nested_projects() {
        let tmp = TempDir::new().unwrap();
        setup_projects(&tmp, &["org/repo1", "org/repo2"]);
        let config = DevConfig::default();
        let projects = discover_projects(tmp.path(), &config);
        assert_eq!(projects.len(), 2);
        assert!(projects.iter().any(|p| p.display_name == "repo1"));
        assert!(projects.iter().any(|p| p.display_name == "repo2"));
    }

    #[test]
    fn handles_name_collisions() {
        let tmp = TempDir::new().unwrap();
        setup_projects(&tmp, &["org-a/shared", "org-b/shared"]);
        let config = DevConfig::default();
        let projects = discover_projects(tmp.path(), &config);
        let names: Vec<&str> = projects.iter().map(|p| p.display_name.as_str()).collect();
        // Colliding names get the relative path
        assert!(names.contains(&"org-a/shared"));
        assert!(names.contains(&"org-b/shared"));
    }

    #[test]
    fn respects_max_depth() {
        let tmp = TempDir::new().unwrap();
        // Depth 3 should be found
        setup_projects(&tmp, &["a/b/c"]);
        // Depth 4 should not (MAX_DEPTH = 3, we scan 0,1,2 = 3 levels)
        let deep = tmp.path().join("x/y/z/w");
        std::fs::create_dir_all(deep.join(".git")).unwrap();
        let config = DevConfig::default();
        let projects = discover_projects(tmp.path(), &config);
        assert!(projects.iter().any(|p| p.display_name == "c"));
        assert!(!projects.iter().any(|p| p.display_name == "w"));
    }

    #[test]
    fn merges_custom_path_entries() {
        let tmp = TempDir::new().unwrap();
        setup_projects(&tmp, &["alpha"]);
        let custom = TempDir::new().unwrap();
        let config = DevConfig::new(
            Layout::Default,
            None,
            [(
                "dotfiles".to_string(),
                ProjectEntry {
                    layout: Layout::Claude,
                    custom_path: Some(custom.path().to_path_buf()),
                    host: None,
                    repository: None,
                    responsibility: None,
                    worktrees: std::collections::HashMap::new(),
                },
            )]
            .into_iter()
            .collect(),
        );
        let projects = discover_projects(tmp.path(), &config);
        assert_eq!(projects.len(), 2);
        assert!(projects.iter().any(|p| p.display_name == "dotfiles"));
    }

    #[test]
    fn empty_directory() {
        let tmp = TempDir::new().unwrap();
        let config = DevConfig::default();
        let projects = discover_projects(tmp.path(), &config);
        assert!(projects.is_empty());
    }

    #[test]
    fn nonexistent_directory() {
        let config = DevConfig::default();
        let projects = discover_projects(Path::new("/nonexistent/path"), &config);
        assert!(projects.is_empty());
    }
}
