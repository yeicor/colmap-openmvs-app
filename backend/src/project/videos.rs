use anyhow::{anyhow, Context};
use dioxus::fullstack::body::Bytes;
use dioxus::fullstack::ByteStream;
use futures::Stream;
use once_cell::sync::Lazy;
use std::io;
use std::pin::Pin;
use std::time::UNIX_EPOCH;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream as TokioReaderStream;
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

    // Stream directly to disk instead of accumulating in memory.
    // Write to a temporary file first, then rename on success.
    let temp_path = video_path.with_extension(format!("{}.tmp", std::process::id()));
    debug!(
        video_path = %video_path.display(),
        temp_path = %temp_path.display(),
        "Opening temporary file for streaming video upload"
    );
    let mut file = tokio::fs::File::create(&temp_path).await.map_err(|e| {
        error!(temp_path = %temp_path.display(), error = %e, "Failed to create temp file");
        anyhow!("Failed to create temp file: {}", e)
    })?;

    let mut total: u64 = 0;
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        total += chunk.len() as u64;
        file.write_all(&chunk).await.map_err(|e| {
            error!(temp_path = %temp_path.display(), error = %e, "Failed to write chunk");
            anyhow!("Failed to write video chunk: {}", e)
        })?;
    }

    file.flush().await.map_err(|e| {
        error!(temp_path = %temp_path.display(), error = %e, "Failed to flush temp file");
        anyhow!("Failed to flush video file: {}", e)
    })?;
    drop(file);

    debug!(video_path = %video_path.display(), body_size = total, "Renaming temp file to final path");
    tokio::fs::rename(&temp_path, &video_path).await.map_err(|e| {
        error!(temp_path = %temp_path.display(), video_path = %video_path.display(), error = %e, "Failed to rename temp file");
        anyhow!("Failed to finalize video file: {}", e)
    })?;

    info!(project_name = %project_name, video_name = %video_name, video_path = %video_path.display(), body_size = total, "Video added successfully");

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

/// Determine the MIME type for a video file from its extension.
pub fn mime_type_for_video(ext: Option<&str>) -> &'static str {
    match ext.map(|e| e.to_lowercase()).as_deref() {
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("avi") => "video/x-msvideo",
        Some("mov") => "video/quicktime",
        Some("m4v") => "video/x-m4v",
        Some("mpg") | Some("mpeg") => "video/mpeg",
        Some("wmv") => "video/x-ms-wmv",
        Some("flv") => "video/x-flv",
        Some("3gp") => "video/3gpp",
        _ => "application/octet-stream",
    }
}

/// Raw video data ready to be streamed to the client.
pub struct VideoData {
    pub name: String,
    pub size: u64,
    pub mime: String,
    pub etag: String,
    pub stream: Pin<Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send>>,
}

/// Stream a video file from disk, returning its raw components.
/// Similar to `get_project_image_bytes` but for video files.
pub async fn get_project_video_bytes(
    project_name: String,
    video_name: String,
) -> dioxus::Result<VideoData> {
    debug!(
        project_name = %project_name,
        video_name = %video_name,
        "Streaming project video"
    );
    let settings = crate::get_settings().await?;
    let videos_path =
        crate::project::project_videos_path(&project_name, &settings.projects_folder)?;
    let canonical_video = validate_and_canonicalize_video_path(&videos_path, &video_name)?;
    let lock = lock_for_video_path(&canonical_video).await;
    let _guard = lock.lock().await;

    let ext = canonical_video.extension().and_then(|s| s.to_str());
    let mime = mime_type_for_video(ext);

    // Stat + ETag from mtime & size
    let metadata = tokio::fs::metadata(&canonical_video)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to stat {}: {}", canonical_video.display(), e))?;
    let size = metadata.len();
    let modified = metadata
        .modified()
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let mtime = modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let etag = format!("\"{:x}-{:x}\"", mtime, size);

    let file = tokio::fs::File::open(&canonical_video)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to open {}: {}", canonical_video.display(), e))?;
    let stream: Pin<Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send>> =
        Box::pin(TokioReaderStream::new(file));

    let name = canonical_video
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("video")
        .to_string();

    Ok(VideoData {
        name,
        size,
        mime: mime.to_string(),
        etag,
        stream,
    })
}
