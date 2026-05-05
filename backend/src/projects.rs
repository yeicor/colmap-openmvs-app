use anyhow::anyhow;
use colmap_openmvs_api::types::Project;
use dioxus::Result as DioxusResult;
use std::path::Path;

pub async fn get_projects() -> DioxusResult<Vec<Project>> {
    let settings = crate::get_settings().await?;
    let projects_path = Path::new(&settings.projects_folder);

    if !projects_path.exists() {
        std::fs::create_dir_all(projects_path)
            .map_err(|e| anyhow!("Failed to create projects folder: {}", e))?;
        return Ok(Vec::new());
    }

    let mut projects = Vec::new();

    match std::fs::read_dir(projects_path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if let Ok(path) = entry.path().canonicalize() {
                    if path.is_dir() {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            projects.push(Project {
                                name: name.to_string(),
                                path: path.to_string_lossy().to_string(),
                            });
                        }
                    }
                }
            }
        }
        Err(e) => return Err(anyhow!("Failed to read projects folder: {}", e).into()),
    }

    projects.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(projects)
}

pub async fn create_project(name: String) -> DioxusResult<Project> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(anyhow!("Invalid project name").into());
    }

    let settings = crate::get_settings().await?;
    let project_path = Path::new(&settings.projects_folder).join(&name);

    if project_path.exists() {
        return Err(anyhow!("Project already exists").into());
    }

    std::fs::create_dir_all(&project_path)
        .map_err(|e| anyhow!("Failed to create project: {}", e))?;

    Ok(Project {
        name,
        path: project_path.to_string_lossy().to_string(),
    })
}

pub async fn delete_project(name: String) -> DioxusResult<()> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(anyhow!("Invalid project name").into());
    }

    let settings = crate::get_settings().await?;
    let project_path = Path::new(&settings.projects_folder).join(&name);

    if !project_path.exists() {
        return Err(anyhow!("Project not found").into());
    }

    std::fs::remove_dir_all(&project_path)
        .map_err(|e| anyhow!("Failed to delete project: {}", e))?;

    Ok(())
}

pub async fn rename_project(name: String, new_name: String) -> DioxusResult<Project> {
    if name.is_empty() || new_name.is_empty() {
        return Err(anyhow!("Names cannot be empty").into());
    }

    if new_name.contains('/') || new_name.contains('\\') {
        return Err(anyhow!("Invalid project name").into());
    }

    let settings = crate::get_settings().await?;
    let old_path = Path::new(&settings.projects_folder).join(&name);
    let new_path = Path::new(&settings.projects_folder).join(&new_name);

    if !old_path.exists() {
        return Err(anyhow!("Project not found").into());
    }

    if new_path.exists() {
        return Err(anyhow!("Project with new name already exists").into());
    }

    std::fs::rename(&old_path, &new_path)
        .map_err(|e| anyhow!("Failed to rename project: {}", e))?;

    Ok(Project {
        name: new_name,
        path: new_path.to_string_lossy().to_string(),
    })
}
