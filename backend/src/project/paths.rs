use anyhow::anyhow;
use std::path::{Component, Path, PathBuf};

pub fn validate_project_name(name: &str) -> dioxus::Result<()> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        Err(anyhow!("Invalid project name"))?;
    }
    Ok(())
}

pub async fn resolve_project_path(project_name: &str) -> dioxus::Result<PathBuf> {
    validate_project_name(project_name)?;

    crate::get_projects()
        .await?
        .into_iter()
        .find(|p| p.name == project_name)
        .map(|p| PathBuf::from(p.path))
        .ok_or_else(|| anyhow!("Project not found: {}", project_name).into())
}

pub fn resolve_project_relative_path(
    project_path: &Path,
    relative_path: &str,
) -> dioxus::Result<PathBuf> {
    let sanitized = relative_path.trim_start_matches('/');
    let relative = Path::new(sanitized);

    for component in relative.components() {
        if matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        ) {
            return Err(anyhow!("Path traversal is not allowed").into());
        }
    }

    Ok(project_path.join(relative))
}

pub fn project_images_path(project_name: &str, projects_folder: &str) -> dioxus::Result<PathBuf> {
    validate_project_name(project_name)?;
    Ok(Path::new(projects_folder).join(project_name).join("images"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_name_validation_rejects_empty_and_paths() {
        assert!(validate_project_name("").is_err());
        assert!(validate_project_name("../demo").is_err());
        assert!(validate_project_name("a/b").is_err());
        assert!(validate_project_name("a\\b").is_err());
        assert!(validate_project_name("demo").is_ok());
    }

    #[test]
    fn project_relative_paths_allow_leading_slash_for_legacy_callers() {
        let resolved = resolve_project_relative_path(Path::new("/tmp/project"), "/outputs/a.ply")
            .expect("path should resolve");
        assert_eq!(resolved, PathBuf::from("/tmp/project/outputs/a.ply"));
    }

    #[test]
    fn project_relative_paths_reject_parent_components() {
        assert!(
            resolve_project_relative_path(Path::new("/tmp/project"), "../settings.json").is_err()
        );
        assert!(resolve_project_relative_path(Path::new("/tmp/project"), "a/../../b").is_err());
    }
}
