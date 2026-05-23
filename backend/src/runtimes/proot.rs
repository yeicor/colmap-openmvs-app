use super::{
    ImageHash, ImageTag, Mount, PrepareProgressTx, PreparedImage, ProcessHandle, Runtime,
    RuntimeResult,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// PRoot binary detection
// ---------------------------------------------------------------------------

/// Get the PRoot command to use from `custom_command`.
/// If `custom_command` is empty, returns `"proot"` (auto-detect).
#[allow(dead_code)]
fn get_proot_command(custom_command: &str) -> String {
    if custom_command.is_empty() {
        "proot".to_string()
    } else {
        custom_command
            .split_whitespace()
            .next()
            .unwrap_or("proot")
            .to_string()
    }
}

/// Get environment variables and the binary path from a custom proot command string.
///
/// Tokens of the form `KEY=VALUE` where `KEY` consists entirely of ASCII
/// alphanumeric characters and underscores are treated as environment-variable
/// assignments.  The first token that does not match that pattern is taken as
/// the binary path.
///
/// # Examples
/// ```
/// // "LD_LIBRARY_PATH=/a:/b /a/libproot.so"
/// //   -> env = [("LD_LIBRARY_PATH", "/a:/b")], bin = "/a/libproot.so"
/// ```
fn get_proot_env_and_args(custom_command: &str) -> (Vec<(String, String)>, String) {
    if custom_command.is_empty() {
        return (vec![], "proot".to_string());
    }

    let mut env_vars: Vec<(String, String)> = vec![];
    let mut proot_cmd = "proot".to_string();

    for part in custom_command.split_whitespace() {
        // If it looks like KEY=VALUE (key is all alnum+underscore), treat as env var
        if let Some((key, value)) = part.split_once('=') {
            if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                env_vars.push((key.to_string(), value.to_string()));
                continue;
            }
        }
        // Otherwise it's the binary path
        proot_cmd = part.to_string();
    }

    (env_vars, proot_cmd)
}

/// Locate the proot binary using multiple fallback strategies.
///
/// Strategy order:
/// 0. Binary specified in `custom_command` (if the path exists on disk)
/// 1. `libproot.so` in `runtime_dir` (Android embedded asset)
/// 2. Plain `proot` in `runtime_dir` (downloaded binary)
/// 3. Direct execution attempt (`proot --version`)
/// 4. `which::which("proot")` (system PATH search)
async fn find_proot_binary(runtime_dir: &Path, custom_command: &str) -> RuntimeResult<String> {
    info!(runtime_dir = %runtime_dir.display(), "[debug]find_proot_binary: starting search");

    // Strategy 0: Use the binary path from custom_command if it exists on disk
    if !custom_command.is_empty() {
        let (_, bin) = get_proot_env_and_args(custom_command);
        if std::path::Path::new(&bin).exists() {
            info!(bin = %bin, "find_proot_binary: using binary from custom_command");
            return Ok(bin);
        }
        info!(bin = %bin, "find_proot_binary: custom_command binary does not exist on disk, continuing search");
    }

    // Strategy 1: Look for embedded 'proot' in runtime_dir (Android APK asset, original name)
    let embedded_proot = runtime_dir.join("proot");
    if embedded_proot.exists() {
        info!(path = %embedded_proot.display(), "find_proot_binary: found embedded proot");
        return Ok(embedded_proot.to_string_lossy().into_owned());
    }

    // Strategy 2 is now merged with Strategy 1 (both look for 'proot' in runtime_dir).

    // Strategy 3: Try direct execution (proot might be in system PATH)
    info!("[DEBUG]find_proot_binary: attempting direct execution of 'proot --version'");
    if Command::new("proot")
        .arg("--version")
        .output()
        .await
        .is_ok()
    {
        info!("[DEBUG]find_proot_binary: direct execution succeeded, 'proot' is in system PATH");
        return Ok("proot".to_string());
    }

    // Strategy 4: Fall back to which (may be cached, least reliable)
    info!("[DEBUG]find_proot_binary: attempting lookup via which::which()");
    if which::which("proot").is_ok() {
        info!("[DEBUG]find_proot_binary: which::which() found proot");
        return Ok("proot".to_string());
    }

    // All strategies failed
    let path_info = std::env::var("PATH").unwrap_or_else(|_| "(not set)".to_string());
    error!(
        runtime_dir = %runtime_dir.display(),
        current_path = %path_info,
        "find_proot_binary: all strategies failed to locate proot binary"
    );
    Err(anyhow::anyhow!(
        "PRoot binary not found. Checked: custom_command={:?}, runtime_dir={}, system PATH. \
         Current PATH: {}. \
         Ensure PRoot is installed and accessible.",
        custom_command,
        runtime_dir.display(),
        path_info
    ))
}

/// Set up a proot [`Command`] with the correct environment variables.
///
/// When `custom_command` is non-empty its `KEY=VALUE` tokens are applied
/// directly (they already encode `LD_LIBRARY_PATH` etc. on Android).
/// Otherwise `LD_LIBRARY_PATH` is set to `runtime_dir` so that a downloaded
/// `libtalloc.so` can be found at runtime.
fn setup_proot_command(proot_bin: &str, runtime_dir: &Path, custom_command: &str) -> Command {
    let mut cmd = Command::new(proot_bin);
    info!(proot_bin = %proot_bin, runtime_dir = ?runtime_dir, "setup_proot_command: initializing");

    if !custom_command.is_empty() {
        // Apply env vars from custom_command (which already has LD_LIBRARY_PATH etc.)
        let (env_vars, _) = get_proot_env_and_args(custom_command);
        for (key, value) in env_vars {
            info!(key = %key, value = %value, "setup_proot_command: applying env var from custom_command");
            cmd.env(&key, &value);
        }
    } else {
        // Default: set LD_LIBRARY_PATH to runtime_dir for libtalloc discovery
        if let Some(lib_path_str) = runtime_dir.to_str() {
            info!(lib_path = %lib_path_str, "setup_proot_command: setting LD_LIBRARY_PATH");
            cmd.env("LD_LIBRARY_PATH", lib_path_str);
        }
    }

    cmd
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// Metadata stored alongside each prepared image rootfs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct ImageMetadata {
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
}

/// Entry in the embedded rootfs manifest for a single regular file.
#[derive(Debug, Deserialize)]
struct RootfsFileInfo {
    path: String,
    #[serde(default)]
    executable: bool,
    #[serde(default)]
    size: Option<u64>,
}

/// Manifest produced at build time describing the complete rootfs hierarchy.
///
/// Files are stored as flat hash-named entries in jniLibs.  The manifest
/// records every directory, every regular file (with its hash key), and
/// every symlink so the app can reconstruct the directory skeleton at
/// first launch without copying any file data.
#[derive(Debug, Deserialize)]
struct RootfsManifest {
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
    /// Absolute directory paths that must exist in the skeleton.
    #[serde(default)]
    dirs: Vec<String>,
    /// Map from hash key (jniLibs filename) → file info.
    #[serde(default)]
    files: std::collections::HashMap<String, RootfsFileInfo>,
    /// Map from absolute guest symlink path → symlink target.
    #[serde(default)]
    symlinks: std::collections::HashMap<String, String>,
}

impl RootfsManifest {
    fn into_image_metadata(self) -> ImageMetadata {
        ImageMetadata {
            tag: self.tag,
            build_date: self.build_date,
            env: self.env,
            entrypoint: self.entrypoint,
            cmd: self.cmd,
            working_dir: if self.working_dir.as_deref().unwrap_or("/").is_empty() {
                Some("/".to_string())
            } else {
                self.working_dir
            },
        }
    }

    fn total_size(&self) -> u64 {
        self.files.values().filter_map(|f| f.size).sum()
    }
}

// ---------------------------------------------------------------------------
// PRoot struct
// ---------------------------------------------------------------------------

/// PRoot-based container runtime.
///
/// Uses the system `proot` binary when available, or downloads one into
/// the runtime directory.  Images are stored in a separate images subdirectory.
/// Obtain an instance through [`super::RuntimeFactory`].
#[derive(Debug, Clone)]
pub struct PRoot {
    pub runtime_dir: PathBuf,
    pub images_dir: PathBuf,
    /// Custom PRoot command (e.g., `"proot"` or
    /// `"LD_LIBRARY_PATH=/path /path/libproot.so"`).  When empty, the binary
    /// is auto-detected.
    pub custom_command: String,
}

impl PRoot {
    /// Create a PRoot runtime with separate directories and custom command.
    pub fn new(runtime_dir: PathBuf, images_dir: PathBuf, custom_command: String) -> Self {
        PRoot {
            runtime_dir,
            images_dir,
            custom_command,
        }
    }
    pub fn new_default_images(runtime_dir: PathBuf, custom_command: String) -> Self {
        Self::new(
            runtime_dir.clone(),
            runtime_dir.join("images"),
            custom_command,
        )
    }
    pub fn new_simple(runtime_dir: PathBuf) -> Self {
        Self::new(
            runtime_dir.clone(),
            runtime_dir.join("images"),
            String::new(),
        )
    }

    // -----------------------------------------------------------------------
    // Version helpers
    // -----------------------------------------------------------------------

    /// Parse a version string from raw proot `--version` output.
    pub fn parse_proot_version(output: &str) -> RuntimeResult<String> {
        if let Some(pos) = output.find(['v', 'V']) {
            if let Some(next_char) = output[pos + 1..].chars().next() {
                if next_char.is_ascii_digit() {
                    let version_part = &output[pos + 1..];
                    let version = version_part
                        .split(|c: char| c.is_whitespace() || c == '\n' || c == '\r')
                        .next()
                        .unwrap_or("")
                        .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '.')
                        .to_string();
                    if !version.is_empty()
                        && version.chars().next().is_some_and(|c| c.is_ascii_digit())
                    {
                        return Ok(version);
                    }
                }
            }
        }

        for word in output.split_whitespace() {
            if let Some(first_char) = word.chars().next() {
                if first_char.is_ascii_digit() {
                    let version = word
                        .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '.')
                        .to_string();
                    if version.contains('.') || version.contains('-') {
                        return Ok(version);
                    }
                }
            }
        }

        Err(anyhow::anyhow!("Could not parse PRoot version from output"))
    }

    // -----------------------------------------------------------------------
    // Version fetching
    // -----------------------------------------------------------------------

    async fn fetch_linux_versions(&self) -> RuntimeResult<Vec<String>> {
        let client = reqwest::Client::new();
        let url = "https://gitlab.com/api/v4/projects/proot%2Fproot/repository/tags";

        let response = client
            .get(url)
            .header("User-Agent", "colmap-openmvs-app")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch versions: {}", e))?;

        #[derive(Deserialize)]
        struct GitLabTag {
            name: String,
        }

        let tags: Vec<GitLabTag> = response
            .json::<Vec<GitLabTag>>()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse versions: {}", e))?;

        let mut versions: Vec<String> = tags.into_iter().map(|t| t.name).collect();
        versions.sort();
        versions.reverse();
        Ok(versions)
    }

    async fn fetch_android_latest_version(&self) -> RuntimeResult<Vec<String>> {
        info!("[DEBUG]fetch_android_latest_version: starting");
        let client = reqwest::Client::new();
        let url = "https://packages.termux.dev/apt/termux-main/pool/main/p/proot/";

        info!(url = %url, "[debug]fetch_android_latest_version: fetching from Termux repository");
        let html = client
            .get(url)
            .header("User-Agent", "colmap-openmvs-app")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch versions: {}", e))?
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read response: {}", e))?;

        debug!(
            html_length = html.len(),
            "fetch_android_latest_version: received HTML"
        );
        let mut versions = Vec::new();
        for line in html.lines() {
            if let Some(start) = line.find("proot_") {
                if let Some(end) = line[start..].find("_aarch64.deb") {
                    let filename = &line[start..start + end];
                    if let Some(version_part) = filename.strip_prefix("proot_") {
                        info!(version = %version_part, "[trace]fetch_android_latest_version: found version");
                        versions.push(version_part.to_string());
                    }
                }
            }
        }

        versions.sort();
        versions.reverse();
        versions.dedup();

        info!(versions = ?versions, "[debug]fetch_android_latest_version: parsed versions");
        if versions.is_empty() {
            info!("fetch_android_latest_version: no versions found in Termux repository");
        }
        versions
            .into_iter()
            .next()
            .map(|v| vec![v])
            .ok_or_else(|| anyhow::anyhow!("No PRoot versions found in Termux repository"))
    }

    async fn fetch_libtalloc_version(&self) -> RuntimeResult<Vec<String>> {
        info!("[DEBUG]fetch_libtalloc_version: starting");
        let client = reqwest::Client::new();
        let url = "https://packages.termux.dev/apt/termux-main/pool/main/libt/libtalloc/";

        info!(url = %url, "[debug]fetch_libtalloc_version: fetching from Termux repository");
        let html = client
            .get(url)
            .header("User-Agent", "colmap-openmvs-app")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch libtalloc versions: {}", e))?
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read libtalloc response: {}", e))?;

        debug!(
            html_length = html.len(),
            "fetch_libtalloc_version: received HTML"
        );
        let mut versions = Vec::new();
        for line in html.lines() {
            if let Some(start) = line.find("libtalloc_") {
                if let Some(end) = line[start..].find("_aarch64.deb") {
                    let filename = &line[start..start + end];
                    if let Some(version_part) = filename.strip_prefix("libtalloc_") {
                        info!("[trace]fetch_libtalloc_version: found version");
                        versions.push(version_part.to_string());
                    }
                }
            }
        }

        versions.sort();
        versions.reverse();
        versions.dedup();

        info!(versions = ?versions, "[debug]fetch_libtalloc_version: parsed versions");
        if versions.is_empty() {
            info!("fetch_libtalloc_version: no versions found in Termux repository");
        }
        versions
            .into_iter()
            .next()
            .map(|v| vec![v])
            .ok_or_else(|| anyhow::anyhow!("No libtalloc versions found in Termux repository"))
    }

    // -----------------------------------------------------------------------
    // Download helpers
    // -----------------------------------------------------------------------

    async fn download_linux(&self, version: &str) -> RuntimeResult<()> {
        let latest_versions = self.fetch_linux_versions().await?;
        let latest = latest_versions
            .first()
            .ok_or_else(|| anyhow::anyhow!("No versions available"))?
            .clone();

        // Extract the base version number from the requested version
        // (handles both "5.3.1", "v5.3.1", and "5.3.1-99a84175" formats)
        let version_cleaned = version.trim_start_matches('v');
        let base_version = version_cleaned.split('-').next().unwrap_or(version_cleaned);

        // Extract base from latest and remove 'v' prefix
        let latest_cleaned = latest.trim_start_matches('v');
        let latest_base = latest_cleaned.split('-').next().unwrap_or(latest_cleaned);

        eprintln!(
            "[DEBUG] download_linux: Requested: {}, base: {}, latest: {}, latest_base: {}",
            version, base_version, latest, latest_base
        );

        // Allow download if the base version matches (ignoring 'v' prefix and suffixes)
        if base_version != latest_base {
            return Err(anyhow::anyhow!(
                "Only the latest version ({}) is available for download, requested: {}",
                latest,
                version
            ));
        }

        tokio::fs::create_dir_all(&self.runtime_dir)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create runtime directory: {}", e))?;

        let url = "https://proot.gitlab.io/proot/bin/proot";
        let bytes = reqwest::Client::new()
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to download proot: {}", e))?
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read download: {}", e))?;

        let proot_path = self.runtime_dir.join("proot");
        tokio::fs::write(&proot_path, &bytes)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write proot binary: {}", e))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o755);
            tokio::fs::set_permissions(&proot_path, perms)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to set executable bit: {}", e))?;
        }

        Ok(())
    }

    async fn download_android(&self, version: &str) -> RuntimeResult<()> {
        info!(version = %version, "download_android: starting Android PRoot download");

        info!(runtime_dir = %self.runtime_dir.display(), "[debug]download_android: creating runtime directory");
        tokio::fs::create_dir_all(&self.runtime_dir)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create runtime directory: {}", e))?;

        let client = reqwest::Client::new();
        let proot_base_url = "https://packages.termux.dev/apt/termux-main/pool/main/p/proot/";

        let proot_url = format!("{}proot_{}_aarch64.deb", proot_base_url, version);
        info!(version = %version, url = %proot_url, "download_android: downloading PRoot");
        self.download_and_extract_deb(&client, &proot_url, "proot")
            .await?;
        info!("[DEBUG]download_android: PRoot download completed");

        // Fetch the latest libtalloc version from the correct directory
        info!("download_android: fetching libtalloc version");
        let libtalloc_versions = self.fetch_libtalloc_version().await?;
        let libtalloc_version = libtalloc_versions
            .first()
            .ok_or_else(|| anyhow::anyhow!("No libtalloc version found"))?
            .clone();
        info!(libtalloc_version = %libtalloc_version, "[debug]download_android: libtalloc version fetched");

        let libtalloc_base_url =
            "https://packages.termux.dev/apt/termux-main/pool/main/libt/libtalloc/";
        let talloc_url = format!(
            "{}libtalloc_{}_aarch64.deb",
            libtalloc_base_url, libtalloc_version
        );
        info!(libtalloc_version = %libtalloc_version, url = %talloc_url, "download_android: downloading libtalloc");
        self.download_and_extract_deb(&client, &talloc_url, "libtalloc")
            .await?;
        info!("[DEBUG]download_android: libtalloc download completed");

        info!(version = %version, "download_android: Android installation completed");
        Ok(())
    }

    async fn download_and_extract_deb(
        &self,
        client: &reqwest::Client,
        url: &str,
        package_name: &str,
    ) -> RuntimeResult<()> {
        info!(package_name = %package_name, url = %url, "download_and_extract_deb: starting download");

        // Fetch bytes asynchronously
        let response = client
            .get(url)
            .header("User-Agent", "colmap-openmvs-app")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to download {}: {}", package_name, e))?
            .error_for_status()
            .map_err(|e| {
                anyhow::anyhow!("Failed to download {} (HTTP error): {}", package_name, e)
            })?;

        info!(package_name = %package_name, "[debug]download_and_extract_deb: response received");
        let bytes = response
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", package_name, e))?
            .to_vec();

        info!(package_name = %package_name, size = bytes.len(), "[debug]download_and_extract_deb: bytes received");

        // Validate that we received a valid ar archive (deb file)
        info!(package_name = %package_name, "[trace]download_and_extract_deb: validating ar archive format");
        if bytes.len() < 8 || &bytes[0..8] != b"!<arch>\n" {
            error!(package_name = %package_name, size = bytes.len(), url = %url, "download_and_extract_deb: invalid deb archive format");
            return Err(anyhow::anyhow!(
                "Invalid deb package received for {}: not an ar archive. URL: {}",
                package_name,
                url
            ));
        }
        info!(package_name = %package_name, "[debug]download_and_extract_deb: archive format is valid");

        // All subsequent work is CPU/disk-bound — offload to a blocking thread.
        let install_dir = self.runtime_dir.clone();
        let pkg = package_name.to_string();

        info!(package_name = %package_name, "[trace]download_and_extract_deb: offloading to blocking task");
        tokio::task::spawn_blocking(move || {
            let temp_deb = install_dir.join(format!("{}.deb", pkg));
            let temp_data = install_dir.join("data.tar.xz");

            std::fs::write(&temp_deb, &bytes)
                .map_err(|e| anyhow::anyhow!("Failed to write {}.deb: {}", pkg, e))?;

            extract_from_ar_sync(&temp_deb, &temp_data)?;
            extract_tar_xz_sync(&install_dir, &temp_data, &pkg)?;

            std::fs::remove_file(&temp_deb).ok();
            std::fs::remove_file(&temp_data).ok();

            info!(package_name = %pkg, "[debug]download_and_extract_deb: extraction completed");
            Ok::<_, anyhow::Error>(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Blocking task panicked: {}", e))??;

        info!(package_name = %package_name, "download_and_extract_deb: completed successfully");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Misc
    // -----------------------------------------------------------------------

    async fn calculate_dir_size_async(&self, path: &Path) -> RuntimeResult<u64> {
        let mut size = 0u64;
        if let Ok(mut entries) = tokio::fs::read_dir(path).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Ok(meta) = entry.metadata().await {
                    if meta.is_dir() {
                        size += Box::pin(self.calculate_dir_size_async(&entry.path())).await?;
                    } else {
                        size += meta.len();
                    }
                }
            }
        }
        Ok(size)
    }

    // -----------------------------------------------------------------------
    // Embedded asset helpers (Android jniLibs)
    // -----------------------------------------------------------------------

    /// Path to the embedded rootfs manifest (`embedded_rootfs_manifest.json`).
    /// Present when the APK was built with the embedded rootfs approach.
    pub fn embedded_rootfs_manifest_path(&self) -> Option<PathBuf> {
        let p = self.runtime_dir.join("embedded_rootfs_manifest.json");
        if p.exists() {
            Some(p)
        } else {
            None
        }
    }

    /// Read and parse the embedded [`RootfsManifest`].
    pub(crate) async fn read_embedded_manifest(&self) -> RuntimeResult<RootfsManifest> {
        let path = self.embedded_rootfs_manifest_path().ok_or_else(|| {
            anyhow::anyhow!(
                "No embedded rootfs manifest found (embedded_rootfs_manifest.json missing from {})",
                self.runtime_dir.display()
            )
        })?;
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read manifest: {}", e))?;
        serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse manifest JSON: {}", e))
    }

    /// Build the lightweight rootfs skeleton from the embedded manifest.
    ///
    /// The skeleton consists of:
    /// - Directories recreated under `images_dir/<tag>/rootfs/`
    /// - Symlinks for every file pointing to `/mnt/jni/<hash>` (the hash-named
    ///   file extracted from jniLibs by Android at install time)
    /// - Original rootfs symlinks (directory aliases, file aliases)
    ///
    /// No file *data* is copied — the actual content lives in `runtime_dir`
    /// (the native-lib directory) already extracted by the Android installer.
    /// proot is invoked with `-b <runtime_dir>:/mnt/jni` so the symlink targets
    /// resolve correctly inside the container.
    async fn setup_rootfs_skeleton(
        &self,
        image_tag: &str,
        tx: &PrepareProgressTx,
    ) -> RuntimeResult<()> {
        let manifest = self.read_embedded_manifest().await?;

        let tag_dir_name = image_tag.replace([':', '/'], "_");
        let image_dir = self.images_dir.join(&tag_dir_name);
        let rootfs_dir = image_dir.join("rootfs");

        tokio::fs::create_dir_all(&rootfs_dir)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create rootfs dir: {}", e))?;

        let _ = tx
            .send(colmap_openmvs_api::PrepareProgress::ExtractingLayer {
                layer: "Building rootfs skeleton".to_string(),
                progress: 0.0,
            })
            .await;

        let total = (manifest.dirs.len() + manifest.files.len() + manifest.symlinks.len()).max(1);
        let mut done = 0usize;

        // ── Create /mnt/jni mount point ──────────────────────────────────
        tokio::fs::create_dir_all(rootfs_dir.join("mnt/jni"))
            .await
            .ok();

        // ── Recreate directory tree ──────────────────────────────────────
        for dir in &manifest.dirs {
            let dest = rootfs_dir.join(dir.trim_start_matches('/'));
            tokio::fs::create_dir_all(&dest).await.ok();
            done += 1;
            if done % 200 == 0 {
                let _ = tx
                    .send(colmap_openmvs_api::PrepareProgress::ExtractingLayer {
                        layer: "Building rootfs skeleton".to_string(),
                        progress: done as f32 / total as f32 * 0.4,
                    })
                    .await;
            }
        }

        // ── Create per-file symlinks: <rootfs>/<path> → /mnt/jni/<hash> ──
        for (hash, file_info) in &manifest.files {
            let dest = rootfs_dir.join(file_info.path.trim_start_matches('/'));
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await.ok();
            }
            let _ = tokio::fs::remove_file(&dest).await;
            let symlink_target = format!("/mnt/jni/{}", hash);
            #[cfg(unix)]
            if let Err(e) = tokio::fs::symlink(&symlink_target, &dest).await {
                warn!(
                    path = %dest.display(),
                    target = %symlink_target,
                    error = %e,
                    "setup_rootfs_skeleton: failed to create file symlink"
                );
            }
            done += 1;
            if done % 500 == 0 {
                let _ = tx
                    .send(colmap_openmvs_api::PrepareProgress::ExtractingLayer {
                        layer: "Building rootfs skeleton".to_string(),
                        progress: 0.4 + done as f32 / total as f32 * 0.45,
                    })
                    .await;
            }
        }

        // ── Recreate original rootfs symlinks ────────────────────────────
        for (link_path, target) in &manifest.symlinks {
            let dest = rootfs_dir.join(link_path.trim_start_matches('/'));
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await.ok();
            }
            let _ = tokio::fs::remove_file(&dest).await;
            let _ = tokio::fs::remove_dir(&dest).await;
            #[cfg(unix)]
            if let Err(e) = tokio::fs::symlink(target, &dest).await {
                warn!(
                    path = %dest.display(),
                    target = %target,
                    error = %e,
                    "setup_rootfs_skeleton: failed to create original symlink"
                );
            }
            done += 1;
        }

        // ── Write metadata ───────────────────────────────────────────────
        let metadata = ImageMetadata {
            tag: image_tag.to_string(),
            ..manifest.into_image_metadata()
        };
        let metadata_json = serde_json::to_string_pretty(&metadata)
            .map_err(|e| anyhow::anyhow!("Failed to serialize metadata: {}", e))?;
        tokio::fs::write(image_dir.join("metadata.json"), metadata_json)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write metadata: {}", e))?;

        let _ = tx
            .send(colmap_openmvs_api::PrepareProgress::ExtractingLayer {
                layer: "Building rootfs skeleton".to_string(),
                progress: 1.0,
            })
            .await;

        info!(image_tag, "setup_rootfs_skeleton: complete");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Embedded-image tag matching
    // -----------------------------------------------------------------------

    /// Return `true` when the embedded manifest tag matches the requested image.
    fn embedded_tag_matches(meta_tag: &str, image: &str) -> bool {
        if meta_tag == image {
            return true;
        }
        if meta_tag.ends_with(&format!("/{}", image)) {
            return true;
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Runtime trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Runtime for PRoot {
    fn is_supported(&self) -> RuntimeResult<()> {
        // On Android, if the embedded proot asset is present that is always
        // sufficient — no need for a system-wide installation.
        #[cfg(target_os = "android")]
        if self.runtime_dir.join("proot").exists() && self.embedded_rootfs_manifest_path().is_some()
        {
            return Ok(());
        }

        let target_os = std::env::consts::OS;
        let target_arch = std::env::consts::ARCH;

        if which::which("proot").is_ok() {
            return Ok(());
        }

        match (target_arch, target_os) {
            ("x86_64", "linux") => Ok(()),
            ("aarch64", "android") | ("x86_64", "android") => Ok(()),
            (arch, os) => Err(anyhow::anyhow!(
                "PRoot cannot be automatically installed on this platform \
                 (arch: {arch}, os: {os}). Supported: x86_64-linux, *-android. \
                 Install proot manually and add it to $PATH to use it on other platforms."
            )),
        }
    }

    async fn version(&self) -> RuntimeResult<String> {
        info!("version: starting");
        let proot_bin = find_proot_binary(&self.runtime_dir, &self.custom_command).await?;
        info!(proot_bin = %proot_bin, "[debug]version: found proot binary");

        let mut cmd = setup_proot_command(&proot_bin, &self.runtime_dir, &self.custom_command);
        info!(cmd = ?cmd, "[trace]version: executing proot --version");
        let output = cmd.arg("--version").output().await.map_err(|e| {
            error!(cmd = ?cmd, error = %e, "version: failed to execute proot --version");
            anyhow::anyhow!("Failed to execute proot to get version: {}", e)
        })?;
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        info!(output = %combined, "[debug]version: proot output");
        let version = Self::parse_proot_version(&combined)?;
        info!(version = %version, "version: parsed version");
        Ok(version)
    }

    async fn available_versions(&self) -> RuntimeResult<Vec<String>> {
        info!("available_versions: starting");
        // Try to use the installed proot version if available
        match find_proot_binary(&self.runtime_dir, &self.custom_command).await {
            Ok(proot_bin) => {
                info!(proot_bin = %proot_bin, "[debug]available_versions: found proot binary");
                let mut cmd =
                    setup_proot_command(&proot_bin, &self.runtime_dir, &self.custom_command);
                info!("[trace]available_versions: executing proot --version");
                let output = cmd.arg("--version").output().await;

                let output = match output {
                    Ok(o) => Some(o),
                    Err(_) if cfg!(target_os = "android") => {
                        info!("[DEBUG]available_versions: trying shell workaround on Android");
                        Command::new("/system/bin/sh")
                            .arg("-c")
                            .arg(format!("{} --version", proot_bin))
                            .output()
                            .await
                            .ok()
                    }
                    Err(_) => None,
                };

                if let Some(output) = output {
                    let combined = format!(
                        "{}{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    );
                    info!(output = %combined, "[debug]available_versions: proot output");
                    if let Ok(v) = Self::parse_proot_version(&combined) {
                        info!(version = %v, "available_versions: returning installed version");
                        return Ok(vec![v]);
                    }
                }
            }
            Err(_) => {
                info!("[DEBUG]available_versions: proot not found, fetching from repository");
            }
        }

        info!(arch = %std::env::consts::ARCH, os = %std::env::consts::OS, "[debug]available_versions: fetching from repository");
        match (std::env::consts::ARCH, std::env::consts::OS) {
            ("x86_64", "linux") => self.fetch_linux_versions().await,
            ("aarch64", "android") | ("x86_64", "android") => {
                self.fetch_android_latest_version().await
            }
            _ => Err(anyhow::anyhow!("Unsupported platform for version fetching")),
        }
    }

    async fn download(&self, version: &str) -> RuntimeResult<()> {
        info!(version = %version, "download: starting");

        // On Android, if the embedded proot asset is present, skip download.
        #[cfg(target_os = "android")]
        if self.runtime_dir.join("proot").exists() && self.embedded_rootfs_manifest_path().is_some()
        {
            info!(
                runtime_dir = %self.runtime_dir.display(),
                "download: embedded proot + manifest present — skipping download"
            );
            return Ok(());
        }

        // Check if proot is already available via any other strategy
        if find_proot_binary(&self.runtime_dir, &self.custom_command)
            .await
            .is_ok()
        {
            info!("[DEBUG]download: proot is already available");
            return Ok(());
        }

        info!(arch = %std::env::consts::ARCH, os = %std::env::consts::OS, "[debug]download: platform info");
        match (std::env::consts::ARCH, std::env::consts::OS) {
            ("x86_64", "linux") => self.download_linux(version).await,
            ("aarch64", "android") | ("x86_64", "android") => self.download_android(version).await,
            _ => Err(anyhow::anyhow!("Unsupported platform for download")),
        }
    }

    async fn prepare(&self, image: &str, tx: PrepareProgressTx) -> RuntimeResult<()> {
        use super::image_manager::ImageManager;

        // ----------------------------------------------------------------
        // Fast-path: manifest-based embedded rootfs (skeleton approach).
        // The actual file data lives in the native-lib dir (runtime_dir),
        // already extracted by the Android installer.  We just create the
        // lightweight directory skeleton + symlinks once.
        // ----------------------------------------------------------------
        if let Ok(manifest) = self.read_embedded_manifest().await {
            if Self::embedded_tag_matches(&manifest.tag, image) {
                let tag_dir_name = image.replace([':', '/'], "_");
                let rootfs_dir = self.images_dir.join(&tag_dir_name).join("rootfs");

                if rootfs_dir.exists() {
                    info!(
                        image,
                        "prepare: rootfs skeleton already exists, nothing to do"
                    );
                    return Ok(());
                }

                info!(image, meta_tag = %manifest.tag, "prepare: building rootfs skeleton from manifest");
                return self.setup_rootfs_skeleton(image, &tx).await;
            } else {
                info!(
                    image,
                    meta_tag = %manifest.tag,
                    "prepare: manifest tag does not match, falling through to network download"
                );
            }
        }

        // ----------------------------------------------------------------
        // Slow-path: pull from OCI registry
        // ----------------------------------------------------------------
        let tag_dir_name = image.replace([':', '/'], "_");
        let image_dir = self.images_dir.join(&tag_dir_name);
        let rootfs_dir = image_dir.join("rootfs");

        tokio::fs::create_dir_all(&image_dir)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create image directory: {}", e))?;

        // Pull and extract image using OCI-compliant client
        let manager = ImageManager::new();
        let image_config = manager.pull_and_extract(image, &rootfs_dir, &tx).await?;

        info!(
            env_count = image_config.env.len(),
            has_entrypoint = image_config.entrypoint.is_some(),
            has_cmd = image_config.cmd.is_some(),
            working_dir = ?image_config.working_dir,
            "prepare: pulled image metadata from OCI registry"
        );

        // Persist complete image metadata
        let metadata = ImageMetadata {
            tag: image.to_string(),
            build_date: image_config.created,
            env: image_config.env,
            entrypoint: image_config.entrypoint,
            cmd: image_config.cmd,
            working_dir: image_config.working_dir.or(Some("/".to_string())),
        };
        info!(
            metadata_tag = %metadata.tag,
            env_count = metadata.env.len(),
            has_entrypoint = metadata.entrypoint.is_some(),
            has_cmd = metadata.cmd.is_some(),
            "prepare: persisting image metadata to disk"
        );
        let metadata_json = serde_json::to_string_pretty(&metadata)
            .map_err(|e| anyhow::anyhow!("Failed to serialize metadata: {}", e))?;
        tokio::fs::write(image_dir.join("metadata.json"), metadata_json)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write metadata: {}", e))?;

        Ok(())
    }

    async fn remove(&self, image_tag: &str) -> RuntimeResult<()> {
        let tag_dir_name = image_tag.replace([':', '/'], "_");
        let image_dir = self.images_dir.join(&tag_dir_name);

        if image_dir.exists() {
            tokio::fs::remove_dir_all(&image_dir)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to remove image: {}", e))?;
        }
        Ok(())
    }

    async fn run(
        &self,
        image: &str,
        args: &[String],
        mounts: &[Mount],
    ) -> RuntimeResult<ProcessHandle> {
        info!(image = %image, args_len = args.len(), mounts_len = mounts.len(), "run: starting container");

        let tag_dir_name = image.replace([':', '/'], "_");
        let image_dir = self.images_dir.join(&tag_dir_name);
        let rootfs_dir = image_dir.join("rootfs");

        info!(runtime_dir = %self.runtime_dir.display(), "[debug]run: runtime directory");
        info!(rootfs_dir = %rootfs_dir.display(), "[debug]run: checking if rootfs exists");
        if !rootfs_dir.exists() {
            return Err(anyhow::anyhow!(
                "Image not prepared: {}. Call prepare() first.",
                image
            ));
        }
        info!("[DEBUG]run: rootfs exists");

        // Load metadata
        let metadata_path = image_dir.join("metadata.json");
        info!(metadata_path = %metadata_path.display(), "[debug]run: loading metadata");
        let metadata: ImageMetadata = if metadata_path.exists() {
            let content = tokio::fs::read_to_string(&metadata_path)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to read metadata: {}", e))?;
            serde_json::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Failed to deserialize metadata: {}", e))?
        } else {
            info!("[DEBUG]run: metadata file not found, using defaults");
            ImageMetadata {
                tag: String::new(),
                build_date: None,
                env: Vec::new(),
                entrypoint: None,
                cmd: None,
                working_dir: Some("/".to_string()),
            }
        };
        info!(metadata = ?metadata, "[trace]run: metadata loaded");

        // Locate the proot binary
        let proot_bin = find_proot_binary(&self.runtime_dir, &self.custom_command).await?;
        info!(proot_bin = %proot_bin, "[debug]run: found proot binary");

        // Build the tokio async Command with proper environment setup
        let mut cmd = setup_proot_command(&proot_bin, &self.runtime_dir, &self.custom_command);

        for env_var in &metadata.env {
            if let Some((key, value)) = env_var.split_once('=') {
                cmd.env(key, value);
            }
        }
        cmd.env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        );
        cmd.env("HOME", "/root");

        // Bind the native-lib dir (runtime_dir) to /mnt/jni inside the
        // container so symlinks in the skeleton can resolve to real files.
        if self.embedded_rootfs_manifest_path().is_some() {
            cmd.arg("-b")
                .arg(format!("{}:/mnt/jni", self.runtime_dir.display()));
        }

        info!(
            mounts_len = mounts.len(),
            "[trace]run: adding mount bindings"
        );
        for mount in mounts {
            cmd.arg("-b").arg(format!(
                "{}:{}",
                mount.host_path.display(),
                mount.container_path
            ));
        }
        cmd.arg("-R").arg(&rootfs_dir);

        if let Some(workdir) = &metadata.working_dir {
            cmd.arg("-w").arg(workdir);
        }

        // Compose entrypoint + cmd + user args
        let mut full_cmd: Vec<String> = Vec::new();
        if let Some(ep) = &metadata.entrypoint {
            full_cmd.extend(ep.clone());
        }
        // Follow Docker semantics: if the caller provides explicit args, they
        // *replace* the image's default CMD entirely.  Only fall back to CMD
        // when no args are supplied.
        if args.is_empty() {
            if let Some(c) = &metadata.cmd {
                full_cmd.extend(c.clone());
            }
        } else {
            full_cmd.extend(args.iter().cloned());
        }

        info!(full_cmd = ?full_cmd, "[trace]run: full command");
        // Wrap command with environment variable exports
        if !metadata.env.is_empty() {
            let mut env_exports = String::new();
            for env_var in &metadata.env {
                env_exports.push_str(&format!("export {}; ", env_var));
            }
            let cmd_str = full_cmd.join(" ");
            cmd.arg("/bin/sh")
                .arg("-c")
                .arg(format!("{}exec {}", env_exports, cmd_str));
        } else {
            for arg in &full_cmd {
                cmd.arg(arg);
            }
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        info!("[DEBUG]run: spawning process");
        info!(proot_bin = %proot_bin, "[trace]run: final command details");
        let child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn process: {}", e))?;

        info!("run: process spawned successfully");
        Ok(ProcessHandle { child })
    }

    async fn list_images(&self) -> RuntimeResult<Vec<PreparedImage>> {
        let images_dir = self.images_dir.clone();
        let mut images = Vec::new();

        // ----------------------------------------------------------------
        // Scan extracted images from the images directory
        // ----------------------------------------------------------------
        if tokio::fs::try_exists(&images_dir).await.unwrap_or(false) {
            let mut entries = tokio::fs::read_dir(&images_dir)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to read images directory: {}", e))?;

            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to read directory entry: {}", e))?
            {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let metadata_path = path.join("metadata.json");
                if !tokio::fs::try_exists(&metadata_path).await.unwrap_or(false) {
                    continue;
                }

                if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                    let (tag_str, build_date) = match tokio::fs::read_to_string(&metadata_path)
                        .await
                    {
                        Ok(content) => match serde_json::from_str::<ImageMetadata>(&content) {
                            Ok(metadata) if !metadata.tag.is_empty() => {
                                (metadata.tag, metadata.build_date)
                            }
                            Ok(metadata) => (format!("unknown:{}", dir_name), metadata.build_date),
                            Err(_) => (format!("unknown:{}", dir_name), None),
                        },
                        Err(_) => (format!("unknown:{}", dir_name), None),
                    };

                    let tag = ImageTag::from_string(tag_str.clone());
                    let size = self.calculate_dir_size_async(&path).await?;
                    images.push(PreparedImage::with_build_date(
                        tag,
                        ImageHash::new(tag_str),
                        size,
                        build_date,
                    ));
                }
            }
        }

        // ----------------------------------------------------------------
        // Augment with embedded image (shown even before first skeleton setup)
        // ----------------------------------------------------------------
        if let Ok(manifest) = self.read_embedded_manifest().await {
            let tag_already_present = images.iter().any(|img| {
                img.tag.to_string() == manifest.tag
                    || Self::embedded_tag_matches(&manifest.tag, &img.tag.to_string())
            });

            if !tag_already_present && !manifest.tag.is_empty() {
                let size = manifest.total_size();
                let tag = ImageTag::from_string(manifest.tag.clone());
                images.push(PreparedImage::with_build_date(
                    tag,
                    ImageHash::new(manifest.tag.clone()),
                    size,
                    manifest.build_date,
                ));
                info!(
                    tag = %manifest.tag,
                    "list_images: added embedded (skeleton not yet built) image to list"
                );
            }
        }

        Ok(images)
    }

    async fn delete_binary(&self) -> RuntimeResult<()> {
        // On Android the proot binary lives in the APK's read-only native lib
        // directory and cannot be deleted.
        #[cfg(target_os = "android")]
        {
            return Err(anyhow::anyhow!(
                "Cannot delete the embedded proot binary — \
                 it is part of the application package and managed by Android."
            ));
        }

        #[cfg(not(target_os = "android"))]
        {
            let proot_bin = find_proot_binary(&self.runtime_dir, &self.custom_command).await?;
            let runtime_proot = self.runtime_dir.join("proot");

            if proot_bin == runtime_proot.to_string_lossy().as_ref() {
                tokio::fs::remove_file(&runtime_proot)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to delete proot binary: {}", e))?;
                info!(
                    path = %runtime_proot.display(),
                    "delete_binary: deleted proot binary"
                );
                return Ok(());
            }

            Err(anyhow::anyhow!(
                "Cannot delete system proot binary at {}. \
                 Only binaries downloaded into the runtime directory can be deleted.",
                proot_bin
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Blocking helpers (run inside tokio::task::spawn_blocking)
// ---------------------------------------------------------------------------

/// Extract `data.tar.xz` from a Debian `.ar` archive.
fn extract_from_ar_sync(ar_path: &Path, output_path: &Path) -> RuntimeResult<()> {
    info!(ar_path = %ar_path.display(), output_path = %output_path.display(), "[debug]extract_from_ar_sync: starting extraction");

    info!("[trace]extract_from_ar_sync: reading ar file");
    let file_data =
        std::fs::read(ar_path).map_err(|e| anyhow::anyhow!("Failed to read ar file: {}", e))?;

    if file_data.len() < 8 {
        error!(
            size = file_data.len(),
            "extract_from_ar_sync: ar file too small"
        );
        return Err(anyhow::anyhow!(
            "Invalid ar archive format: file too small ({} bytes)",
            file_data.len()
        ));
    }

    info!("[trace]extract_from_ar_sync: validating ar magic signature");
    if &file_data[0..8] != b"!<arch>\n" {
        let magic = String::from_utf8_lossy(&file_data[0..8]);
        error!(magic = %magic, "extract_from_ar_sync: invalid ar magic signature");
        return Err(anyhow::anyhow!(
            "Invalid ar archive format: bad magic signature. Got: {:?}",
            magic
        ));
    }
    info!("[DEBUG]extract_from_ar_sync: ar magic signature is valid");

    let mut offset = 8usize;
    info!("[trace]extract_from_ar_sync: parsing ar members");
    while offset + 60 <= file_data.len() {
        let name = String::from_utf8_lossy(&file_data[offset..offset + 16])
            .trim_end()
            .trim_end_matches('/')
            .to_string();

        let size_str = String::from_utf8_lossy(&file_data[offset + 48..offset + 58])
            .trim_end()
            .to_string();
        let size: usize = size_str
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid ar member size"))?;

        info!(member_name = %name, member_size = size, "[trace]extract_from_ar_sync: found ar member");
        if name.contains("data.tar") {
            info!(member_name = %name, member_size = size, "[debug]extract_from_ar_sync: data.tar found");
            let data_start = offset + 60;
            std::fs::write(output_path, &file_data[data_start..data_start + size])
                .map_err(|e| anyhow::anyhow!("Failed to write extracted data: {}", e))?;
            info!(output_path = %output_path.display(), bytes_written = size, "[debug]extract_from_ar_sync: extracted successfully");
            return Ok(());
        }

        offset += 60 + ((size + 1) & !1);
    }

    error!("extract_from_ar_sync: data.tar not found in ar archive");
    Err(anyhow::anyhow!("data.tar not found in ar archive"))
}

/// Decompress an XZ-compressed byte slice.
fn decompress_xz_sync(data: &[u8]) -> RuntimeResult<Vec<u8>> {
    use std::io::Read;
    let mut decoder = xz2::read::XzDecoder::new(data);
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|e| anyhow::anyhow!("Failed to decompress xz: {}", e))?;
    Ok(output)
}

/// Extract proot/libtalloc binaries from a `data.tar.xz`.
fn extract_tar_xz_sync(
    images_dir: &Path,
    tar_xz_path: &Path,
    package_name: &str,
) -> RuntimeResult<()> {
    info!(images_dir = %images_dir.display(), tar_xz_path = %tar_xz_path.display(), package_name = %package_name, "[debug]extract_tar_xz_sync: starting extraction");

    let tar_xz_data =
        std::fs::read(tar_xz_path).map_err(|e| anyhow::anyhow!("Failed to read tar.xz: {}", e))?;

    debug!(
        tar_xz_size = tar_xz_data.len(),
        "extract_tar_xz_sync: decompressing xz data"
    );
    let tar_data = decompress_xz_sync(&tar_xz_data)?;
    debug!(
        tar_size = tar_data.len(),
        "extract_tar_xz_sync: tar data decompressed"
    );

    let mut archive = tar::Archive::new(&tar_data[..]);

    info!("[trace]extract_tar_xz_sync: iterating through tar entries");
    for entry in archive
        .entries()
        .map_err(|e| anyhow::anyhow!("Failed to read tar: {}", e))?
    {
        let mut entry = entry.map_err(|e| anyhow::anyhow!("Failed to read tar entry: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| anyhow::anyhow!("Failed to get entry path: {}", e))?;
        let path_str = path.to_string_lossy().into_owned();

        info!(entry_path = %path_str, package_name = %package_name, "[trace]extract_tar_xz_sync: processing tar entry");
        if package_name == "proot" && path_str.ends_with("usr/bin/proot") {
            let dest = images_dir.join("proot");
            info!(dest = %dest.display(), "[debug]extract_tar_xz_sync: extracting proot binary");
            entry
                .unpack(&dest)
                .map_err(|e| anyhow::anyhow!("Failed to unpack proot: {}", e))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o755);
                std::fs::set_permissions(&dest, perms).ok();
                info!(dest = %dest.display(), "[debug]extract_tar_xz_sync: set executable permissions on proot");
            }
        } else if package_name == "libtalloc" && path_str.contains("usr/lib/libtalloc") {
            let dest = images_dir.join(path.file_name().unwrap_or_default());
            info!(dest = %dest.display(), "[debug]extract_tar_xz_sync: extracting libtalloc library");
            entry
                .unpack(&dest)
                .map_err(|e| anyhow::anyhow!("Failed to unpack library: {}", e))?;
        }
    }

    info!(package_name = %package_name, "[debug]extract_tar_xz_sync: extraction completed");
    Ok(())
}
