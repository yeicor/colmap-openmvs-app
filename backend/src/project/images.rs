use anyhow::{anyhow, Context};
use dioxus::fullstack::body::Bytes;
use dioxus::fullstack::ByteStream;
use futures::stream::StreamExt;
use image::{DynamicImage, ImageDecoder, ImageReader};
use once_cell::sync::Lazy;
use tracing::{debug, error, info, warn};

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use colmap_openmvs_api::TaskKind;
use colmap_openmvs_api::{DemoProgressEvent, ResizeProgressEvent};

// Type alias for complex image locks structure
type ImageLocksMap = Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>;

// Global lock map for image operations (per image path)
static IMAGE_LOCKS: Lazy<ImageLocksMap> = Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

async fn lock_for_image_path<P: AsRef<Path>>(path: P) -> Arc<Mutex<()>> {
    let path_str = path.as_ref().to_string_lossy().to_string();
    let mut map = IMAGE_LOCKS.lock().await;
    map.entry(path_str)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Helper function to safely canonicalize and validate image paths
fn validate_and_canonicalize_image_path(
    images_path: &Path,
    image_name: &str,
) -> dioxus::Result<std::path::PathBuf> {
    validate_image_name(image_name)?;

    let canonical_base = images_path
        .canonicalize()
        .map_err(|e| anyhow!("Failed to resolve images directory: {}", e))?;

    let image_path = images_path.join(image_name);
    let canonical_image = image_path
        .canonicalize()
        .map_err(|e| anyhow!("Image not found or inaccessible: {}", e))?;

    if !canonical_image.starts_with(&canonical_base) {
        Err(anyhow!("Access denied: path traversal attempt detected"))?;
    }

    if !canonical_image.is_file() {
        Err(anyhow!("Image file not found"))?;
    }

    Ok(canonical_image)
}

pub async fn get_project_images(project_name: String) -> dioxus::Result<Vec<String>> {
    debug!(project_name = %project_name, "Retrieving project images list");
    let settings = crate::get_settings().await?;
    let images_path =
        crate::project::project_images_path(&project_name, &settings.projects_folder)?;
    debug!(images_path = %images_path.display(), "Resolved images directory path");

    let lock = lock_for_image_path(&images_path).await;
    let _guard = lock.lock().await;

    if !images_path.exists() {
        debug!(images_path = %images_path.display(), "Images directory does not exist, creating it");
        std::fs::create_dir_all(&images_path)
            .map_err(|e| anyhow!("Failed to create images folder: {}", e))?;
        info!(images_path = %images_path.display(), project_name = %project_name, "Images directory created");
        return Ok(Vec::new());
    }

    let mut images = Vec::new();
    let entries = std::fs::read_dir(&images_path).context("Failed to read images directory")?;
    for entry in entries.flatten() {
        if let Ok(path) = entry.path().canonicalize() {
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if is_image_file(name) {
                        debug!(image_name = %name, "Found image file");
                        images.push(name.to_string());
                    }
                }
            }
        }
    }

    images.sort();
    info!(project_name = %project_name, image_count = images.len(), "Successfully retrieved images list");
    Ok(images)
}

/// Return the raw bytes of a project image as a streaming `ByteStream`.
///
/// Unlike `get_project_image` (which streams via `FileStream` for direct HTTP
/// access), this function returns a `ByteStream` so it can be called as a
/// Dioxus server function from the client.  The bytes are then used to build a
/// Blob URL in the browser without any hardcoded server URL.
pub async fn get_project_image_bytes(
    project_name: String,
    image_name: String,
) -> dioxus::Result<ByteStream> {
    debug!(project_name = %project_name, image_name = %image_name, "Retrieving project image bytes");
    let settings = crate::get_settings().await?;
    let images_path =
        crate::project::project_images_path(&project_name, &settings.projects_folder)?;
    let canonical_image = validate_and_canonicalize_image_path(&images_path, &image_name)?;
    let lock = lock_for_image_path(&canonical_image).await;
    let _guard = lock.lock().await;
    let bytes = tokio::fs::read(&canonical_image)
        .await
        .map_err(|e| anyhow!("Failed to read image file: {}", e))?;
    Ok(ByteStream::new(futures::stream::once(async move {
        Bytes::from(bytes)
    })))
}

pub async fn add_project_image(
    project_name: String,
    image_name: String,
    mut body: ByteStream,
) -> dioxus::Result<()> {
    debug!(project_name = %project_name, image_name = %image_name, "Adding image to project");
    validate_image_name(&image_name)?;
    let settings = crate::get_settings().await?;
    let images_path =
        crate::project::project_images_path(&project_name, &settings.projects_folder)?;
    debug!(images_path = %images_path.display(), "Resolved images directory");

    std::fs::create_dir_all(&images_path)
        .map_err(|e| anyhow!("Failed to create images folder: {}", e))?;

    let canonical_base = images_path
        .canonicalize()
        .map_err(|e| anyhow!("Failed to resolve images directory: {}", e))?;

    let image_path = images_path.join(&image_name);
    let canonical_dest = std::path::PathBuf::from(&image_path);

    if !canonical_dest.starts_with(&canonical_base) && canonical_dest.canonicalize().is_ok() {
        warn!(image_name = %image_name, "Path traversal attempt detected");
        Err(anyhow!("Access denied: path traversal attempt detected"))?
    }

    let lock = lock_for_image_path(&image_path).await;
    let _guard = lock.lock().await;

    let mut image_bytes = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        image_bytes.extend_from_slice(&chunk);
    }

    debug!(image_path = %image_path.display(), body_size = image_bytes.len(), "Writing image file");
    std::fs::write(&image_path, &image_bytes).map_err(|e| {
        error!(image_path = %image_path.display(), error = %e, "Failed to write image file");
        anyhow!("Failed to write image file: {}", e)
    })?;
    info!(project_name = %project_name, image_name = %image_name, image_path = %image_path.display(), body_size = image_bytes.len(), "Image added successfully");

    Ok(())
}

pub async fn delete_project_image(project_name: String, image_name: String) -> dioxus::Result<()> {
    debug!(project_name = %project_name, image_name = %image_name, "Deleting image from project");
    let settings = crate::get_settings().await?;
    let images_path =
        crate::project::project_images_path(&project_name, &settings.projects_folder)?;
    debug!(images_path = %images_path.display(), "Resolved images directory");

    let canonical_image = validate_and_canonicalize_image_path(&images_path, &image_name)?;
    let lock = lock_for_image_path(&canonical_image).await;
    let _guard = lock.lock().await;

    debug!(image_path = %canonical_image.display(), "Removing image file");
    std::fs::remove_file(&canonical_image).map_err(|e| {
        error!(image_path = %canonical_image.display(), error = %e, "Failed to delete image");
        anyhow!("Failed to delete image: {}", e)
    })?;
    info!(project_name = %project_name, image_name = %image_name, "Image deleted successfully");

    Ok(())
}

pub async fn clear_project_images(project_name: String) -> dioxus::Result<()> {
    debug!(project_name = %project_name, "Clearing all images from project");
    let settings = crate::get_settings().await?;
    let images_path =
        crate::project::project_images_path(&project_name, &settings.projects_folder)?;
    debug!(images_path = %images_path.display(), "Resolved images directory");

    if images_path.exists() {
        debug!("Images directory exists, removing it");
        std::fs::remove_dir_all(&images_path)
            .map_err(|e| {
                error!(images_path = %images_path.display(), error = %e, "Failed to clear images directory");
                anyhow!("Failed to clear images: {}", e)
            })?;
        info!(project_name = %project_name, "All project images cleared successfully");
    } else {
        debug!(images_path = %images_path.display(), "Images directory does not exist");
    }

    Ok(())
}

/// Batch resize images with streaming progress events
pub async fn batch_resize_images(
    project_name: String,
    max_dimension: u32,
) -> dioxus::Result<String> {
    debug!(project_name = %project_name, max_dimension = max_dimension, "Starting batch image resize");
    crate::project::validate_project_name(&project_name)?;

    if !(64..=8192).contains(&max_dimension) {
        warn!(max_dimension = max_dimension, "Invalid max dimension value");
        Err(anyhow!("Max dimension must be between 64 and 8192 pixels"))?
    }

    let task_id = crate::task_registry::TASK_REGISTRY
        .create_task(TaskKind::BatchResize, project_name.clone());
    info!(task_id = %task_id, project_name = %project_name, "Batch resize task created");

    let task_id_clone = task_id.clone();
    let project_name_clone = project_name.clone();
    tokio::spawn(async move {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<ResizeProgressEvent>();
        let proj = project_name_clone.clone();
        tokio::spawn(async move {
            let _ = batch_resize_images_stream(proj, max_dimension, tx).await;
        });
        while let Some(event) = rx.next().await {
            let is_error = matches!(event, ResizeProgressEvent::Error { .. });
            debug!(error = is_error, "Processing resize progress event");
            crate::task_registry::publish_event(
                &task_id_clone,
                colmap_openmvs_api::TaskEvent::ResizeProgress(event),
            );
            if is_error {
                error!("Resize operation encountered an error");
                crate::task_registry::publish_event(
                    &task_id_clone,
                    colmap_openmvs_api::TaskEvent::Failed("Resize failed.".to_string()),
                );
                return;
            }
        }
        info!(task_id = %task_id_clone, "Resize operation completed successfully");
        crate::task_registry::publish_event(
            &task_id_clone,
            colmap_openmvs_api::TaskEvent::Completed,
        );
    });

    Ok(task_id)
}

/// Download demo images with streaming progress events
/// Available demo dataset sources from GitHub
const DEMO_SOURCES: &[(&str, &str, &str)] = &[
    ("ET", "snavely/bundler_sfm", "examples/ET"),
    ("kermit", "snavely/bundler_sfm", "examples/kermit"),
];

pub async fn download_demo_images(
    project_name: String,
    source_id: String,
) -> dioxus::Result<String> {
    debug!(project_name = %project_name, source_id = %source_id, "Starting demo image download");
    crate::project::validate_project_name(&project_name)?;

    let task_id = crate::task_registry::TASK_REGISTRY
        .create_task(TaskKind::DownloadDemo, project_name.clone());
    info!(task_id = %task_id, project_name = %project_name, "Demo download task created");

    let task_id_clone = task_id.clone();
    let project_name_clone = project_name.clone();
    tokio::spawn(async move {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<DemoProgressEvent>();
        let proj = project_name_clone.clone();
        let src = source_id.clone();
        tokio::spawn(async move {
            if let Err(e) = download_github_images_stream(proj, src, tx.clone()).await {
                let _ = tx.unbounded_send(DemoProgressEvent::Error {
                    message: format!("Download failed: {}", e),
                });
            }
        });
        while let Some(event) = rx.next().await {
            let is_error = matches!(event, DemoProgressEvent::Error { .. });
            debug!(event = ?event, "Processing demo progress event");
            crate::task_registry::publish_event(
                &task_id_clone,
                colmap_openmvs_api::TaskEvent::DemoProgress(event),
            );
            if is_error {
                error!("Demo download operation encountered an error");
                crate::task_registry::publish_event(
                    &task_id_clone,
                    colmap_openmvs_api::TaskEvent::Failed("Demo download failed.".to_string()),
                );
                return;
            }
        }
        info!(task_id = %task_id_clone, "Demo download completed successfully");
        crate::task_registry::publish_event(
            &task_id_clone,
            colmap_openmvs_api::TaskEvent::Completed,
        );
    });

    Ok(task_id)
}

#[derive(serde::Deserialize)]
struct GitHubEntry {
    name: String,
    #[serde(rename = "type")]
    entry_type: String,
    download_url: Option<String>,
}

async fn download_github_images_stream(
    project_name: String,
    source_id: String,
    tx: futures::channel::mpsc::UnboundedSender<DemoProgressEvent>,
) -> dioxus::Result<()> {
    let source = DEMO_SOURCES
        .iter()
        .find(|(id, _, _)| *id == source_id)
        .ok_or_else(|| anyhow!("Unknown demo source: {}", source_id))?;

    let (_, repo, path) = *source;

    let settings = crate::get_settings().await?;
    let images_path = Path::new(&settings.projects_folder)
        .join(&project_name)
        .join("images")
        .to_path_buf();

    let lock = lock_for_image_path(&images_path).await;
    let _guard = lock.lock().await;

    std::fs::create_dir_all(&images_path)
        .map_err(|e| anyhow!("Failed to create images folder: {}", e))?;

    let _ = tx.unbounded_send(DemoProgressEvent::FetchingFileList);

    let api_url = format!("https://api.github.com/repos/{}/contents/{}", repo, path);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent("colmap-openmvs-app/1.0")
        .build()
        .into();

    let response = agent
        .get(&api_url)
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| anyhow!("Failed to fetch GitHub directory listing: {}", e))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.into_body().read_to_string().unwrap_or_default();
        let _ = tx.unbounded_send(DemoProgressEvent::Error {
            message: format!("GitHub API returned {}: {}", status, body),
        });
        return Ok(());
    }

    let body = response
        .into_body()
        .read_to_vec()
        .map_err(|e| anyhow!("Failed to read GitHub API response: {}", e))?;
    let entries: Vec<GitHubEntry> = serde_json::from_slice(&body)
        .map_err(|e| anyhow!("Failed to parse GitHub API response: {}", e))?;

    let jpg_files: Vec<GitHubEntry> = entries
        .into_iter()
        .filter(|e| e.entry_type == "file" && e.name.to_lowercase().ends_with(".jpg"))
        .collect();

    if jpg_files.is_empty() {
        let _ = tx.unbounded_send(DemoProgressEvent::Error {
            message: "No JPEG images found in the selected dataset".to_string(),
        });
        return Ok(());
    }

    let total = jpg_files.len();
    info!(total = total, source_id = %source_id, "Found images to download");

    for (idx, entry) in jpg_files.into_iter().enumerate() {
        let download_url = match entry.download_url {
            Some(url) => url,
            None => {
                warn!(name = %entry.name, "No download URL for file, skipping");
                continue;
            }
        };

        let _ = tx.unbounded_send(DemoProgressEvent::DownloadProgress {
            filename: entry.name.clone(),
            downloaded: idx,
            total,
        });

        let data = match agent.get(&download_url).call() {
            Ok(resp) => resp
                .into_body()
                .read_to_vec()
                .map_err(|e| anyhow!("Failed to read {}: {}", entry.name, e)),
            Err(e) => Err(anyhow!("Failed to download {}: {}", entry.name, e)),
        };

        let data = match data {
            Ok(bytes) => bytes,
            Err(e) => {
                let _ = tx.unbounded_send(DemoProgressEvent::Error {
                    message: e.to_string(),
                });
                return Ok(());
            }
        };

        let dest = images_path.join(&entry.name);
        if let Err(e) = tokio::fs::write(&dest, &data).await {
            let _ = tx.unbounded_send(DemoProgressEvent::Error {
                message: format!("Failed to save {}: {}", entry.name, e),
            });
            return Ok(());
        }

        info!(name = %entry.name, progress = idx + 1, total = total, "Downloaded demo image");
    }

    // Signal completion
    let _ = tx.unbounded_send(DemoProgressEvent::DownloadProgress {
        filename: String::new(),
        downloaded: total,
        total,
    });

    Ok(())
}

/// Helper function to resize a single image file
async fn resize_image_file(image_path: &Path, max_dimension: u32) -> dioxus::Result<bool> {
    debug!(image_path = %image_path.display(), max_dimension = max_dimension, "Starting image resize");

    let decoder = ImageReader::open(image_path)
        .map_err(|e| {
            error!(image_path = %image_path.display(), error = %e, "Failed to open image file");
            anyhow!(
                "Failed to open image file: {} ({})",
                image_path.display(),
                e
            )
        })?
        .with_guessed_format()
        .map_err(|e| {
            error!(image_path = %image_path.display(), error = %e, "Failed to guess image format");
            anyhow!(
                "Failed to guess image format: {} ({})",
                image_path.display(),
                e
            )
        })?
        .into_decoder()
        .map_err(|e| {
            error!(image_path = %image_path.display(), error = %e, "Failed to create image decoder");
            anyhow!(
                "Failed to create image decoder: {} ({})",
                image_path.display(),
                e
            )
        })?;

    let (width, height) = decoder.dimensions();
    debug!(image_path = %image_path.display(), width = width, height = height, "Image dimensions determined");

    if width <= max_dimension && height <= max_dimension {
        debug!(image_path = %image_path.display(), width = width, height = height, max_dimension = max_dimension, "Image already within size limit");
        return Ok(false); // No resizing needed
    }

    let (new_width, new_height) = if width > height {
        let new_w = max_dimension;
        let new_h = ((height as f64 / width as f64) * max_dimension as f64).max(1.0) as u32;
        (new_w, new_h)
    } else {
        let new_h = max_dimension;
        let new_w = ((width as f64 / height as f64) * max_dimension as f64).max(1.0) as u32;
        (new_w, new_h)
    };
    debug!(image_path = %image_path.display(), old_width = width, old_height = height, new_width = new_width, new_height = new_height, "Calculated new dimensions");

    let img = DynamicImage::from_decoder(decoder).map_err(|e| {
        error!(image_path = %image_path.display(), error = %e, "Failed to decode image for resizing");
        anyhow!(
            "Failed to decode image for resizing: {} ({})",
            image_path.display(),
            e
        )
    })?;
    debug!(image_path = %image_path.display(), "Resizing image");
    let resized = img.resize_exact(new_width, new_height, image::imageops::FilterType::Lanczos3);
    // Write resized image directly to file without buffering in RAM
    let file = std::fs::File::create(image_path).map_err(|e| {
        error!(image_path = %image_path.display(), error = %e, "Failed to create resized image file");
        anyhow!(
            "Failed to create resized image file: {} ({})",
            image_path.display(),
            e
        )
    })?;
    resized
        .write_to(&mut std::io::BufWriter::new(file), image::ImageFormat::Jpeg)
        .map_err(|e| {
            error!(image_path = %image_path.display(), error = %e, "Failed to encode JPEG");
            anyhow!("Failed to encode JPEG: {}", e)
        })?;

    info!(image_path = %image_path.display(), "Image resized successfully");
    Ok(true)
}

async fn batch_resize_images_stream(
    project_name: String,
    max_dimension: u32,
    tx: futures::channel::mpsc::UnboundedSender<ResizeProgressEvent>,
) -> dioxus::Result<()> {
    let settings = crate::get_settings().await?;
    let images_path = Path::new(&settings.projects_folder)
        .join(&project_name)
        .join("images");

    if !images_path.exists() {
        Err(anyhow!("Images directory not found"))?;
    }

    let mut image_files = Vec::new();
    match std::fs::read_dir(&images_path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if let Ok(path) = entry.path().canonicalize() {
                    if path.is_file() {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            if is_image_file(name) {
                                image_files.push((name.to_string(), path));
                            }
                        }
                    }
                }
            }
        }
        Err(e) => Err(anyhow!("Failed to read images folder: {}", e))?,
    }

    let total_files = image_files.len();
    let _ = tx.unbounded_send(ResizeProgressEvent::ResizeProgress {
        name: String::new(),
        completed: 0,
        total_files,
    });

    let mut completed = 0;
    for (image_name, image_path) in image_files {
        let lock = lock_for_image_path(&image_path).await;
        let _guard = lock.lock().await;

        match resize_image_file(&image_path, max_dimension).await {
            Ok(_) => {
                completed += 1;
                let _ = tx.unbounded_send(ResizeProgressEvent::ResizeProgress {
                    name: image_name,
                    completed,
                    total_files,
                });
            }
            Err(e) => {
                eprintln!("[Batch Resize] Error resizing {}: {}", image_name, e);
            }
        }
    }

    Ok(())
}

fn validate_image_name(name: &str) -> dioxus::Result<()> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        Err(anyhow!("Invalid image name"))?;
    }
    if !is_image_file(name) {
        Err(anyhow!("Invalid image file type"))?;
    }
    Ok(())
}

fn is_image_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
        || lower.ends_with(".bmp")
        || lower.ends_with(".gif")
        || lower.ends_with(".webp")
        || lower.ends_with(".tiff")
}
