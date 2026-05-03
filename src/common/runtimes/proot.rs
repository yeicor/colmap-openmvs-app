use super::{ProcessHandle, RuntimeError, RuntimeResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Progress events during image preparation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PrepareProgress {
    ResolvingImage,
    Downloading {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    ExtractingLayer {
        layer: String,
        progress: f32,
    },
    WritingRootFs,
    Configuring,
    Completed,
}

/// Docker image metadata stored after preparation
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImageMetadata {
    env: HashMap<String, String>,
    entrypoint: Option<Vec<String>>,
    cmd: Option<Vec<String>>,
    working_dir: Option<String>,
}

/// PRoot runtime configuration
#[derive(Debug, Clone)]
pub struct PRoot {
    pub install_dir: PathBuf,
}

impl PRoot {
    /// Create a new PRoot runtime with specified install directory
    pub fn new(install_dir: PathBuf) -> Self {
        PRoot { install_dir }
    }

    /// Check if PRoot is supported on this platform
    pub fn is_supported(&self) -> RuntimeResult<()> {
        let target_os = std::env::consts::OS;
        let target_arch = std::env::consts::ARCH;

        // Check if system proot is available in PATH
        if which::which("proot").is_ok() {
            return Ok(());
        }

        // Check platform support
        match (target_arch, target_os) {
            ("x86_64", "linux") => Ok(()),
            ("aarch64", "android") | ("x86_64", "android") => Ok(()),
            (arch, os) => Err(RuntimeError::NotSupported(format!(
                "PRoot cannot be automatically installed on this platform (arch: {}, os: {}).
                 Supported platforms: x86_64-linux, *-android. \
                 You can install proot manually and add it to $PATH to use it on unsupported platforms.",
                arch, os
            ))),
        }
    }

    /// Get the version of proot
    pub async fn version(&self) -> RuntimeResult<String> {
        // Try system proot first
        if which::which("proot").is_ok() {
            return self.get_system_proot_version().await;
        }

        // Try installed version
        let proot_bin = self.install_dir.join("proot");
        if proot_bin.exists() {
            return self.get_installed_proot_version(&proot_bin).await;
        }

        Err(RuntimeError::NotFound(
            "PRoot not found in PATH or install directory".to_string(),
        ))
    }

    async fn get_system_proot_version(&self) -> RuntimeResult<String> {
        let output = Command::new("proot")
            .arg("--version")
            .output()
            .map_err(|e| RuntimeError::VersionError(format!("Failed to execute proot: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}{}", stdout, stderr);

        // Parse version from output like:
        // |__|  |__|__\_____/\_____/\____| v5.4.0-5f780cba
        Self::parse_proot_version(&combined)
    }

    async fn get_installed_proot_version(&self, proot_bin: &Path) -> RuntimeResult<String> {
        let output = Command::new(proot_bin)
            .arg("--version")
            .output()
            .map_err(|e| RuntimeError::VersionError(format!("Failed to execute proot: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}{}", stdout, stderr);

        Self::parse_proot_version(&combined)
    }

    pub fn parse_proot_version(output: &str) -> RuntimeResult<String> {
        // Try to find a version number in the format: X.Y.Z or X.Y.Z-hash
        // Look for patterns like: v5.4.0, 5.4.0, v5.4.0-5f780cba, 5.4.0-5f780cba

        // First, try to find 'v' followed by a digit (common pattern)
        if let Some(pos) = output.find(|c: char| c == 'v' || c == 'V') {
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
                        && version.chars().next().map_or(false, |c| c.is_ascii_digit())
                    {
                        return Ok(version);
                    }
                }
            }
        }

        // If 'v' prefix not found, look for a bare version number (X.Y.Z pattern)
        for word in output.split_whitespace() {
            if let Some(first_char) = word.chars().next() {
                if first_char.is_ascii_digit() {
                    let version = word
                        .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '.')
                        .to_string();

                    // Check if it looks like a version (has at least one dot or dash)
                    if version.contains('.') || version.contains('-') {
                        return Ok(version);
                    }
                }
            }
        }

        Err(RuntimeError::VersionError(
            "Could not parse PRoot version from output".to_string(),
        ))
    }

    /// Get available versions for download
    pub async fn available_versions(&self) -> RuntimeResult<Vec<String>> {
        // Check if system proot is available - if so, return current version
        if which::which("proot").is_ok() {
            let version = self.get_system_proot_version().await?;
            return Ok(vec![version]);
        }

        let target_os = std::env::consts::OS;
        let target_arch = std::env::consts::ARCH;

        match (target_arch, target_os) {
            ("x86_64", "linux") => self.fetch_linux_versions().await,
            ("aarch64", "android") | ("x86_64", "android") => {
                self.fetch_android_latest_version().await
            }
            _ => Err(RuntimeError::NotSupported(
                "Unsupported platform for version fetching".to_string(),
            )),
        }
    }

    async fn fetch_linux_versions(&self) -> RuntimeResult<Vec<String>> {
        // Fetch tags from GitLab
        let client = reqwest::Client::new();
        let url = "https://gitlab.com/api/v4/projects/root%2Fproot/repository/tags";

        let response = client
            .get(url)
            .header("User-Agent", "colmap-openmvs-app")
            .send()
            .await
            .map_err(|e| RuntimeError::DownloadError(format!("Failed to fetch versions: {}", e)))?;

        #[derive(Deserialize)]
        struct GitLabTag {
            name: String,
        }

        let tags: Vec<GitLabTag> = response
            .json()
            .await
            .map_err(|e| RuntimeError::DownloadError(format!("Failed to parse versions: {}", e)))?;

        let mut versions: Vec<String> = tags.iter().map(|t| t.name.clone()).collect();
        versions.sort();
        versions.reverse(); // Most recent first

        Ok(versions)
    }

    async fn fetch_android_latest_version(&self) -> RuntimeResult<Vec<String>> {
        // Fetch from Termux package repository
        let client = reqwest::Client::new();
        let url = "https://packages.termux.dev/apt/termux-main/pool/main/p/proot/";

        let html = client
            .get(url)
            .header("User-Agent", "colmap-openmvs-app")
            .send()
            .await
            .map_err(|e| RuntimeError::DownloadError(format!("Failed to fetch versions: {}", e)))?
            .text()
            .await
            .map_err(|e| RuntimeError::DownloadError(format!("Failed to read response: {}", e)))?;

        // Parse .deb filenames to extract versions
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

        // Return only latest
        if let Some(latest) = versions.first() {
            Ok(vec![latest.clone()])
        } else {
            Err(RuntimeError::DownloadError(
                "No PRoot versions found in Termux repository".to_string(),
            ))
        }
    }

    /// Download and install a specific version of PRoot
    pub async fn download(&self, version: &str) -> RuntimeResult<()> {
        // Check if system proot is available - if so, no-op
        if which::which("proot").is_ok() {
            return Ok(());
        }

        let target_os = std::env::consts::OS;
        let target_arch = std::env::consts::ARCH;

        match (target_arch, target_os) {
            ("x86_64", "linux") => self.download_linux(version).await,
            ("aarch64", "android") | ("x86_64", "android") => self.download_android(version).await,
            _ => Err(RuntimeError::NotSupported(
                "Unsupported platform for download".to_string(),
            )),
        }
    }

    async fn download_linux(&self, version: &str) -> RuntimeResult<()> {
        // Get latest version
        let latest_versions = self.fetch_linux_versions().await?;
        let latest = latest_versions.first().ok_or(RuntimeError::DownloadError(
            "No versions available".to_string(),
        ))?;

        if version != latest {
            return Err(RuntimeError::InvalidVersion(format!(
                "Only latest version ({}) is supported for download, requested: {}",
                latest, version
            )));
        }

        // Create install directory
        fs::create_dir_all(&self.install_dir).map_err(|e| {
            RuntimeError::IoError(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to create install directory: {}", e),
            ))
        })?;

        // Download proot binary
        let url = "https://proot.gitlab.io/proot/bin/proot";
        let client = reqwest::Client::new();
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| RuntimeError::DownloadError(format!("Failed to download: {}", e)))?;

        let bytes = response
            .bytes()
            .await
            .map_err(|e| RuntimeError::DownloadError(format!("Failed to read download: {}", e)))?;

        let proot_path = self.install_dir.join("proot");
        fs::write(&proot_path, bytes).map_err(|e| {
            RuntimeError::IoError(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to write proot binary: {}", e),
            ))
        })?;

        // Make executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o755);
            fs::set_permissions(&proot_path, perms).map_err(|e| {
                RuntimeError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to set executable: {}", e),
                ))
            })?;
        }

        Ok(())
    }

    async fn download_android(&self, version: &str) -> RuntimeResult<()> {
        // Create install directory
        fs::create_dir_all(&self.install_dir).map_err(|e| {
            RuntimeError::IoError(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to create install directory: {}", e),
            ))
        })?;

        let client = reqwest::Client::new();
        let base_url = "https://packages.termux.dev/apt/termux-main/pool/main/p/proot/";

        // Download proot .deb
        let proot_deb_url = format!("{}proot_{}_aarch64.deb", base_url, version);
        self.download_and_extract_deb(&client, &proot_deb_url, "proot")
            .await?;

        // Download libtalloc .deb
        let libtalloc_deb_url = format!("{}libtalloc_{}_aarch64.deb", base_url, version);
        self.download_and_extract_deb(&client, &libtalloc_deb_url, "libtalloc")
            .await?;

        Ok(())
    }

    async fn download_and_extract_deb(
        &self,
        client: &reqwest::Client,
        url: &str,
        package_name: &str,
    ) -> RuntimeResult<()> {
        let response = client.get(url).send().await.map_err(|e| {
            RuntimeError::DownloadError(format!("Failed to download {}: {}", package_name, e))
        })?;

        let bytes = response.bytes().await.map_err(|e| {
            RuntimeError::DownloadError(format!("Failed to read {}: {}", package_name, e))
        })?;

        // Save .deb to temporary file
        let temp_deb = self.install_dir.join(format!("{}.deb", package_name));
        fs::write(&temp_deb, bytes).map_err(|e| {
            RuntimeError::IoError(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to write {}.deb: {}", package_name, e),
            ))
        })?;

        // Extract data.tar.xz from .deb
        let temp_data = self.install_dir.join("data.tar.xz");
        self.extract_from_ar(&temp_deb, &temp_data)?;

        // Extract files from data.tar.xz
        self.extract_tar_xz(&temp_data, package_name)?;

        // Cleanup
        fs::remove_file(&temp_deb).ok();
        fs::remove_file(&temp_data).ok();

        Ok(())
    }

    fn extract_from_ar(&self, ar_path: &Path, output_path: &Path) -> RuntimeResult<()> {
        // Simple ar extraction - data.tar.xz is typically at a fixed offset
        // For a proper implementation, consider using the `ar` crate
        let file_data = fs::read(ar_path).map_err(|e| {
            RuntimeError::IoError(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to read ar file: {}", e),
            ))
        })?;

        // ar format: magic (8 bytes) followed by members
        // Member header: name (16), mtime (12), uid (6), gid (6), mode (8), size (10), magic (2)
        // We need to find and extract data.tar.xz

        if file_data.len() < 8 || &file_data[0..8] != b"!<arch>\n" {
            return Err(RuntimeError::DownloadError(
                "Invalid ar archive format".to_string(),
            ));
        }

        let mut offset = 8;
        while offset + 60 <= file_data.len() {
            let name_bytes = &file_data[offset..offset + 16];
            let name = String::from_utf8_lossy(name_bytes)
                .trim_end()
                .trim_end_matches('/')
                .to_string();

            let size_bytes = &file_data[offset + 48..offset + 58];
            let size_str = String::from_utf8_lossy(size_bytes).trim_end().to_string();
            let size: usize = size_str
                .parse()
                .map_err(|_| RuntimeError::DownloadError("Invalid ar member size".to_string()))?;

            if name.contains("data.tar") {
                let data_start = offset + 60;
                fs::write(output_path, &file_data[data_start..data_start + size]).map_err(|e| {
                    RuntimeError::IoError(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Failed to write extracted data: {}", e),
                    ))
                })?;
                return Ok(());
            }

            offset += 60 + ((size + 1) & !1);
        }

        Err(RuntimeError::DownloadError(
            "data.tar.xz not found in ar archive".to_string(),
        ))
    }

    fn extract_tar_xz(&self, tar_xz_path: &Path, package_name: &str) -> RuntimeResult<()> {
        // For Android, we use a simpler approach: tar.xz extraction via tar command
        // This avoids complex xz decompression issues

        // Attempt to decompress xz using a simple decoder
        let tar_xz_data = fs::read(tar_xz_path).map_err(|e| {
            RuntimeError::IoError(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to read tar.xz file: {}", e),
            ))
        })?;

        // Use a simple XZ decompression approach
        let tar_data = self.decompress_xz(&tar_xz_data)?;

        // Extract specific files from tar
        let mut archive = tar::Archive::new(&tar_data[..]);

        for entry in archive
            .entries()
            .map_err(|e| RuntimeError::DownloadError(format!("Failed to read tar: {}", e)))?
        {
            let mut entry = entry.map_err(|e| {
                RuntimeError::DownloadError(format!("Failed to read tar entry: {}", e))
            })?;
            let path = entry.path().map_err(|e| {
                RuntimeError::DownloadError(format!("Failed to get entry path: {}", e))
            })?;

            let path_str = path.to_string_lossy();

            // Extract proot binary or libtalloc libraries
            if package_name == "proot" && path_str.ends_with("usr/bin/proot") {
                let file_path = self.install_dir.join("proot");
                entry.unpack(&file_path).map_err(|e| {
                    RuntimeError::DownloadError(format!("Failed to unpack proot: {}", e))
                })?;

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = fs::Permissions::from_mode(0o755);
                    fs::set_permissions(&file_path, perms).ok();
                }
            } else if package_name == "libtalloc" && path_str.contains("usr/lib/libtalloc") {
                let lib_path = self.install_dir.join(path.file_name().unwrap_or_default());
                entry.unpack(&lib_path).map_err(|e| {
                    RuntimeError::DownloadError(format!("Failed to unpack library: {}", e))
                })?;
            }
        }

        Ok(())
    }

    fn decompress_xz(&self, data: &[u8]) -> RuntimeResult<Vec<u8>> {
        // Decompress XZ data using xz2
        use std::io::Read;
        let mut decoder = xz2::read::XzDecoder::new(data);
        let mut output = Vec::new();
        decoder
            .read_to_end(&mut output)
            .map_err(|e| RuntimeError::DownloadError(format!("Failed to decompress xz: {}", e)))?;
        Ok(output)
    }

    /// Prepare a Docker image for execution
    pub async fn prepare(
        &self,
        image: &str,
        progress: impl Fn(PrepareProgress) + Send + Sync,
    ) -> RuntimeResult<()> {
        progress(PrepareProgress::ResolvingImage);

        // Create image directory structure
        let image_hash = self.hash_image_name(image);
        let image_dir = self.install_dir.join("images").join(&image_hash);
        let rootfs_dir = image_dir.join("rootfs");

        fs::create_dir_all(&rootfs_dir).map_err(|e| {
            RuntimeError::PrepareError(format!("Failed to create image directory: {}", e))
        })?;

        progress(PrepareProgress::Downloading {
            downloaded_bytes: 0,
            total_bytes: None,
        });

        // For now, we'll create a minimal rootfs structure
        // In a real implementation, this would download and extract Docker layers
        self.create_minimal_rootfs(&rootfs_dir)?;

        progress(PrepareProgress::WritingRootFs);

        // Create metadata file
        let metadata = ImageMetadata {
            env: HashMap::new(),
            entrypoint: None,
            cmd: None,
            working_dir: Some("/".to_string()),
        };

        let metadata_path = image_dir.join("metadata.json");
        let metadata_json = serde_json::to_string_pretty(&metadata)
            .map_err(|e| RuntimeError::SerializationError(e))?;
        fs::write(&metadata_path, metadata_json)
            .map_err(|e| RuntimeError::PrepareError(format!("Failed to write metadata: {}", e)))?;

        progress(PrepareProgress::Configuring);

        // Configure /etc/resolv.conf
        self.configure_resolv_conf(&rootfs_dir)?;

        progress(PrepareProgress::Completed);

        Ok(())
    }

    fn create_minimal_rootfs(&self, rootfs_dir: &Path) -> RuntimeResult<()> {
        let dirs = vec![
            "bin",
            "sbin",
            "usr/bin",
            "usr/sbin",
            "usr/local/bin",
            "lib",
            "lib64",
            "usr/lib",
            "etc",
            "var",
            "var/log",
            "tmp",
            "home",
            "root",
        ];

        for dir in dirs {
            fs::create_dir_all(rootfs_dir.join(dir)).map_err(|e| {
                RuntimeError::PrepareError(format!("Failed to create directory {}: {}", dir, e))
            })?;
        }

        Ok(())
    }

    fn configure_resolv_conf(&self, rootfs_dir: &Path) -> RuntimeResult<()> {
        let resolv_path = rootfs_dir.join("etc/resolv.conf");

        // Try to read host resolv.conf
        let content = if let Ok(host_content) = fs::read_to_string("/etc/resolv.conf") {
            host_content
        } else {
            // Fallback to safe defaults
            "nameserver 8.8.8.8\nnameserver 8.8.4.4\n".to_string()
        };

        fs::write(&resolv_path, content).map_err(|e| {
            RuntimeError::PrepareError(format!("Failed to write resolv.conf: {}", e))
        })?;

        Ok(())
    }

    pub fn hash_image_name(&self, image: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        image.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    /// Remove a prepared image
    pub async fn remove(&self, image: &str) -> RuntimeResult<()> {
        let image_hash = self.hash_image_name(image);
        let image_dir = self.install_dir.join("images").join(&image_hash);

        if image_dir.exists() {
            fs::remove_dir_all(&image_dir).map_err(|e| {
                RuntimeError::PrepareError(format!("Failed to remove image: {}", e))
            })?;
        }

        Ok(())
    }

    /// Execute a prepared image with given arguments
    pub async fn run(&self, image: &str, args: &[String]) -> RuntimeResult<ProcessHandle> {
        let image_hash = self.hash_image_name(image);
        let image_dir = self.install_dir.join("images").join(&image_hash);
        let rootfs_dir = image_dir.join("rootfs");

        if !rootfs_dir.exists() {
            return Err(RuntimeError::ExecutionError(format!(
                "Image not prepared: {}",
                image
            )));
        }

        // Load metadata
        let metadata_path = image_dir.join("metadata.json");
        let metadata: ImageMetadata = if metadata_path.exists() {
            let content = fs::read_to_string(&metadata_path).map_err(|e| {
                RuntimeError::ExecutionError(format!("Failed to read metadata: {}", e))
            })?;
            serde_json::from_str(&content).map_err(|e| RuntimeError::SerializationError(e))?
        } else {
            ImageMetadata {
                env: HashMap::new(),
                entrypoint: None,
                cmd: None,
                working_dir: Some("/".to_string()),
            }
        };

        // Find proot binary
        let proot_bin = if which::which("proot").is_ok() {
            "proot".to_string()
        } else {
            let custom_proot = self.install_dir.join("proot");
            if custom_proot.exists() {
                custom_proot.to_string_lossy().to_string()
            } else {
                return Err(RuntimeError::ExecutionError(
                    "PRoot binary not found".to_string(),
                ));
            }
        };

        // Build command
        let mut cmd = Command::new(&proot_bin);

        // Set environment variables (without inheriting host environment)
        cmd.env_clear();
        for (key, value) in &metadata.env {
            cmd.env(key, value);
        }

        // Add default environment
        cmd.env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        );
        cmd.env("HOME", "/root");

        // Set root filesystem
        cmd.arg("-R").arg(&rootfs_dir);

        // Set working directory if specified
        if let Some(workdir) = &metadata.working_dir {
            cmd.arg("-w").arg(workdir);
        }

        // Add entrypoint and cmd
        let mut full_cmd = Vec::new();

        if let Some(entrypoint) = &metadata.entrypoint {
            full_cmd.extend(entrypoint.clone());
        }

        if let Some(cmd_args) = &metadata.cmd {
            full_cmd.extend(cmd_args.clone());
        }

        // Add user-provided arguments
        full_cmd.extend(args.iter().cloned());

        // If we have a command to execute
        if !full_cmd.is_empty() {
            for arg in full_cmd {
                cmd.arg(arg);
            }
        }

        // Setup piped IO for async operations
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| RuntimeError::ExecutionError(format!("Failed to spawn process: {}", e)))?;

        Ok(ProcessHandle { child })
    }
}
