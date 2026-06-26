use anyhow::anyhow;
use colmap_openmvs_api::types::{EnvVarConfig, Project, SavedProjectConfig};
use dioxus::Result as DioxusResult;
use std::path::Path;
use tokio::fs;
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

    // Get the effective settings file path to filter it out from projects
    let settings_path = crate::settings::get_effective_settings_path(&settings);

    match std::fs::read_dir(projects_path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if let Ok(path) = entry.path().canonicalize() {
                    // Skip if this is the settings file
                    if path == settings_path
                        || path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| n == "settings.json")
                            .unwrap_or(false)
                    {
                        debug!(path = %path.display(), "Skipping settings file from project list");
                        continue;
                    }

                    if path.is_dir() {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            debug!(project_name = %name, project_path = %path.display(), "Found project");
                            let size = compute_dir_size(&path).await;
                            projects.push(Project {
                                name: name.to_string(),
                                path: path.to_string_lossy().to_string(),
                                size,
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

/// Recursively compute the total size of a directory in bytes.
async fn compute_dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(mut entries) = fs::read_dir(path).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let metadata = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };
            if metadata.is_file() {
                total += metadata.len();
            } else if metadata.is_dir() {
                total += Box::pin(compute_dir_size(&entry.path())).await;
            }
        }
    }
    total
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

    // On Android devices write a default config.sh with conservative settings
    // that prioritise stability on resource-constrained hardware (single-thread,
    // reduced quality, lower memory usage).
    if cfg!(target_os = "android") {
        if let Err(e) = write_low_resource_project_config(&project_path, &settings).await {
            warn!(
                project_name = %name,
                error = %e,
                "Failed to write default config.sh, continuing without it"
            );
        } else {
            debug!(
                project_name = %name,
                "Default config.sh written successfully"
            );
        }
    }

    info!(project_name = %name, project_path = %project_path.display(), "Project created successfully");
    Ok(Project {
        name,
        path: project_path.to_string_lossy().to_string(),
        size: 0,
    })
}

/// Write a default config.sh with conservative settings for low-resource / stability-oriented devices.
/// Only called on Android builds.
async fn write_low_resource_project_config(
    project_path: &Path,
    settings: &colmap_openmvs_api::Settings,
) -> DioxusResult<()> {
    let image_tag = settings
        .parse_default_image()
        .1
        .unwrap_or("unknown")
        .to_string();

    let config = SavedProjectConfig {
        image_tag,
        environment_variables: vec![
            EnvVarConfig {
                name: "COLMAP_FEATURE_EXTRACTOR_ARGS".into(),
                value: "--FeatureExtraction.num_threads=1".into(),
            },
            EnvVarConfig {
                name: "COLMAP_MATCHER".into(),
                value: "exhaustive_matcher".into(), // vocab_tree may require instructions that some arm64 CPUs don't support, causing crashes.
            },
            EnvVarConfig {
                name: "COLMAP_MATCHER_ARGS".into(),
                value: "--FeatureMatching.num_threads=1".into(),
            },
            EnvVarConfig {
                name: "COLMAP_MAPPER_ARGS".into(),
                value: "--GlobalMapper.num_threads=1".into(),
            },
            EnvVarConfig {
                name: "COLMAP_UNDISTORTER_ARGS".into(),
                value: "--num_threads=1".into(),
            },
            EnvVarConfig {
                name: "OPENMVS_INTERFACE_COLMAP_ARGS".into(),
                value: "--max-threads=1".into(),
            },
            EnvVarConfig {
                name: "OPENMVS_DENSIFY_POINT_CLOUD_ARGS".into(),
                value: "--max-threads=1".into(),
            },
            EnvVarConfig {
                name: "OPENMVS_RECONSTRUCT_MESH_SPARSE_ARGS".into(),
                value: "--max-threads=1".into(),
            },
            EnvVarConfig {
                name: "OPENMVS_REFINE_MESH_SPARSE_ARGS".into(),
                value: "--max-threads=1".into(),
            },
            EnvVarConfig {
                name: "OPENMVS_TEXTURE_MESH_SPARSE_ARGS".into(),
                value: "--max-threads=1".into(),
            },
            EnvVarConfig {
                name: "OPENMVS_RECONSTRUCT_MESH_DENSE_ARGS".into(),
                value: "--max-threads=1".into(),
            },
            EnvVarConfig {
                name: "OPENMVS_REFINE_MESH_DENSE_ARGS".into(),
                value: "--max-threads=1".into(),
            },
            EnvVarConfig {
                name: "OPENMVS_TEXTURE_MESH_DENSE_ARGS".into(),
                value: "--max-threads=1".into(),
            },
        ],
        custom_script: None,
    };

    crate::config::save_project_config_by_path(project_path.to_string_lossy().into_owned(), config)
        .await?;

    Ok(())
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
    let size = compute_dir_size(&new_path).await;
    Ok(Project {
        name: new_name,
        path: new_path.to_string_lossy().to_string(),
        size,
    })
}
