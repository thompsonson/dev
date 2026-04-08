use crate::discovery::DiscoveredProject;

/// Resolve a project query to a discovered project.
///
/// Three-tier matching:
/// 1. Exact match on display name
/// 2. Exact match on directory basename
/// 3. Fuzzy substring match on display name
pub fn resolve_project<'a>(
    query: &str,
    projects: &'a [DiscoveredProject],
) -> Option<&'a DiscoveredProject> {
    // 1. Exact display name match
    if let Some(p) = projects.iter().find(|p| p.display_name == query) {
        return Some(p);
    }

    // 2. Exact basename match
    if let Some(p) = projects.iter().find(|p| {
        p.full_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == query)
            .unwrap_or(false)
    }) {
        return Some(p);
    }

    // 3. Substring match on display name
    projects.iter().find(|p| p.display_name.contains(query))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn project(name: &str, path: &str) -> DiscoveredProject {
        DiscoveredProject {
            display_name: name.to_string(),
            full_path: PathBuf::from(path),
        }
    }

    #[test]
    fn exact_display_name() {
        let projects = vec![
            project("alpha", "/projects/alpha"),
            project("beta", "/projects/beta"),
        ];
        let result = resolve_project("alpha", &projects).unwrap();
        assert_eq!(result.display_name, "alpha");
    }

    #[test]
    fn basename_match() {
        let projects = vec![project("org/myrepo", "/projects/org/myrepo")];
        let result = resolve_project("myrepo", &projects).unwrap();
        assert_eq!(result.display_name, "org/myrepo");
    }

    #[test]
    fn substring_match() {
        let projects = vec![project("my-long-project", "/projects/my-long-project")];
        let result = resolve_project("long", &projects).unwrap();
        assert_eq!(result.display_name, "my-long-project");
    }

    #[test]
    fn no_match() {
        let projects = vec![project("alpha", "/projects/alpha")];
        assert!(resolve_project("zzz", &projects).is_none());
    }

    #[test]
    fn exact_preferred_over_substring() {
        let projects = vec![
            project("chops-web", "/projects/chops-web"),
            project("chops", "/projects/chops"),
        ];
        let result = resolve_project("chops", &projects).unwrap();
        assert_eq!(result.display_name, "chops");
    }
}
