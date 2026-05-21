use anyhow::anyhow;
use colmap_openmvs_api::types::Project;
use dioxus::Result as DioxusResult;
use std::path::Path;
use tracing::{debug, error, info, warn};

pub async fn get_projects() -> DioxusResult<Vec<Project>> {
    debug!("Fetching list of all projects");
    let settings = crate::get_settings().await?;
    let projects_path = Path::new(&settings.projects_folder);
    debug!(projects_folder = %settings.projects_folder, "Projects folder resolved");

    if !projects_path.exists() {
        debug!(path = %projects_path.display(), "Projects folder does not exist, creating it");
        std::fs::create_dir_all(projects_path)
            .map_err(|e| anyhow!("Failed to create projects folder: {}", e))?;
        info!(path = %projects_path.display(), "Projects folder created successfully");
        return Ok(Vec::new());
    }

    let mut projects = Vec::new();

    match std::fs::read_dir(projects_path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if let Ok(path) = entry.path().canonicalize() {
                    if path.is_dir() {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            debug!(project_name = %name, project_path = %path.display(), "Found project");
                            projects.push(Project {
                                name: name.to_string(),
                                path: path.to_string_lossy().to_string(),
                            });
                        }
                    }
                }
            }
        }
        Err(e) => {
            error!(path = %projects_path.display(), error = %e, "Failed to read projects folder");
            return Err(anyhow!("Failed to read projects folder: {}", e).into());
        }
    }

    projects.sort_by(|a, b| a.name.cmp(&b.name));
    info!(
        count = projects.len(),
        "Successfully retrieved projects list"
    );
    Ok(projects)
}

pub async fn create_project(name: String) -> DioxusResult<Project> {
    debug!(project_name = %name, "Creating new project");

    if name.is_empty() || name.contains('/') || name.contains('\\') {
        warn!(project_name = %name, "Invalid project name provided");
        return Err(anyhow!("Invalid project name").into());
    }

    let settings = crate::get_settings().await?;
    let project_path = Path::new(&settings.projects_folder).join(&name);
    debug!(project_path = %project_path.display(), "Resolved project path");

    if project_path.exists() {
        warn!(project_name = %name, project_path = %project_path.display(), "Project already exists");
        return Err(anyhow!("Project already exists").into());
    }

    std::fs::create_dir_all(&project_path)
        .map_err(|e| {
            error!(project_name = %name, project_path = %project_path.display(), error = %e, "Failed to create project directory");
            anyhow!("Failed to create project: {}", e)
        })?;

    info!(project_name = %name, project_path = %project_path.display(), "Project created successfully");
    Ok(Project {
        name,
        path: project_path.to_string_lossy().to_string(),
    })
}

pub async fn delete_project(name: String) -> DioxusResult<()> {
    debug!(project_name = %name, "Deleting project");

    if name.is_empty() || name.contains('/') || name.contains('\\') {
        warn!(project_name = %name, "Invalid project name provided for deletion");
        return Err(anyhow!("Invalid project name").into());
    }

    let settings = crate::get_settings().await?;
    let project_path = Path::new(&settings.projects_folder).join(&name);
    debug!(project_path = %project_path.display(), "Resolved project path for deletion");

    if !project_path.exists() {
        warn!(project_name = %name, project_path = %project_path.display(), "Project not found for deletion");
        return Err(anyhow!("Project not found").into());
    }

    std::fs::remove_dir_all(&project_path)
        .map_err(|e| {
            error!(project_name = %name, project_path = %project_path.display(), error = %e, "Failed to delete project directory");
            anyhow!("Failed to delete project: {}", e)
        })?;

    info!(project_name = %name, project_path = %project_path.display(), "Project deleted successfully");
    Ok(())
}

pub async fn rename_project(name: String, new_name: String) -> DioxusResult<Project> {
    debug!(old_name = %name, new_name = %new_name, "Renaming project");

    if name.is_empty() || new_name.is_empty() {
        warn!(old_name = %name, new_name = %new_name, "Empty project name provided");
        return Err(anyhow!("Names cannot be empty").into());
    }

    if new_name.contains('/') || new_name.contains('\\') {
        warn!(old_name = %name, new_name = %new_name, "Invalid new project name provided");
        return Err(anyhow!("Invalid project name").into());
    }

    let settings = crate::get_settings().await?;
    let old_path = Path::new(&settings.projects_folder).join(&name);
    let new_path = Path::new(&settings.projects_folder).join(&new_name);
    debug!(old_path = %old_path.display(), new_path = %new_path.display(), "Resolved project paths");

    if !old_path.exists() {
        warn!(old_name = %name, old_path = %old_path.display(), "Project not found for renaming");
        return Err(anyhow!("Project not found").into());
    }

    if new_path.exists() {
        warn!(new_name = %new_name, new_path = %new_path.display(), "New project name already exists");
        return Err(anyhow!("Project with new name already exists").into());
    }

    std::fs::rename(&old_path, &new_path)
        .map_err(|e| {
            error!(old_name = %name, new_name = %new_name, old_path = %old_path.display(), new_path = %new_path.display(), error = %e, "Failed to rename project");
            anyhow!("Failed to rename project: {}", e)
        })?;

    info!(old_name = %name, new_name = %new_name, new_path = %new_path.display(), "Project renamed successfully");
    Ok(Project {
        name: new_name,
        path: new_path.to_string_lossy().to_string(),
    })
}
