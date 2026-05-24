//! Android-specific startup tasks to prepare the runtime environment.
//!
//! On Android, the APK's native libraries (jniLibs) are extracted to disk by the system.
//! This module sets up symbolic links and directory structure for the PRoot runtime,
//! using the embedded manifest to guide the setup.
//!
//! Key strategy:
//! - Symlinks point directly to actual jniLibs paths (e.g., /data/app/.../lib/arm64/librootfs-XXX.so)
//! - This allows size calculation to correctly follow symlinks and get actual file sizes
//! - The embedded image is always available and cannot be removed on Android

use std::collections::HashMap;

#[cfg(target_os = "android")]
use std::path::PathBuf;
#[cfg(target_os = "android")]
use tracing::{debug, info, warn};

/// Set up the Android runtime environment for PRoot and the embedded rootfs.
///
/// This function:
/// 1. Reads the embedded rootfs manifest from jniLibs
/// 2. Creates the target rootfs directory structure in the configured images_dir
/// 3. Sets up symbolic links from rootfs paths to actual librootfs-*.so files in jniLibs
///    (Symlinks point to real paths like /data/app/.../lib/arm64/librootfs-XXX.so)
/// 4. Recreates symlinks for directory/file aliases from the manifest
///
/// This is idempotent and safe to call multiple times.
#[cfg(target_os = "android")]
pub async fn setup_android_runtime() -> anyhow::Result<()> {
    info!("Android startup: initializing PRoot runtime environment");

    // Get configuration from settings
    let settings = crate::settings::get_settings()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load settings: {}", e))?;
    let binary_dir = PathBuf::from(&settings.proot_binary_dir);
    let images_dir = PathBuf::from(&settings.proot_images_dir);

    debug!(
        binary_dir = %binary_dir.display(),
        images_dir = %images_dir.display(),
        "Android startup: using directories"
    );

    // Get the actual jniLibs directory for symlink targets
    let jnilib_dir = crate::settings::get_android_native_lib_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to determine jniLibs directory"))?;

    debug!(
        jnilib_dir = %jnilib_dir,
        "Android startup: determined jniLibs directory for symlink targets"
    );

    // Try to read the embedded manifest
    let manifest_path = binary_dir.join("librootfs-manifest.so");
    if !manifest_path.exists() {
        debug!(
            path = %manifest_path.display(),
            "Android startup: embedded manifest not found, skipping setup"
        );
        return Ok(());
    }

    info!(
        path = %manifest_path.display(),
        "Android startup: reading embedded manifest"
    );

    let manifest_content = tokio::fs::read_to_string(&manifest_path).await?;
    let manifest: EmbeddedManifest = serde_json::from_str(&manifest_content)
        .map_err(|e| anyhow::anyhow!("Failed to parse embedded manifest: {}", e))?;

    info!(
        tag = %manifest.tag,
        file_count = manifest.files.len(),
        symlink_count = manifest.symlinks.len(),
        "Android startup: loaded embedded manifest"
    );

    // Create images directory if it doesn't exist
    tokio::fs::create_dir_all(&images_dir)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create images directory: {}", e))?;

    debug!(images_dir = %images_dir.display(), "Android startup: created images directory");

    // Create the image-specific directory for this tag
    let tag_dir_name = manifest.tag.replace([':', '/'], "_");
    let image_dir = images_dir.join(&tag_dir_name);
    let rootfs_dir = image_dir.join("rootfs");

    // Skip if already set up
    if rootfs_dir.exists() {
        info!(
            tag = %manifest.tag,
            rootfs = %rootfs_dir.display(),
            "Android startup: rootfs skeleton already exists, skipping setup"
        );
        return Ok(());
    }

    info!(
        rootfs = %rootfs_dir.display(),
        "Android startup: creating rootfs skeleton"
    );

    tokio::fs::create_dir_all(&rootfs_dir)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create rootfs directory: {}", e))?;

    debug!("Android startup: created rootfs directory");

    // Set up file symlinks pointing to actual jniLibs paths
    let mut symlink_count = 0;
    for (hash, file_info) in &manifest.files {
        let dest_path = rootfs_dir.join(file_info.path.trim_start_matches('/'));

        // Create parent directories as needed
        if let Some(parent) = dest_path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }

        // Remove any existing file/symlink at this location
        let _ = tokio::fs::remove_file(&dest_path).await;

        // Create symlink to the actual librootfs-*.so file in jniLibs
        // This allows size calculation to follow symlinks and get real file sizes
        let symlink_target = format!("{}/librootfs-{}.so", jnilib_dir, hash);
        match tokio::fs::symlink(&symlink_target, &dest_path).await {
            Ok(()) => {
                symlink_count += 1;
                // If executable, chmod the symlink to 0755
                if file_info.executable {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = tokio::fs::set_permissions(&dest_path, std::fs::Permissions::from_mode(0o755)).await;
                }
                if symlink_count % 100 == 0 {
                    debug!(
                        count = symlink_count,
                        path = %file_info.path,
                        "Android startup: created file symlinks"
                    );
                }
            }
            Err(e) => {
                warn!(
                    path = %file_info.path,
                    target = %symlink_target,
                    error = %e,
                    "Android startup: failed to create file symlink"
                );
            }
        }
    }

    info!(symlink_count, "Android startup: created file symlinks");

    // Recreate original rootfs symlinks (directory/file aliases)
    let mut alias_count = 0;
    for (link_path, link_target) in &manifest.symlinks {
        let dest_path = rootfs_dir.join(link_path.trim_start_matches('/'));

        // Create parent directories as needed
        if let Some(parent) = dest_path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }

        // Remove any existing file/symlink/dir at this location
        let _ = tokio::fs::remove_file(&dest_path).await;
        let _ = tokio::fs::remove_dir(&dest_path).await;

        // Create symlink to the target
        match tokio::fs::symlink(link_target, &dest_path).await {
            Ok(()) => {
                alias_count += 1;
                if alias_count % 50 == 0 {
                    debug!(
                        count = alias_count,
                        path = %link_path,
                        target = %link_target,
                        "Android startup: created alias symlinks"
                    );
                }
            }
            Err(e) => {
                warn!(
                    path = %link_path,
                    target = %link_target,
                    error = %e,
                    "Android startup: failed to create alias symlink"
                );
            }
        }
    }

    info!(alias_count, "Android startup: created alias symlinks");

    // Note: libtalloc.so dependency is now handled via patchelf RPATH=$ORIGIN,
    // no need for symlink hacks anymore

    // Write metadata file
    let metadata = ImageMetadata {
        tag: manifest.tag.clone(),
        build_date: manifest.build_date,
        env: manifest.env,
        entrypoint: manifest.entrypoint,
        cmd: manifest.cmd,
        working_dir: manifest.working_dir,
    };

    let metadata_json = serde_json::to_string_pretty(&metadata)
        .map_err(|e| anyhow::anyhow!("Failed to serialize metadata: {}", e))?;

    tokio::fs::write(image_dir.join("metadata.json"), metadata_json)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write metadata: {}", e))?;

    debug!(metadata_path = %image_dir.join("metadata.json").display(), "Android startup: wrote metadata file");

    info!(
        tag = %manifest.tag,
        rootfs = %rootfs_dir.display(),
        total_files = manifest.files.len(),
        total_aliases = manifest.symlinks.len(),
        "Android startup: completed PRoot runtime setup"
    );

    Ok(())
}

/// No-op on non-Android platforms.
#[cfg(not(target_os = "android"))]
pub async fn setup_android_runtime() -> anyhow::Result<()> {
    Ok(())
}

#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
struct EmbeddedManifest {
    #[serde(default)]
    tag: String,
    #[serde(default)]
    build_date: Option<String>,
    #[serde(default)]
    env: Vec<String>,
    #[serde(default)]
    entrypoint: Option<Vec<String>>,
    #[serde(default)]
    cmd: Option<Vec<String>>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    files: HashMap<String, FileInfo>,
    #[serde(default)]
    symlinks: HashMap<String, String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize)]
struct FileInfo {
    path: String,
    #[serde(default)]
    executable: bool,
    #[serde(default)]
    size: u64,
}

#[allow(dead_code)]
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ImageMetadata {
    tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    build_date: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    env: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entrypoint: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cmd: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    working_dir: Option<String>,
}
