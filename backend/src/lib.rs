//! Backend library for colmap-openmvs-app
//! Contains all implementations for server functions with access to heavy native dependencies

mod config;
pub use config::{get_image_config, load_project_config, save_project_config};

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
    add_project_image, add_project_video, batch_resize_images, clear_project_images,
    clear_project_videos, delete_project_image, delete_project_video, download_demo_images,
    get_project_image_bytes, get_project_images, get_project_video_bytes, get_project_videos,
    get_video_frame_counts, ImageData, VideoData,
};

mod files;
pub use files::init as init_files;

mod outputs;
pub use outputs::{generate_glb, write_project_output};

mod init;
pub use init::startup;

mod projects;
pub use projects::{create_project, delete_project, get_projects, rename_project};

mod settings;
pub use settings::{get_settings, initialize, initialize_from_env, update_settings, CliConfig};

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

use colmap_openmvs_api::OutputFile;
use dioxus::fullstack::body::Bytes;
use dioxus::fullstack::ByteStream;
use dioxus::Result as DioxusResult;
use std::io::{Read, Write};
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
            // Skip input directories at the project root (images/ and videos/ are inputs, not outputs)
            if is_root_dir && (name == "images" || name == "videos") {
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
            let glb_available = is_viewable;

            let modified_at = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            out.push(OutputFile {
                relative_path: relative,
                name,
                size: metadata.len(),
                is_viewable,
                glb_available,
                modified_at,
            });
        }
    }

    Ok(())
}

/// Stream an output file as raw bytes — callable from the client via the Dioxus
/// server-function protocol so no URL needs to be constructed or hardcoded.
pub async fn get_project_output_bytes(
    project_name: String,
    relative_path: String,
) -> DioxusResult<ByteStream> {
    let project_path = project::resolve_project_path(&project_name).await?;
    let full_path = project::resolve_project_relative_path(&project_path, &relative_path)?;
    debug!("Streaming output file bytes: {}", full_path.display());
    let bytes = tokio::fs::read(&full_path)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read output file: {}", e))?;
    Ok(ByteStream::new(futures::stream::once(async move {
        Bytes::from(bytes)
    })))
}

/// Stream all files under `folder_path` as a ZIP archive.
pub async fn download_project_outputs_zip(
    project_name: String,
    folder_path: String,
) -> DioxusResult<ByteStream> {
    let project_path = project::resolve_project_path(&project_name).await?;
    let folder_path_clone = folder_path.clone();

    let result = tokio::task::spawn_blocking(move || {
        let base = if folder_path_clone.is_empty() {
            project_path.clone()
        } else {
            project_path.join(&folder_path_clone)
        };

        if !base.exists() || !base.is_dir() {
            return Err(anyhow::anyhow!("Directory not found: {}", base.display()));
        }

        let mut zip_buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip_writer = zip::ZipWriter::new(&mut zip_buf);
            collect_files_for_zip(&project_path, &base, &folder_path_clone, &mut zip_writer)?;
            zip_writer
                .finish()
                .map_err(|e| anyhow::anyhow!("Failed to finalize ZIP: {}", e))?;
        }
        Ok::<_, anyhow::Error>(zip_buf.into_inner())
    })
    .await
    .map_err(|e| anyhow::anyhow!("ZIP build task failed: {}", e))??;

    Ok(ByteStream::new(futures::stream::once(async move {
        Bytes::from(result)
    })))
}

fn collect_files_for_zip(
    root: &std::path::Path,
    dir: &std::path::Path,
    prefix: &str,
    zip_writer: &mut zip::ZipWriter<&mut std::io::Cursor<Vec<u8>>>,
) -> anyhow::Result<()> {
    use zip::write::FileOptions;

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");

        // Compute ZIP entry path:
        // - When prefix is empty (root backup): use relative path as-is
        // - When prefix is non-empty (folder backup): use the relative path
        //   with the folder name preserved as the root element, so that
        //   restoring on the parent folder creates the folder with its contents.
        //   Example: backing up "my_folder" yields entries like
        //   "my_folder/file.txt", "my_folder/subdir/file2.txt"
        let zip_entry = if prefix.is_empty() {
            relative.clone()
        } else {
            let prefix_with_slash = format!("{}/", prefix);
            relative
                .strip_prefix(&prefix_with_slash)
                .map(|stripped| format!("{}/{}", prefix, stripped))
                .unwrap_or_else(|| relative.clone())
        };

        if path.is_dir() {
            // Skip input directories at the project root
            if dir == root && (name == "images" || name == "videos") {
                continue;
            }
            // Skip config/settings at root
            if dir == root && (name == "config.sh" || name == "settings.json") {
                continue;
            }
            // Add directory entry
            let _ = zip_writer.add_directory::<&str, ()>(&zip_entry, FileOptions::default());
            collect_files_for_zip(root, &path, prefix, zip_writer)?;
        } else if path.is_file() {
            if dir == root && (name == "config.sh" || name == "settings.json") {
                continue;
            }
            let bytes = std::fs::read(&path)?;
            let options: FileOptions<'_, ()> =
                FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            zip_writer
                .start_file(&zip_entry, options)
                .map_err(|e| anyhow::anyhow!("ZIP entry '{}': {}", zip_entry, e))?;
            zip_writer
                .write_all(&bytes)
                .map_err(|e| anyhow::anyhow!("ZIP write '{}': {}", zip_entry, e))?;
        }
    }
    Ok(())
}

/// Restore files from a ZIP archive or single file upload into a project folder.
pub async fn restore_project_outputs(
    project_name: String,
    folder_path: String,
    body: ByteStream,
) -> DioxusResult<()> {
    let project_path = project::resolve_project_path(&project_name).await?;

    // Read all bytes from the stream
    let mut all_bytes = Vec::new();
    let mut s = body;
    while let Some(chunk) = s.next().await {
        let chunk = chunk?;
        all_bytes.extend_from_slice(&chunk);
    }

    // Detect if payload is a ZIP (magic bytes PK\x03\x04)
    if all_bytes.len() >= 4
        && all_bytes[0] == 0x50
        && all_bytes[1] == 0x4B
        && all_bytes[2] == 0x03
        && all_bytes[3] == 0x04
    {
        // ZIP extraction
        let cursor = std::io::Cursor::new(&all_bytes);
        let mut archive =
            zip::ZipArchive::new(cursor).map_err(|e| anyhow::anyhow!("Invalid ZIP: {}", e))?;

        let target_dir = if folder_path.is_empty() {
            project_path.clone()
        } else {
            project_path.join(&folder_path)
        };

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_string();
            let target_path = target_dir.join(&name);
            if let Some(parent) = target_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| anyhow::anyhow!("Create dir: {}", e))?;
            }
            let mut entry_bytes = Vec::new();
            entry
                .read_to_end(&mut entry_bytes)
                .map_err(|e| anyhow::anyhow!("Read ZIP entry '{}': {}", name, e))?;
            tokio::fs::write(&target_path, &entry_bytes)
                .await
                .map_err(|e| anyhow::anyhow!("Write '{}': {}", target_path.display(), e))?;
        }
    } else {
        // Single file: write directly
        if folder_path.is_empty() {
            return Err(anyhow::anyhow!("Single file upload requires a filename").into());
        }
        let target_path = project_path.join(&folder_path);
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| anyhow::anyhow!("Create dir: {}", e))?;
        }
        tokio::fs::write(&target_path, &all_bytes)
            .await
            .map_err(|e| anyhow::anyhow!("Write file: {}", e))?;
    }

    Ok(())
}

/// Generate and stream a GLB version of an output file.
pub async fn get_project_output_glb(
    project_name: String,
    relative_path: String,
) -> DioxusResult<ByteStream> {
    let project_path = project::resolve_project_path(&project_name).await?;
    let relative_path_clone = relative_path.clone();
    let result = tokio::task::spawn_blocking(move || {
        outputs::generate_glb(&relative_path_clone, &project_path)
    })
    .await
    .map_err(|e| anyhow::anyhow!("GLB generation task failed: {}", e))??;
    Ok(ByteStream::new(futures::stream::once(async move {
        Bytes::from(result)
    })))
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

        // Preserve input directories (images, videos) and project configuration
        if name == "images" || name == "videos" || name == "config.sh" {
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
