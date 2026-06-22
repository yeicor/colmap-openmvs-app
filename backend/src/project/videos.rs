use anyhow::{anyhow, Context};
use dioxus::fullstack::ByteStream;
use once_cell::sync::Lazy;
use tracing::{debug, error, info, warn};

use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use std::collections::HashMap;

// Type alias for video lock map
type VideoLocksMap = Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>;

// Global lock map for video operations (per video path)
static VIDEO_LOCKS: Lazy<VideoLocksMap> = Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

async fn lock_for_video_path<P: AsRef<Path>>(path: P) -> Arc<Mutex<()>> {
    let path_str = path.as_ref().to_string_lossy().to_string();
    let mut map = VIDEO_LOCKS.lock().await;
    map.entry(path_str)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// List video files in a project's videos/ directory.
pub async fn get_project_videos(project_name: String) -> dioxus::Result<Vec<String>> {
    debug!(project_name = %project_name, "Retrieving project videos list");
    let settings = crate::get_settings().await?;
    let videos_path =
        crate::project::project_videos_path(&project_name, &settings.projects_folder)?;
    debug!(videos_path = %videos_path.display(), "Resolved videos directory path");

    if !videos_path.exists() {
        debug!(videos_path = %videos_path.display(), "Videos directory does not exist");
        return Ok(Vec::new());
    }

    let mut videos = Vec::new();
    let entries = std::fs::read_dir(&videos_path).context("Failed to read videos directory")?;
    for entry in entries.flatten() {
        if let Ok(path) = entry.path().canonicalize() {
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if is_video_file(name) {
                        debug!(video_name = %name, "Found video file");
                        videos.push(name.to_string());
                    }
                }
            }
        }
    }

    videos.sort();
    info!(project_name = %project_name, video_count = videos.len(), "Successfully retrieved videos list");
    Ok(videos)
}

/// Upload a video file to a project's videos/ directory.
pub async fn add_project_video(
    project_name: String,
    video_name: String,
    mut body: ByteStream,
) -> dioxus::Result<()> {
    debug!(project_name = %project_name, video_name = %video_name, "Adding video to project");
    validate_video_name(&video_name)?;
    let settings = crate::get_settings().await?;
    let videos_path =
        crate::project::project_videos_path(&project_name, &settings.projects_folder)?;
    debug!(videos_path = %videos_path.display(), "Resolved videos directory");

    std::fs::create_dir_all(&videos_path)
        .map_err(|e| anyhow!("Failed to create videos folder: {}", e))?;

    let canonical_base = videos_path
        .canonicalize()
        .map_err(|e| anyhow!("Failed to resolve videos directory: {}", e))?;

    let video_path = videos_path.join(&video_name);
    let canonical_dest = std::path::PathBuf::from(&video_path);

    if !canonical_dest.starts_with(&canonical_base) && canonical_dest.canonicalize().is_ok() {
        warn!(video_name = %video_name, "Path traversal attempt detected");
        Err(anyhow!("Access denied: path traversal attempt detected"))?
    }

    let lock = lock_for_video_path(&video_path).await;
    let _guard = lock.lock().await;

    let mut video_bytes = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        video_bytes.extend_from_slice(&chunk);
    }

    debug!(video_path = %video_path.display(), body_size = video_bytes.len(), "Writing video file");
    std::fs::write(&video_path, &video_bytes).map_err(|e| {
        error!(video_path = %video_path.display(), error = %e, "Failed to write video file");
        anyhow!("Failed to write video file: {}", e)
    })?;
    info!(project_name = %project_name, video_name = %video_name, video_path = %video_path.display(), body_size = video_bytes.len(), "Video added successfully");

    Ok(())
}

/// Delete a video file from a project's videos/ directory.
pub async fn delete_project_video(project_name: String, video_name: String) -> dioxus::Result<()> {
    debug!(project_name = %project_name, video_name = %video_name, "Deleting video from project");
    let settings = crate::get_settings().await?;
    let videos_path =
        crate::project::project_videos_path(&project_name, &settings.projects_folder)?;
    debug!(videos_path = %videos_path.display(), "Resolved videos directory");

    let canonical_video = validate_and_canonicalize_video_path(&videos_path, &video_name)?;
    let lock = lock_for_video_path(&canonical_video).await;
    let _guard = lock.lock().await;

    debug!(video_path = %canonical_video.display(), "Removing video file");
    std::fs::remove_file(&canonical_video).map_err(|e| {
        error!(video_path = %canonical_video.display(), error = %e, "Failed to delete video");
        anyhow!("Failed to delete video: {}", e)
    })?;
    info!(project_name = %project_name, video_name = %video_name, "Video deleted successfully");

    Ok(())
}

/// Clear all video files from a project's videos/ directory.
pub async fn clear_project_videos(project_name: String) -> dioxus::Result<()> {
    debug!(project_name = %project_name, "Clearing all videos from project");
    let settings = crate::get_settings().await?;
    let videos_path =
        crate::project::project_videos_path(&project_name, &settings.projects_folder)?;
    debug!(videos_path = %videos_path.display(), "Resolved videos directory");

    if videos_path.exists() {
        debug!("Videos directory exists, removing it");
        std::fs::remove_dir_all(&videos_path)
            .map_err(|e| {
                error!(videos_path = %videos_path.display(), error = %e, "Failed to clear videos directory");
                anyhow!("Failed to clear videos: {}", e)
            })?;
        info!(project_name = %project_name, "All project videos cleared successfully");
    } else {
        debug!(videos_path = %videos_path.display(), "Videos directory does not exist");
    }

    Ok(())
}

/// Helper function to safely canonicalize and validate video paths
fn validate_and_canonicalize_video_path(
    videos_path: &Path,
    video_name: &str,
) -> dioxus::Result<std::path::PathBuf> {
    validate_video_name(video_name)?;

    let canonical_base = videos_path
        .canonicalize()
        .map_err(|e| anyhow!("Failed to resolve videos directory: {}", e))?;

    let video_path = videos_path.join(video_name);
    let canonical_video = video_path
        .canonicalize()
        .map_err(|e| anyhow!("Video not found or inaccessible: {}", e))?;

    if !canonical_video.starts_with(&canonical_base) {
        Err(anyhow!("Access denied: path traversal attempt detected"))?;
    }

    if !canonical_video.is_file() {
        Err(anyhow!("Video file not found"))?;
    }

    Ok(canonical_video)
}

fn validate_video_name(name: &str) -> dioxus::Result<()> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        Err(anyhow!("Invalid video name"))?;
    }
    if !is_video_file(name) {
        Err(anyhow!("Invalid video file type"))?;
    }
    Ok(())
}

fn is_video_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".mp4")
        || lower.ends_with(".webm")
        || lower.ends_with(".mkv")
        || lower.ends_with(".avi")
        || lower.ends_with(".mov")
        || lower.ends_with(".m4v")
        || lower.ends_with(".mpg")
        || lower.ends_with(".mpeg")
        || lower.ends_with(".wmv")
        || lower.ends_with(".flv")
        || lower.ends_with(".3gp")
}
