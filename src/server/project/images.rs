use dioxus::core::anyhow;
use dioxus::fullstack::response::IntoResponse;
use dioxus::prelude::*;
use dioxus_fullstack::payloads::sse::ServerEvents;
#[cfg(feature = "server")]
use futures_util::StreamExt;
#[cfg(feature = "server")]
use image::{DynamicImage, ImageDecoder, ImageReader};
#[cfg(feature = "server")]
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
#[cfg(feature = "server")]
use sevenz_rust2::decompress_with_extract_fn;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
#[cfg(feature = "server")]
use tokio::sync::Mutex;

// Global lock map for image operations (per image path)
#[cfg(feature = "server")]
static IMAGE_LOCKS: Lazy<Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

/// Helper to get a lock for a given image path (as string)
#[cfg(feature = "server")]
async fn lock_for_image_path<P: AsRef<Path>>(path: P) -> Arc<Mutex<()>> {
    let path_str = path.as_ref().to_string_lossy().to_string();
    let mut map = IMAGE_LOCKS.lock().await;
    map.entry(path_str)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum DownloadProgressEvent {
    DownloadStarted {
        total_bytes: u64,
    },
    DownloadProgress {
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    DownloadComplete {
        total_bytes: u64,
    },
    ExtractionStarted,
    FileExtracted {
        name: String,
        size: u64,
    },
    ExtractionProgress {
        count: usize,
        total_bytes: u64,
    },
    ExtractionComplete {
        total_files: usize,
        total_bytes: u64,
    },
    Error {
        message: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ResizeProgressEvent {
    ResizeStarted {
        total_files: usize,
    },
    FileResized {
        name: String,
    },
    ResizeProgress {
        completed: usize,
        total_files: usize,
    },
    ResizeComplete {
        total_files: usize,
    },
    Error {
        message: String,
    },
}

/// Helper function to safely canonicalize and validate image paths
fn validate_and_canonicalize_image_path(
    images_path: &Path,
    image_name: &str,
) -> Result<std::path::PathBuf> {
    // Validate the image name first
    validate_image_name(image_name)?;

    // Canonicalize the base images directory
    let canonical_base = images_path
        .canonicalize()
        .map_err(|e| anyhow!("Failed to resolve images directory: {}", e))?;

    // Construct the image path
    let image_path = images_path.join(image_name);

    // Canonicalize the image path
    let canonical_image = image_path
        .canonicalize()
        .map_err(|e| anyhow!("Image not found or inaccessible: {}", e))?;

    // Verify the canonical path is within the base directory
    if !canonical_image.starts_with(&canonical_base) {
        return Err(anyhow!("Access denied: path traversal attempt detected").into());
    }

    // Verify it's a file, not a directory
    if !canonical_image.is_file() {
        return Err(anyhow!("Image file not found").into());
    }

    Ok(canonical_image)
}

#[get("/projects/:project_name/images")]
pub async fn get_project_images(project_name: String) -> Result<Vec<String>> {
    validate_project_name(&project_name)?;
    let settings = crate::server::get_settings().await?;
    let images_path = Path::new(&settings.projects_folder)
        .join(&project_name)
        .join("images");

    // Lock on the images directory
    let lock = lock_for_image_path(&images_path).await;
    let _guard = lock.lock().await;

    if !images_path.exists() {
        std::fs::create_dir_all(&images_path)
            .map_err(|e| anyhow!("Failed to create images folder: {}", e))?;
        return Ok(Vec::new());
    }

    let mut images = Vec::new();

    match std::fs::read_dir(&images_path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if let Ok(path) = entry.path().canonicalize() {
                    if path.is_file() {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            if is_image_file(name) {
                                images.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }
        Err(e) => return Err(anyhow!("Failed to read images folder: {}", e).into()),
    }

    images.sort();
    Ok(images)
}

#[cfg(feature = "server")]
#[get("/projects/:project_name/images/:image_name")]
pub async fn get_project_image(
    project_name: String,
    image_name: String,
) -> Result<dioxus::server::axum::response::Response> {
    validate_project_name(&project_name)?;
    let settings = crate::server::get_settings().await?;
    let images_path = Path::new(&settings.projects_folder)
        .join(&project_name)
        .join("images");

    let canonical_image = validate_and_canonicalize_image_path(&images_path, &image_name)?;
    let lock = lock_for_image_path(&canonical_image).await;
    let _guard = lock.lock().await;

    let bytes = std::fs::read(&canonical_image).map_err(|e| {
        anyhow!(
            "Failed to read image file: {} ({})",
            canonical_image.display(),
            e
        )
    })?;

    let content_type = get_content_type_for_image(&image_name);
    Ok((
        [(
            dioxus::server::axum::http::header::CONTENT_TYPE,
            content_type,
        )],
        bytes,
    )
        .into_response())
}

#[post("/projects/:project_name/images/:image_name")]
pub async fn add_project_image(
    project_name: String,
    image_name: String,
    body: Vec<u8>,
) -> Result<()> {
    validate_project_name(&project_name)?;
    validate_image_name(&image_name)?;
    let settings = crate::server::get_settings().await?;
    let images_path = Path::new(&settings.projects_folder)
        .join(&project_name)
        .join("images");

    std::fs::create_dir_all(&images_path)
        .map_err(|e| anyhow!("Failed to create images folder: {}", e))?;

    // Canonicalize the base directory to ensure it exists
    let canonical_base = images_path
        .canonicalize()
        .map_err(|e| anyhow!("Failed to resolve images directory: {}", e))?;

    let image_path = images_path.join(&image_name);

    // Verify the destination is within the project directory
    let canonical_dest = std::path::PathBuf::from(&image_path);
    if !canonical_dest.starts_with(&canonical_base) && canonical_dest.canonicalize().is_ok() {
        return Err(anyhow!("Access denied: path traversal attempt detected").into());
    }

    let lock = lock_for_image_path(&image_path).await;
    let _guard = lock.lock().await;

    std::fs::write(&image_path, body).map_err(|e| anyhow!("Failed to write image file: {}", e))?;

    Ok(())
}

#[delete("/projects/:project_name/images/:image_name")]
pub async fn delete_project_image(project_name: String, image_name: String) -> Result<()> {
    validate_project_name(&project_name)?;
    let settings = crate::server::get_settings().await?;
    let images_path = Path::new(&settings.projects_folder)
        .join(&project_name)
        .join("images");

    let canonical_image = validate_and_canonicalize_image_path(&images_path, &image_name)?;
    let lock = lock_for_image_path(&canonical_image).await;
    let _guard = lock.lock().await;

    std::fs::remove_file(&canonical_image).map_err(|e| anyhow!("Failed to delete image: {}", e))?;

    Ok(())
}

#[post("/projects/:project_name/images")]
pub async fn clear_project_images(project_name: String) -> Result<()> {
    validate_project_name(&project_name)?;

    let settings = crate::server::get_settings().await?;
    let images_path = Path::new(&settings.projects_folder)
        .join(&project_name)
        .join("images");

    if images_path.exists() {
        std::fs::remove_dir_all(&images_path)
            .map_err(|e| anyhow!("Failed to clear images folder: {}", e))?;
    }

    Ok(())
}

#[post("/projects/:project_name/images/resize/:max_dimension")]
pub async fn batch_resize_images(
    project_name: String,
    max_dimension: u32,
) -> Result<ServerEvents<ResizeProgressEvent>> {
    validate_project_name(&project_name)?;

    if max_dimension < 64 || max_dimension > 8192 {
        return Err(anyhow!("Max dimension must be between 64 and 8192 pixels").into());
    }

    let project_name = project_name.clone();

    Ok(ServerEvents::new(move |mut tx| async move {
        if let Err(e) = batch_resize_images_stream(project_name, max_dimension, &mut tx).await {
            let _ = tx.send(ResizeProgressEvent::Error {
                message: format!("{}", e),
            });
        }
    }))
}

#[post("/projects/:project_name/images/demo")]
pub async fn download_demo_images(
    project_name: String,
) -> Result<ServerEvents<DownloadProgressEvent>> {
    validate_project_name(&project_name)?;
    let project_name = project_name.clone();

    Ok(ServerEvents::new(|mut tx| async move {
        if let Err(e) = download_demo_images_stream(project_name, &mut tx).await {
            let _ = tx.send(DownloadProgressEvent::Error {
                message: format!("{}", e),
            });
        }
    }))
}

#[cfg(feature = "server")]
async fn download_demo_images_stream(
    project_name: String,
    tx: &mut dioxus_fullstack::payloads::sse::SseTx<DownloadProgressEvent>,
) -> Result<()> {
    use std::sync::mpsc;

    let settings = crate::server::get_settings().await?;
    let images_path = Path::new(&settings.projects_folder)
        .join(&project_name)
        .join("images")
        .to_path_buf();

    let lock = lock_for_image_path(&images_path).await;
    let _guard = lock.lock().await;

    std::fs::create_dir_all(&images_path)
        .map_err(|e| anyhow!("Failed to create images folder: {}", e))?;

    // Download demo images from ETH3D
    let url = "https://www.eth3d.net/data/door_dslr_jpg.7z";

    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow!("Failed to download demo images from {}: {}", url, e))?;

    // Track download progress with content_length
    let total_size = response.content_length().unwrap_or(0);

    if total_size > 0 {
        let _ = tx
            .send(DownloadProgressEvent::DownloadStarted {
                total_bytes: total_size,
            })
            .await;
    }

    let mut bytes_stream = response.bytes_stream();

    let mut bytes = vec![];
    while let Some(item) = bytes_stream.next().await {
        bytes.extend_from_slice(&item?);
        let _ = tx
            .send(DownloadProgressEvent::DownloadProgress {
                downloaded_bytes: bytes.len() as u64,
                total_bytes: total_size,
            })
            .await;
    }

    let _ = tx
        .send(DownloadProgressEvent::DownloadComplete {
            total_bytes: bytes.len() as u64,
        })
        .await;

    let _ = tx.send(DownloadProgressEvent::ExtractionStarted).await;

    // Create a channel for extraction progress
    let (progress_tx, progress_rx) = mpsc::channel();
    let images_path_clone = images_path.clone();

    // Spawn extraction in a separate thread
    std::thread::spawn(move || {
        extract_jpg_from_7z_memory_with_events(
            std::io::Cursor::new(&bytes),
            &images_path_clone,
            progress_tx,
        );
    });

    // Forward events from the extraction thread to the SSE stream
    for event in progress_rx.iter() {
        let _ = tx.send(event).await;
    }

    Ok(())
}

fn validate_project_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(
            anyhow!("Invalid project name: must not be empty or contain path separators").into(),
        );
    }
    Ok(())
}

fn validate_image_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(
            anyhow!("Invalid image name: must not be empty or contain path separators").into(),
        );
    }
    if !is_image_file(name) {
        return Err(anyhow!("Invalid image file type: supported formats are JPG, JPEG, PNG, BMP, GIF, WebP, and TIFF").into());
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

fn get_content_type_for_image(image_name: &str) -> String {
    let lower = image_name.to_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg".to_string()
    } else if lower.ends_with(".png") {
        "image/png".to_string()
    } else if lower.ends_with(".bmp") {
        "image/bmp".to_string()
    } else if lower.ends_with(".gif") {
        "image/gif".to_string()
    } else if lower.ends_with(".webp") {
        "image/webp".to_string()
    } else if lower.ends_with(".tiff") {
        "image/tiff".to_string()
    } else {
        "application/octet-stream".to_string()
    }
}

#[cfg(feature = "server")]
fn extract_jpg_from_7z_memory_with_events<R: std::io::Read + std::io::Seek>(
    reader: R,
    dest_path: &std::path::Path,
    tx: std::sync::mpsc::Sender<DownloadProgressEvent>,
) {
    use std::sync::{Arc, Mutex};
    let extracted_count = Arc::new(Mutex::new(0usize));
    let total_bytes = Arc::new(Mutex::new(0u64));

    // Canonicalize the destination path
    let canonical_dest = match dest_path.canonicalize() {
        Ok(path) => path,
        Err(e) => {
            let _ = tx.send(DownloadProgressEvent::Error {
                message: format!("Failed to resolve extraction directory: {}", e),
            });
            return;
        }
    };

    let result = decompress_with_extract_fn(reader, dest_path, |file_entry, reader, _| {
        // Only extract files that are not directories and have image file extensions
        if !file_entry.is_directory && is_image_file(&file_entry.name) {
            // Extract just the filename (not directory paths)
            if let Some(file_name) = std::path::Path::new(&file_entry.name)
                .file_name()
                .and_then(|n| n.to_str())
            {
                let dest_file = canonical_dest.join(file_name);

                // Verify the destination file is within the extraction directory
                if !dest_file.starts_with(&canonical_dest) {
                    eprintln!(
                        "[Demo Images] Security check failed: path traversal attempt detected for '{}'",
                        file_name
                    );
                    return Ok(true);
                }

                // Create parent directories if needed
                if let Some(parent) = dest_file.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        eprintln!(
                            "[Demo Images] Warning: Failed to create parent directories: {}",
                            e
                        );
                    }
                }

                match std::fs::File::create(&dest_file) {
                    Ok(mut output) => {
                        match std::io::copy(reader, &mut output) {
                            Ok(bytes_written) => {
                                let count = *extracted_count.lock().unwrap() + 1;
                                let bytes = *total_bytes.lock().unwrap() + bytes_written;
                                *extracted_count.lock().unwrap() = count;
                                *total_bytes.lock().unwrap() = bytes;

                                let file_name_str = file_name.to_string();

                                eprintln!(
                                    "[Demo Images] Extracted '{}': {} bytes",
                                    file_name, bytes_written
                                );

                                // Send file extracted event
                                let _ = tx.send(DownloadProgressEvent::FileExtracted {
                                    name: file_name_str,
                                    size: bytes_written,
                                });

                                // Send progress event
                                let _ = tx.send(DownloadProgressEvent::ExtractionProgress {
                                    count,
                                    total_bytes: bytes,
                                });

                                Ok(true) // Continue processing
                            }
                            Err(e) => {
                                eprintln!(
                                    "[Demo Images] Error copying file '{}' to destination: {}",
                                    file_name, e
                                );
                                Ok(true)
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "[Demo Images] Error creating file '{}': {}",
                            dest_file.display(),
                            e
                        );
                        Ok(true)
                    }
                }
            } else {
                Ok(true)
            }
        } else {
            Ok(true)
        }
    });

    match result {
        Ok(_) => {
            let final_count = *extracted_count.lock().unwrap();
            let final_bytes = *total_bytes.lock().unwrap();
            eprintln!(
                "[Demo Images] Extraction complete: {} files, {} total bytes",
                final_count, final_bytes
            );

            if final_count > 0 {
                let _ = tx.send(DownloadProgressEvent::ExtractionComplete {
                    total_files: final_count,
                    total_bytes: final_bytes,
                });
            } else {
                let _ = tx.send(DownloadProgressEvent::Error {
                    message: "No image files found in the 7z archive".to_string(),
                });
            }
        }
        Err(e) => {
            let final_count = *extracted_count.lock().unwrap();
            eprintln!(
                "[Demo Images] Extraction error: {}. {} files were extracted before the error.",
                e, final_count
            );
            let _ = tx.send(DownloadProgressEvent::Error {
                message: format!("Extraction failed: {}", e),
            });
        }
    }
}

#[cfg(feature = "server")]
async fn batch_resize_images_stream(
    project_name: String,
    max_dimension: u32,
    tx: &mut dioxus_fullstack::payloads::SseTx<ResizeProgressEvent>,
) -> Result<()> {
    let settings = crate::server::get_settings().await?;
    let images_path = Path::new(&settings.projects_folder)
        .join(&project_name)
        .join("images");

    if !images_path.exists() {
        return Err(anyhow!("Images directory not found").into());
    }

    // Get list of image files
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
        Err(e) => return Err(anyhow!("Failed to read images folder: {}", e).into()),
    }

    let total_files = image_files.len();
    let _ = tx
        .send(ResizeProgressEvent::ResizeStarted { total_files })
        .await;

    let mut completed = 0;
    for (image_name, image_path) in image_files {
        // Get per-image lock to prevent concurrent modifications
        let lock = lock_for_image_path(&image_path).await;
        let _guard = lock.lock().await;
        match resize_image_file(&image_path, max_dimension).await {
            Ok(_) => {
                completed += 1;
                let _ = tx
                    .send(ResizeProgressEvent::FileResized { name: image_name })
                    .await;

                let _ = tx
                    .send(ResizeProgressEvent::ResizeProgress {
                        completed,
                        total_files,
                    })
                    .await;
            }
            Err(e) => {
                eprintln!("[Batch Resize] Error resizing {}: {}", image_name, e);
            }
        }
    }

    let _ = tx
        .send(ResizeProgressEvent::ResizeComplete {
            total_files: completed,
        })
        .await;

    Ok(())
}

#[cfg(feature = "server")]
async fn resize_image_file(image_path: &Path, max_dimension: u32) -> Result<bool> {
    // Load and decode image
    let img_decoder = ImageReader::open(&image_path)
        .map_err(|e| anyhow!("Failed to load image: {}", e))?
        .with_guessed_format()
        .map_err(|e| anyhow!("Failed to guess image format: {}", e))?
        .into_decoder()
        .map_err(|e| anyhow!("Failed to decode image: {}", e))?;

    let (width, height) = img_decoder.dimensions();

    // Check if resize is needed
    if width <= max_dimension && height <= max_dimension {
        return Ok(false); // No resize needed
    }

    // Calculate new dimensions maintaining aspect ratio
    let (new_width, new_height) = if width > height {
        let new_w = max_dimension;
        let new_h = ((height as f64 / width as f64) * max_dimension as f64).max(1.0) as u32;
        (new_w, new_h)
    } else {
        let new_h = max_dimension;
        let new_w = ((width as f64 / height as f64) * max_dimension as f64).max(1.0) as u32;
        (new_w, new_h)
    };

    // Actually decode the image now that we know we need to resize
    let img = DynamicImage::from_decoder(img_decoder)
        .map_err(|e| anyhow!("Failed to decode image for resizing: {}", e))?;

    // Resize the image with high-quality filtering
    let resized = img.resize_exact(new_width, new_height, image::imageops::FilterType::Lanczos3);

    // Encode as JPEG for efficient storage (using this method means preferring performance over quality anyway), directly to disk
    let mut output_file = std::fs::File::create(&image_path)
        .map_err(|e| anyhow!("Failed to create output file for resized image: {}", e))?;

    resized
        .write_to(&mut output_file, image::ImageFormat::Jpeg)
        .map_err(|e| anyhow!("Failed to write resized image: {}", e))?;

    Ok(true)
}
