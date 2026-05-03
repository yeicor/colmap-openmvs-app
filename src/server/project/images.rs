#[cfg(feature = "server")]
use dioxus::core::anyhow;
use dioxus::prelude::*;
use sevenz_rust2::decompress_with_extract_fn;
#[cfg(feature = "server")]
use std::path::Path;

#[get("/projects/:project_name/images")]
pub async fn get_project_images(project_name: String) -> Result<Vec<String>> {
    validate_project_name(&project_name)?;

    let settings = crate::server::get_settings().await?;
    let images_path = Path::new(&settings.projects_folder)
        .join(&project_name)
        .join("images");

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

#[get("/projects/:project_name/images/:image_name")]
pub async fn get_project_image(
    project_name: String,
    image_name: String,
) -> Result<(String, Vec<u8>)> {
    validate_project_name(&project_name)?;
    validate_image_name(&image_name)?;

    let settings = crate::server::get_settings().await?;
    let image_path = Path::new(&settings.projects_folder)
        .join(&project_name)
        .join("images")
        .join(&image_name);

    // Ensure the image path is within the project directory (path traversal protection)
    let canonical_base = Path::new(&settings.projects_folder)
        .join(&project_name)
        .join("images")
        .canonicalize()
        .map_err(|e| anyhow!("Failed to resolve images directory: {}", e))?;

    let canonical_image = image_path
        .canonicalize()
        .map_err(|e| anyhow!("Image not found or inaccessible: {}", e))?;

    if !canonical_image.starts_with(&canonical_base) {
        return Err(anyhow!("Access denied: path traversal attempt detected").into());
    }

    if !canonical_image.is_file() {
        return Err(anyhow!("Image file not found").into());
    }

    let content_type = get_content_type_for_image(&image_name);
    let bytes = std::fs::read(&canonical_image).map_err(|e| {
        anyhow!(
            "Failed to read image file: {} ({})",
            canonical_image.display(),
            e
        )
    })?;

    Ok((content_type, bytes))
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

    let image_path = images_path.join(&image_name);

    std::fs::write(&image_path, body).map_err(|e| anyhow!("Failed to write image file: {}", e))?;

    Ok(())
}

#[delete("/projects/:project_name/images/:image_name")]
pub async fn delete_project_image(project_name: String, image_name: String) -> Result<()> {
    validate_project_name(&project_name)?;
    validate_image_name(&image_name)?;

    let settings = crate::server::get_settings().await?;
    let image_path = Path::new(&settings.projects_folder)
        .join(&project_name)
        .join("images")
        .join(&image_name);

    if !image_path.exists() {
        return Err(anyhow!("Image not found").into());
    }

    std::fs::remove_file(&image_path).map_err(|e| anyhow!("Failed to delete image: {}", e))?;

    Ok(())
}

#[delete("/projects/:project_name/images")]
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

#[post("/projects/:project_name/images/demo")]
pub async fn download_demo_images(project_name: String) -> Result<()> {
    validate_project_name(&project_name)?;

    let settings = crate::server::get_settings().await?;
    let images_path = Path::new(&settings.projects_folder)
        .join(&project_name)
        .join("images");

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
        // Log download started with size information
        eprintln!(
            "[Demo Images] Downloading {} bytes from {}",
            total_size, url
        );
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| anyhow!("Failed to read response body from {}: {}", url, e))?;

    eprintln!(
        "[Demo Images] Downloaded {} bytes, starting extraction",
        bytes.len()
    );

    // Extract JPG files directly from memory with progress tracking
    extract_jpg_from_7z_memory(std::io::Cursor::new(bytes.iter().as_slice()), &images_path)
        .map_err(|e| anyhow!("Failed to extract demo images from 7z archive: {}", e))?;

    Ok(())
}

// Helper functions
#[cfg(feature = "server")]
fn validate_project_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(
            anyhow!("Invalid project name: must not be empty or contain path separators").into(),
        );
    }
    Ok(())
}

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
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

#[cfg(feature = "server")]
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
fn extract_jpg_from_7z_memory<R: std::io::Read + std::io::Seek>(
    reader: R,
    dest_path: &std::path::Path,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut extracted_count = 0;
    let mut total_bytes = 0u64;

    let result = decompress_with_extract_fn(reader, dest_path, |file_entry, reader, _| {
        // Only extract files that are not directories and have image file extensions
        if !file_entry.is_directory && is_image_file(&file_entry.name) {
            // Extract just the filename (not directory paths)
            if let Some(file_name) = std::path::Path::new(&file_entry.name)
                .file_name()
                .and_then(|n| n.to_str())
            {
                let dest_file = dest_path.join(file_name);

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
                                extracted_count += 1;
                                total_bytes += bytes_written;
                                eprintln!(
                                    "[Demo Images] Extracted '{}': {} bytes",
                                    file_name, bytes_written
                                );
                                Ok(true) // Continue processing
                            }
                            Err(e) => {
                                eprintln!(
                                    "[Demo Images] Error copying file '{}' to destination: {}",
                                    file_name, e
                                );
                                // Don't fail the entire extraction, just skip this file
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
                        // Don't fail the entire extraction, just skip this file
                        Ok(true)
                    }
                }
            } else {
                Ok(true)
            }
        } else {
            Ok(true) // Skip non-image files and directories
        }
    });

    match result {
        Ok(_) => {
            eprintln!(
                "[Demo Images] Extraction complete: {} files, {} total bytes",
                extracted_count, total_bytes
            );
            if extracted_count == 0 {
                return Err("No image files found in the 7z archive".into());
            }
            Ok(())
        }
        Err(e) => Err(format!(
            "7z decompression failed: {}. {} files were extracted before the error.",
            e, extracted_count
        )
        .into()),
    }
}
