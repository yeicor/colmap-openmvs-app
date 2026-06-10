//! Android-specific startup tasks to prepare the PRoot runtime environment.
//!
//! On Android, the APK's native libraries (jniLibs) are extracted to disk by the
//! Android installer. Only true ELF binaries are placed in jniLibs (as
//! `librootfs-<hash16>.so` files) — non-ELF files are instead packed into a
//! `rootfs.zip` archive that is embedded directly into the Rust binary at
//! compile time via `include_bytes!` (see `backend/build.rs`).
//!
//! At startup this module:
//! 1. Reads `.rootfs_manifest.json` from within the embedded `rootfs.zip`
//! 2. Extracts all non-ELF files from the zip into `{filesDir}/rootfs/`
//! 3. For every ELF file listed in the manifest, creates a symlink from
//!    `{rootfs}/<original-path>` → `{jniLibs}/librootfs-<hash16>.so`
//! 4. Recreates any rootfs-internal symlinks recorded in the manifest
//!
//! This ensures all files in jniLibs are genuine ELF binaries (satisfying
//! Google Play's LOAD-segment alignment check), while the full rootfs is
//! still available via symlinks + the extracted flat archive.

use crate::runtimes::shared;
use crate::runtimes::ImageMetadata;

use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// Global mutex to serialise concurrent calls to [`setup_android_runtime`].
/// Without this, multiple API invocations can race on the idempotency check
/// (e.g. two threads both see that `.rootfs_ready` is missing and both start
/// extracting / symlinking, causing spurious failures).
static SETUP_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

/// Set up the Android runtime environment for PRoot and the embedded rootfs.
pub async fn setup_android_runtime() -> anyhow::Result<()> {
    // Serialise concurrent callers — only one thread performs the setup while
    // the others wait, then all observe the completed state.
    let _guard = SETUP_MUTEX.get_or_init(|| Mutex::new(())).lock().await;

    info!("Android startup: initializing PRoot runtime environment");

    let settings = crate::settings::get_settings()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load settings: {e}"))?;
    let images_dir = PathBuf::from(&settings.proot_images_dir);

    debug!(images_dir = %images_dir.display(), "Android startup: using images directory");

    let jnilib_dir = crate::settings::get_android_native_lib_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to determine jniLibs directory"))?;

    debug!(jnilib_dir = %jnilib_dir, "Android startup: jniLibs directory");

    let files_dir = crate::settings::get_android_files_dir();
    debug!(files_dir = %files_dir, "Android startup: files directory");

    // ── 1. Read the manifest from the embedded rootfs.zip ──────────────
    let (manifest, rootfs_zip_bytes) = read_embedded_manifest_and_zip()?;

    info!(
        tag = %manifest.tag,
        file_count = manifest.files.len(),
        symlink_count = manifest.symlinks.len(),
        zip_size = rootfs_zip_bytes.len(),
        "Android startup: loaded embedded manifest"
    );

    // ── 2. Determine target directories ────────────────────────────────
    let tag_dir_name = manifest.tag.replace([':', '/'], "_");
    let image_dir = images_dir.join(&tag_dir_name);
    let rootfs_dir = image_dir.join("rootfs");

    // ── 3. Idempotency check ───────────────────────────────────────────
    if rootfs_dir.exists() && rootfs_dir.join(".rootfs_ready").exists() {
        info!(
            tag = %manifest.tag,
            rootfs = %rootfs_dir.display(),
            "Android startup: rootfs already set up, verifying symlinks"
        );
        if verify_symlinks(&rootfs_dir, &manifest).await {
            info!("Android startup: symlinks valid, skipping setup");
            return Ok(());
        }
        warn!("Android startup: symlinks broken (reinstall likely), rebuilding");
        tokio::fs::remove_dir_all(&rootfs_dir).await?;
    }

    tokio::fs::create_dir_all(&rootfs_dir)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create rootfs directory: {e}"))?;

    // ── 4. Extract non-ELF files from the embedded zip ─────────────────
    extract_rootfs_zip_inner(&rootfs_zip_bytes, &rootfs_dir.to_string_lossy())?;
    debug!(extract_dir = %rootfs_dir.display(), "Android startup: extracted non-ELF files");

    // ── 5. Create ELF symlinks pointing to jniLibs ─────────────────────
    let mut elf_count = 0usize;
    for (hash, file_info) in &manifest.files {
        let dest_path = rootfs_dir.join(file_info.path.trim_start_matches('/'));
        if let Some(parent) = dest_path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        let _ = tokio::fs::remove_file(&dest_path).await;

        // Symlink → {jniLibs}/librootfs-<hash16>.so
        let target = format!("{jnilib_dir}/librootfs-{hash}.so");
        if tokio::fs::symlink(&target, &dest_path).await.is_ok() {
            elf_count += 1;
        } else {
            warn!(
                path = file_info.path,
                target = %target,
                "Android startup: failed to create ELF symlink"
            );
        }
    }
    info!(elf_count, "Android startup: created ELF symlinks");

    // ── 6. Recreate rootfs-internal symlinks ───────────────────────────
    let mut alias_count = 0usize;
    for (link_path, link_target) in &manifest.symlinks {
        let dest_path = rootfs_dir.join(link_path.trim_start_matches('/'));
        if let Some(parent) = dest_path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        let _ = tokio::fs::remove_file(&dest_path).await;
        let _ = tokio::fs::remove_dir(&dest_path).await;
        if tokio::fs::symlink(link_target, &dest_path).await.is_ok() {
            alias_count += 1;
        } else {
            warn!(
                path = link_path,
                target = link_target,
                "failed to create alias symlink"
            );
        }
    }
    info!(alias_count, "Android startup: created alias symlinks");

    // ── 7. Write metadata ──────────────────────────────────────────────
    let metadata = ImageMetadata {
        tag: manifest.tag.clone(),
        build_date: manifest.build_date.clone(),
        env: manifest.env.clone(),
        entrypoint: manifest.entrypoint.clone(),
        cmd: manifest.cmd.clone(),
        working_dir: manifest.working_dir.clone(),
    };
    let metadata_json = serde_json::to_string_pretty(&metadata)
        .map_err(|e| anyhow::anyhow!("Failed to serialize metadata: {e}"))?;
    tokio::fs::write(image_dir.join("metadata.json"), metadata_json)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write metadata: {e}"))?;

    // ── 8. Mark as ready ───────────────────────────────────────────────
    tokio::fs::write(rootfs_dir.join(".rootfs_ready"), b"")
        .await
        .ok();

    info!(
        tag = %manifest.tag,
        rootfs = %rootfs_dir.display(),
        elf_count = manifest.files.len(),
        alias_count = manifest.symlinks.len(),
        "Android startup: completed"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Embedded resource access
// ---------------------------------------------------------------------------

/// Read the embedded rootfs.zip bytes and parse the manifest from within it.
///
/// The zip is embedded at compile time by `backend/build.rs` via
/// `include_bytes!(concat!(env!("OUT_DIR"), "/rootfs.zip"))`.
/// On non-Android targets this function always returns an error (the
/// code path that calls it is guarded by `#[cfg(target_os = "android")]`).
fn read_embedded_manifest_and_zip() -> anyhow::Result<(shared::RootfsManifest, Vec<u8>)> {
    #[cfg(target_os = "android")]
    {
        let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/rootfs.zip"));

        let cursor = Cursor::new(bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|e| anyhow::anyhow!("Failed to open embedded rootfs.zip: {e}"))?;

        let mut manifest_entry = archive.by_name(".rootfs_manifest.json").map_err(|_| {
            anyhow::anyhow!(".rootfs_manifest.json not found in embedded rootfs.zip")
        })?;

        let mut content = String::new();
        manifest_entry
            .read_to_string(&mut content)
            .map_err(|e| anyhow::anyhow!("Failed to read manifest from zip: {e}"))?;

        let manifest: shared::RootfsManifest = serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse embedded manifest: {e}"))?;

        Ok((manifest, bytes.to_vec()))
    }

    #[cfg(not(target_os = "android"))]
    {
        Err(anyhow::anyhow!(
            "No embedded rootfs.zip (non-Android target)"
        ))
    }
}

// ---------------------------------------------------------------------------
// Zip extraction
// ---------------------------------------------------------------------------

/// Extract the embedded `rootfs.zip` bytes to `extract_dir`.
///
/// The zip contains only non-ELF files with their original rootfs-relative
/// paths (e.g. `etc/passwd`), plus the `.rootfs_manifest.json` at the root.
/// Already-extracted directories are skipped.
fn extract_rootfs_zip_inner(zip_bytes: &[u8], extract_dir: &str) -> anyhow::Result<()> {
    let index_path = std::path::Path::new(extract_dir).join(".rootfs_extracted");
    if index_path.exists() {
        debug!("Rootfs already extracted, skipping");
        return Ok(());
    }

    let cursor = Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| anyhow::anyhow!("Failed to open rootfs.zip: {e}"))?;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| anyhow::anyhow!("Failed to read zip entry {i}: {e}"))?;

        let filename = file.name().to_string();

        // Skip the manifest — it's processed separately.
        if filename == ".rootfs_manifest.json" {
            continue;
        }

        let out_path = format!("{extract_dir}/{filename}");

        if filename.ends_with('/') {
            std::fs::create_dir_all(&out_path).ok();
        } else {
            if let Some(parent) = std::path::Path::new(&out_path).parent() {
                std::fs::create_dir_all(parent).ok();
            }
            // Write only if not already present (idempotent).
            if !std::path::Path::new(&out_path).exists() {
                let mut out = std::fs::File::create(&out_path)?;
                std::io::copy(&mut file, &mut out)?;
                // Restore Unix permissions stored in the zip entry.
                if let Some(mode) = file.unix_mode() {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(mode))
                            .ok();
                    }
                }
            }
        }
    }

    std::fs::write(&index_path, b"done").ok();
    info!(extract_dir = %extract_dir, "Non-ELF rootfs extracted");
    Ok(())
}

// ---------------------------------------------------------------------------
// Symlink verification
// ---------------------------------------------------------------------------

/// Check a sample of ELF symlinks in the rootfs skeleton.
///
/// On Android the jniLibs path changes after reinstall, so absolute symlinks
/// from a previous install break.  This checks up to 5 entries; if any
/// target is missing the rootfs is rebuilt.
async fn verify_symlinks(rootfs_dir: &std::path::Path, manifest: &shared::RootfsManifest) -> bool {
    let check_count = manifest.files.len().min(5);
    let mut checked = 0usize;
    for file_info in manifest.files.values() {
        let dest = rootfs_dir.join(file_info.path.trim_start_matches('/'));
        match tokio::fs::read_link(&dest).await {
            Ok(target) => {
                if !target.exists() {
                    return false;
                }
            }
            Err(_) => return false,
        }
        checked += 1;
        if checked >= check_count {
            break;
        }
    }
    true
}
