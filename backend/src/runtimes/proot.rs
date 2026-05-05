use super::{
    ImageHash, ImageTag, PrepareProgressTx, PreparedImage, ProcessHandle, Runtime, RuntimeResult,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::process::Command;

// ---------------------------------------------------------------------------
// PRoot binary detection
// ---------------------------------------------------------------------------

/// Locate the proot binary with multiple fallback strategies.
///
/// This function tries to find proot in the following order:
/// 1. Custom location in runtime directory (highest priority - checked first after downloads)
/// 2. Direct execution attempt - for system proot in PATH
/// 3. Via `which::which("proot")` - checks system PATH (lowest priority)
///
/// Returns the path to the proot binary if found, or an error with diagnostic info.
async fn find_proot_binary(runtime_dir: &Path) -> RuntimeResult<String> {
    // Strategy 1: Check custom location in runtime directory FIRST (highest priority)
    // This ensures newly downloaded proot is detected immediately
    let custom = runtime_dir.join("proot");
    if custom.exists() {
        return Ok(custom.to_string_lossy().into_owned());
    }

    // Strategy 2: Try direct execution (proot might be in system PATH)
    match Command::new("proot").arg("--version").output().await {
        Ok(_) => {
            return Ok("proot".to_string());
        }
        Err(_) => {}
    }

    // Strategy 3: Fall back to which (may be cached, least reliable)
    if which::which("proot").is_ok() {
        return Ok("proot".to_string());
    }

    // All strategies failed
    let path_info = std::env::var("PATH").unwrap_or_else(|_| "(not set)".to_string());
    Err(anyhow::anyhow!(
        "PRoot binary not found. Checked: custom runtime directory ({}), direct execution, and system PATH. \
         Current PATH: {}. \
         Ensure PRoot is installed and accessible. \
         Visit https://github.com/proot-me/proot for installation instructions.",
        custom.display(),
        path_info
    ))
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// Metadata stored alongside each prepared image rootfs.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImageMetadata {
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
}

impl PRoot {
    /// Create a PRoot runtime with separate runtime and images directories.
    pub fn new(runtime_dir: PathBuf) -> Self {
        let images_dir = runtime_dir.join("images");
        PRoot {
            runtime_dir,
            images_dir,
        }
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
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse versions: {}", e))?;

        let mut versions: Vec<String> = tags.into_iter().map(|t| t.name).collect();
        versions.sort();
        versions.reverse();
        Ok(versions)
    }

    async fn fetch_android_latest_version(&self) -> RuntimeResult<Vec<String>> {
        let client = reqwest::Client::new();
        let url = "https://packages.termux.dev/apt/termux-main/pool/main/p/proot/";

        let html = client
            .get(url)
            .header("User-Agent", "colmap-openmvs-app")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch versions: {}", e))?
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read response: {}", e))?;

        let mut versions = Vec::new();
        for line in html.lines() {
            if let Some(start) = line.find("proot_") {
                if let Some(end) = line[start..].find(".deb") {
                    let filename = &line[start..start + end];
                    if let Some(version_part) = filename.strip_prefix("proot_") {
                        if let Some(version) = version_part.split('_').next() {
                            versions.push(version.to_string());
                        }
                    }
                }
            }
        }

        versions.sort();
        versions.reverse();
        versions.dedup();

        versions
            .into_iter()
            .next()
            .map(|v| vec![v])
            .ok_or_else(|| anyhow::anyhow!("No PRoot versions found in Termux repository"))
    }

    // -----------------------------------------------------------------------
    // Download helpers
    // -----------------------------------------------------------------------

    async fn download_linux(&self, version: &str) -> RuntimeResult<()> {
        let latest_versions = self.fetch_linux_versions().await?;
        let latest = latest_versions
            .first()
            .ok_or_else(|| anyhow::anyhow!("No versions available"))?;

        if version != latest {
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
        tokio::fs::create_dir_all(&self.runtime_dir)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create runtime directory: {}", e))?;

        let client = reqwest::Client::new();
        let base_url = "https://packages.termux.dev/apt/termux-main/pool/main/p/proot/";

        let proot_url = format!("{}proot_{}_aarch64.deb", base_url, version);
        self.download_and_extract_deb(&client, &proot_url, "proot")
            .await?;

        let talloc_url = format!("{}libtalloc_{}_aarch64.deb", base_url, version);
        self.download_and_extract_deb(&client, &talloc_url, "libtalloc")
            .await?;

        Ok(())
    }

    async fn download_and_extract_deb(
        &self,
        client: &reqwest::Client,
        url: &str,
        package_name: &str,
    ) -> RuntimeResult<()> {
        // Fetch bytes asynchronously
        let bytes = client
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to download {}: {}", package_name, e))?
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", package_name, e))?
            .to_vec();

        // All subsequent work is CPU/disk-bound — offload to a blocking thread.
        let install_dir = self.runtime_dir.clone();
        let pkg = package_name.to_string();

        tokio::task::spawn_blocking(move || {
            let temp_deb = install_dir.join(format!("{}.deb", pkg));
            let temp_data = install_dir.join("data.tar.xz");

            std::fs::write(&temp_deb, &bytes)
                .map_err(|e| anyhow::anyhow!("Failed to write {}.deb: {}", pkg, e))?;

            extract_from_ar_sync(&temp_deb, &temp_data)?;
            extract_tar_xz_sync(&install_dir, &temp_data, &pkg)?;

            std::fs::remove_file(&temp_deb).ok();
            std::fs::remove_file(&temp_data).ok();

            Ok::<_, anyhow::Error>(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Blocking task panicked: {}", e))??;

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
}

// ---------------------------------------------------------------------------
// Runtime trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Runtime for PRoot {
    fn is_supported(&self) -> RuntimeResult<()> {
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
        let proot_bin = find_proot_binary(&self.runtime_dir).await?;
        let output = Command::new(&proot_bin)
            .arg("--version")
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to execute proot: {}", e))?;

        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Self::parse_proot_version(&combined)
    }

    async fn available_versions(&self) -> RuntimeResult<Vec<String>> {
        // Try to use the installed proot version if available
        match find_proot_binary(&self.runtime_dir).await {
            Ok(proot_bin) => {
                let output = Command::new(&proot_bin)
                    .arg("--version")
                    .output()
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to execute proot: {}", e))?;

                let combined = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                let v = Self::parse_proot_version(&combined)?;
                return Ok(vec![v]);
            }
            Err(_) => {
                // PRoot not found, try to fetch available versions from repositories
            }
        }

        match (std::env::consts::ARCH, std::env::consts::OS) {
            ("x86_64", "linux") => self.fetch_linux_versions().await,
            ("aarch64", "android") | ("x86_64", "android") => {
                self.fetch_android_latest_version().await
            }
            _ => Err(anyhow::anyhow!("Unsupported platform for version fetching")),
        }
    }

    async fn download(&self, version: &str) -> RuntimeResult<()> {
        // Check if proot is already available
        if find_proot_binary(&self.runtime_dir).await.is_ok() {
            return Ok(()); // Proot is already available
        }

        match (std::env::consts::ARCH, std::env::consts::OS) {
            ("x86_64", "linux") => self.download_linux(version).await,
            ("aarch64", "android") | ("x86_64", "android") => self.download_android(version).await,
            _ => Err(anyhow::anyhow!("Unsupported platform for download")),
        }
    }

    async fn prepare(&self, image: &str, tx: PrepareProgressTx) -> RuntimeResult<()> {
        use super::image_manager::ImageManager;

        // Use tag as directory name (normalized)
        let tag_dir_name = image.replace(':', "_").replace('/', "_");
        let image_dir = self.images_dir.join(&tag_dir_name);
        let rootfs_dir = image_dir.join("rootfs");

        tokio::fs::create_dir_all(&image_dir)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create image directory: {}", e))?;

        // Pull and extract image using OCI-compliant client
        let manager = ImageManager::new();
        let image_config = manager.pull_and_extract(image, &rootfs_dir, &tx).await?;

        // Persist complete image metadata
        let metadata = ImageMetadata {
            tag: image.to_string(),
            build_date: Some(chrono::Utc::now().to_rfc3339()),
            env: image_config.env,
            entrypoint: image_config.entrypoint,
            cmd: image_config.cmd,
            working_dir: image_config.working_dir.or(Some("/".to_string())),
        };
        let metadata_json = serde_json::to_string_pretty(&metadata)
            .map_err(|e| anyhow::anyhow!("Failed to serialize metadata: {}", e))?;
        tokio::fs::write(image_dir.join("metadata.json"), metadata_json)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write metadata: {}", e))?;

        Ok(())
    }

    async fn remove(&self, image_tag: &str) -> RuntimeResult<()> {
        // Use tag as directory name (normalized)
        let tag_dir_name = image_tag.replace(':', "_").replace('/', "_");
        let image_dir = self.images_dir.join(&tag_dir_name);

        if image_dir.exists() {
            tokio::fs::remove_dir_all(&image_dir)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to remove image: {}", e))?;
        }
        Ok(())
    }

    async fn run(&self, image: &str, args: &[String]) -> RuntimeResult<ProcessHandle> {
        let tag_dir_name = image.replace(':', "_").replace('/', "_");
        let image_dir = self.images_dir.join(&tag_dir_name);
        let rootfs_dir = image_dir.join("rootfs");

        if !rootfs_dir.exists() {
            return Err(anyhow::anyhow!(
                "Image not prepared: {}. Call prepare() first.",
                image
            ));
        }

        // Load metadata
        let metadata_path = image_dir.join("metadata.json");
        let metadata: ImageMetadata = if metadata_path.exists() {
            let content = tokio::fs::read_to_string(&metadata_path)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to read metadata: {}", e))?;
            serde_json::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Failed to deserialize metadata: {}", e))?
        } else {
            ImageMetadata {
                tag: String::new(),
                build_date: None,
                env: Vec::new(),
                entrypoint: None,
                cmd: None,
                working_dir: Some("/".to_string()),
            }
        };

        // Locate the proot binary using centralized detection
        let proot_bin = find_proot_binary(&self.runtime_dir).await?;

        // Build the tokio async Command
        let mut cmd = Command::new(&proot_bin);
        cmd.env_clear();

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

        cmd.arg("-R").arg(&rootfs_dir);

        if let Some(workdir) = &metadata.working_dir {
            cmd.arg("-w").arg(workdir);
        }

        // Compose entrypoint + cmd + user args
        let mut full_cmd: Vec<String> = Vec::new();
        if let Some(ep) = &metadata.entrypoint {
            full_cmd.extend(ep.clone());
        }
        if let Some(c) = &metadata.cmd {
            full_cmd.extend(c.clone());
        }
        full_cmd.extend(args.iter().cloned());

        for arg in &full_cmd {
            cmd.arg(arg);
        }

        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn process: {}", e))?;

        Ok(ProcessHandle { child })
    }

    async fn list_images(&self) -> RuntimeResult<Vec<PreparedImage>> {
        let images_dir = self.images_dir.clone();

        if !tokio::fs::try_exists(&images_dir).await.unwrap_or(false) {
            return Ok(Vec::new());
        }

        let mut images = Vec::new();
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
                // Try to read metadata to get the original tag and build date
                let (tag_str, build_date) = match tokio::fs::read_to_string(&metadata_path).await {
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
                // Use tag as hash for identification
                images.push(PreparedImage::with_build_date(
                    tag,
                    ImageHash::new(tag_str),
                    size,
                    build_date,
                ));
            }
        }

        Ok(images)
    }
}

// ---------------------------------------------------------------------------
// Blocking helpers (run inside tokio::task::spawn_blocking)
// ---------------------------------------------------------------------------

/// Extract `data.tar.xz` from a Debian `.ar` archive.
fn extract_from_ar_sync(ar_path: &Path, output_path: &Path) -> RuntimeResult<()> {
    let file_data =
        std::fs::read(ar_path).map_err(|e| anyhow::anyhow!("Failed to read ar file: {}", e))?;

    if file_data.len() < 8 || &file_data[0..8] != b"!<arch>\n" {
        return Err(anyhow::anyhow!("Invalid ar archive format"));
    }

    let mut offset = 8usize;
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

        if name.contains("data.tar") {
            let data_start = offset + 60;
            std::fs::write(output_path, &file_data[data_start..data_start + size])
                .map_err(|e| anyhow::anyhow!("Failed to write extracted data: {}", e))?;
            return Ok(());
        }

        offset += 60 + ((size + 1) & !1);
    }

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
    let tar_xz_data =
        std::fs::read(tar_xz_path).map_err(|e| anyhow::anyhow!("Failed to read tar.xz: {}", e))?;

    let tar_data = decompress_xz_sync(&tar_xz_data)?;
    let mut archive = tar::Archive::new(&tar_data[..]);

    for entry in archive
        .entries()
        .map_err(|e| anyhow::anyhow!("Failed to read tar: {}", e))?
    {
        let mut entry = entry.map_err(|e| anyhow::anyhow!("Failed to read tar entry: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| anyhow::anyhow!("Failed to get entry path: {}", e))?;
        let path_str = path.to_string_lossy().into_owned();

        if package_name == "proot" && path_str.ends_with("usr/bin/proot") {
            let dest = images_dir.join("proot");
            entry
                .unpack(&dest)
                .map_err(|e| anyhow::anyhow!("Failed to unpack proot: {}", e))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o755);
                std::fs::set_permissions(&dest, perms).ok();
            }
        } else if package_name == "libtalloc" && path_str.contains("usr/lib/libtalloc") {
            let dest = images_dir.join(path.file_name().unwrap_or_default());
            entry
                .unpack(&dest)
                .map_err(|e| anyhow::anyhow!("Failed to unpack library: {}", e))?;
        }
    }

    Ok(())
}
