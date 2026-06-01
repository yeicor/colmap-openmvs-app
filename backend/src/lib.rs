//! Backend library for colmap-openmvs-app
//! Contains all implementations for server functions with access to heavy native dependencies

mod config;
pub use config::{
    get_image_config, load_named_project_config, load_project_config, save_named_project_config,
    save_project_config,
};

mod line_reader;
pub use line_reader::LineReader;

#[cfg(target_os = "android")]
mod android_startup;
#[cfg(target_os = "android")]
pub use android_startup::setup_android_runtime;

mod android_settings_validation;
pub use android_settings_validation::AndroidSettingsValidation;

mod project;
pub use project::{
    add_project_image, batch_resize_images, clear_project_images, delete_project_image,
    download_demo_images, get_project_image, get_project_image_bytes, get_project_images,
};

mod files;
pub use files::{
    init as init_files, pick_and_import_images, pick_projects_folder, pick_settings_file,
    save_output_as,
};

mod init;
pub use init::on_frontend_started;

mod projects;
pub use projects::{create_project, delete_project, get_projects, rename_project};

mod settings;
pub use settings::{get_settings, update_settings};

mod runtimes_api;
pub use runtimes_api::{
    cancel_task, delete_runtime_binary, download_runtime_version, get_available_runtime_versions,
    get_docker_runtime_info, get_embedded_image_tag, get_project_run_status, get_runtime_info,
    get_task_info, list_available_image_tags, list_docker_images, list_runtime_images, list_tasks,
    poll_task_events, prepare_docker_image, prepare_runtime_image, remove_docker_image,
    remove_runtime_image, repair_android_settings,
};

pub mod runtimes;

pub mod task_registry;
pub use task_registry::{publish_event, task_registry, TaskRegistry};

mod pipeline;
pub use pipeline::run_pipeline;

mod theme;
pub use theme::get_dark_mode;

mod process;
pub use process::kill_process_tree;

pub use output_viewer::get_project_output_for_viewer;
mod output_viewer;

pub mod ply_to_glb;

use colmap_openmvs_api::OutputFile;
use dioxus::Result as DioxusResult;
use tracing::debug;

/// List output files in a project's work directory.
/// Scans for *.ply files and known COLMAP output files (points3D.bin, database.db).
pub async fn list_project_outputs(project_name: String) -> DioxusResult<Vec<OutputFile>> {
    let root = project::resolve_project_path(&project_name).await?;

    debug!("Scanning project outputs from: {}", root.display());

    let mut outputs: Vec<OutputFile> = Vec::new();

    walk_for_outputs(&root, &root, &mut outputs)
        .map_err(|e| anyhow::anyhow!("Failed to scan project outputs: {}", e))?;

    outputs.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.relative_path.cmp(&b.relative_path))
    });
    debug!("Found {} output files", outputs.len());
    Ok(outputs)
}

/// Recursively walk `dir`, collecting output files into `out`.
fn walk_for_outputs(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<OutputFile>,
) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    let is_root_dir = dir == root;

    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        if path.is_dir() {
            // Skip the images directory at the project root (those are inputs, not outputs)
            if is_root_dir && name == "images" {
                continue;
            }
            walk_for_outputs(root, &path, out)?;
        } else if path.is_file() {
            // Skip config/settings files at the project root level
            if is_root_dir && (name == "config.sh" || name == "settings.json") {
                continue;
            }

            let metadata = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };

            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");

            let is_viewable = name.ends_with(".ply") || name == "points3D.bin";

            let modified_at = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            out.push(OutputFile {
                relative_path: relative,
                name,
                size: metadata.len(),
                is_viewable,
                modified_at,
            });
        }
    }

    Ok(())
}

/// Read an output file as raw bytes — callable from the client via the Dioxus
/// server-function protocol so no URL needs to be constructed or hardcoded.
pub async fn get_project_output_bytes(
    project_name: String,
    relative_path: String,
) -> DioxusResult<Vec<u8>> {
    let project_path = project::resolve_project_path(&project_name).await?;
    let full_path = project::resolve_project_relative_path(&project_path, &relative_path)?;
    debug!("Reading output file bytes: {}", full_path.display());
    tokio::fs::read(&full_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read output file: {}", e).into())
}

/// Read a project output file as bytes (for download or viewing).
pub async fn get_project_output(
    project_name: String,
    relative_path: String,
) -> DioxusResult<dioxus::fullstack::FileStream> {
    let project_path = project::resolve_project_path(&project_name).await?;
    let full_path = project::resolve_project_relative_path(&project_path, &relative_path)?;
    debug!("Reading output file from: {}", full_path.display());
    dioxus::fullstack::FileStream::from_path(full_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read output file: {}", e).into())
}

/// Delete an output file from a project.
pub async fn delete_project_output(
    project_name: String,
    relative_path: String,
) -> DioxusResult<()> {
    let project_path = project::resolve_project_path(&project_name).await?;
    let full_path = project::resolve_project_relative_path(&project_path, &relative_path)?;

    debug!("Deleting output file: {}", full_path.display());

    // Check if it's a file or directory
    if full_path.is_file() {
        tokio::fs::remove_file(&full_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to delete file: {}", e).into())
    } else if full_path.is_dir() {
        tokio::fs::remove_dir_all(&full_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to delete directory: {}", e).into())
    } else {
        Err(anyhow::anyhow!("Path does not exist").into())
    }
}

/// Delete all output files/directories in a project, preserving only the
/// `images/` input directory and the `config.sh` configuration file.
pub async fn clear_project_outputs(project_name: String) -> DioxusResult<()> {
    let project_path = project::resolve_project_path(&project_name).await?;
    let root = project_path.as_path();
    let entries = std::fs::read_dir(root)
        .map_err(|e| anyhow::anyhow!("Failed to read project directory: {}", e))?;

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // Preserve input images and project configuration
        if name == "images" || name == "config.sh" {
            continue;
        }

        if path.is_dir() {
            tokio::fs::remove_dir_all(&path).await.map_err(|e| {
                anyhow::anyhow!("Failed to delete directory {}: {}", path.display(), e)
            })?;
        } else if path.is_file() {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to delete file {}: {}", path.display(), e))?;
        }
    }

    debug!("Cleared all outputs for project: {}", project_name);
    Ok(())
}
