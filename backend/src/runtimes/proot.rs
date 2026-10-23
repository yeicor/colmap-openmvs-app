use super::{
    proot_deb, shared, shared::*, ImageHash, ImageTag, Mount, PrepareProgressTx, PreparedImage,
    ProcessHandle, Runtime, RuntimeResult,
};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{debug, error, info, trace, warn};
// ---------------------------------------------------------------------------
// PRoot binary detection
// ---------------------------------------------------------------------------

/// Locate the proot binary using multiple fallback strategies.
///
/// Strategy order:
/// 1. `libproot.so` in `runtime_dir` (Android APK embedded asset, *.so naming)
/// 2. Plain `proot` in `runtime_dir` (runtime-downloaded binary)
/// 3. Direct execution attempt (`proot --version`)
/// 4. `which::which("proot")` (system PATH search)
async fn find_proot_binary(runtime_dir: &Path) -> RuntimeResult<String> {
    info!(runtime_dir = %runtime_dir.display(), "find_proot_binary: starting search");

    // Strategy 1: Look for 'libproot.so' in runtime_dir (Android APK asset, *.so naming)
    let embedded_proot = runtime_dir.join("libproot.so");
    if embedded_proot.exists() {
        let abs_path = embedded_proot
            .canonicalize()
            .unwrap_or_else(|_| embedded_proot.clone())
            .to_string_lossy()
            .into_owned();
        info!(path = %abs_path, "find_proot_binary: found embedded libproot.so");
        return Ok(abs_path);
    }

    // Strategy 2: Look for plain 'proot' in runtime_dir (runtime-downloaded binary)
    let downloaded_proot = runtime_dir.join("proot");
    if downloaded_proot.exists() {
        let abs_path = downloaded_proot
            .canonicalize()
            .unwrap_or_else(|_| downloaded_proot.clone())
            .to_string_lossy()
            .into_owned();
        info!(path = %abs_path, "find_proot_binary: found downloaded proot");
        return Ok(abs_path);
    }

    // Strategy 3: Try direct execution (proot might be in system PATH)
    info!("find_proot_binary: attempting direct execution of 'proot --version'");
    if Command::new("proot")
        .arg("--version")
        .output()
        .await
        .is_ok()
    {
        // Try to get the absolute path from which::which()
        if let Ok(abs_path_buf) = which::which("proot") {
            let abs_path = abs_path_buf.to_string_lossy().into_owned();
            info!(path = %abs_path, "find_proot_binary: direct execution succeeded, resolved to absolute path");
            return Ok(abs_path);
        }
        info!("find_proot_binary: direct execution succeeded, 'proot' is in system PATH");
        return Ok("proot".to_string());
    }

    // Strategy 4: Fall back to which (may be cached, least reliable)
    info!("find_proot_binary: attempting lookup via which::which()");
    if let Ok(abs_path_buf) = which::which("proot") {
        let abs_path = abs_path_buf.to_string_lossy().into_owned();
        info!(path = %abs_path, "find_proot_binary: which::which() found proot");
        return Ok(abs_path);
    }

    // All strategies failed
    let path_info = std::env::var("PATH").unwrap_or_else(|_| "(not set)".to_string());
    error!(
        runtime_dir = %runtime_dir.display(),
        current_path = %path_info,
        "find_proot_binary: all strategies failed to locate proot binary"
    );
    Err(anyhow::anyhow!(
        "PRoot binary not found. Checked: runtime_dir={}, system PATH. Current PATH: {}. Ensure PRoot is installed and accessible.",
        runtime_dir.display(),
        path_info
    ))
}

/// Set up a proot [`Command`] with no special environment variables.
///
/// With patchelf setting RPATH=$ORIGIN, libproot.so will automatically find
/// libtalloc.so and libandroid-shmem.so in the same directory. No special
/// LD_LIBRARY_PATH setup needed.
fn setup_proot_command(proot_bin: &str) -> Command {
    let cmd = Command::new(proot_bin);
    info!(proot_bin = %proot_bin, "setup_proot_command: initializing");
    cmd
}

/// Apply the environment variables that PRoot itself needs on this platform.
///
/// These variables configure the PRoot process itself, they are not part of
/// the environment of the guest.
fn setup_proot_environment(cmd: &mut Command) {
    // PRoot accelerates the syscall interception with a seccomp filter that
    // is broken on the kernels affected by Ubuntu bug #1202161: the guest
    // then dies with SIGBUS or its paths are translated wrongly, so every
    // command fails.  PRoot itself suggests this variable as the workaround.
    #[cfg(target_os = "linux")]
    cmd.env("PROOT_NO_SECCOMP", "1");
}

/// Whether this process is already traced by a ptrace-based sandbox.
///
/// PRoot is implemented with ptrace and cannot reliably run inside another
/// instance of itself: the inner loader can fail with exit status 182 before
/// producing any output.  DroidDeck on Android runs Linux applications inside
/// PRoot, so the Linux build of this app is nested when launched from there
/// (the Android build is not).
#[cfg(target_os = "linux")]
fn is_running_under_ptrace() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| tracer_pid_from_status(&status))
        .is_some_and(|pid| pid != 0)
}

/// Extract `TracerPid` from the contents of `/proc/<pid>/status`.
#[cfg(target_os = "linux")]
fn tracer_pid_from_status(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("TracerPid:"))
        .and_then(|pid| pid.trim().parse::<u32>().ok())
}

/// The PRoot build that is downloaded as a last resort when the installed
/// binary cannot run in a nested sandbox.
///
/// Upstream 5.4.0 is the newest release that still works inside the Termux
/// PRoot fork used by DroidDeck.  5.1.x crashes with SIGSEGV and 5.4.1+ also
/// crashes, so this exact version matters.
#[cfg(target_os = "linux")]
const PORTABLE_NESTED_PROOT_VERSION: &str = "5.4.0";

/// The PRoot binary that was verified to work in a nested sandbox.
///
/// This is a path, not just a flag: the fallback may be a portable binary
/// stored next to the installed one rather than replacing it.
#[cfg(target_os = "linux")]
static NESTED_PROOT_SELECTION: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Serializes the one-time nested-PRoot check between concurrent pipeline runs.
#[cfg(target_os = "linux")]
static NESTED_PROOT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Forget the nested-PRoot verification, for example after the installed
/// binary was replaced or removed.
#[cfg(target_os = "linux")]
fn reset_nested_proot_check() {
    *NESTED_PROOT_SELECTION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Return the cached nested-PRoot selection if its binary still exists.
#[cfg(target_os = "linux")]
fn cached_nested_proot() -> Option<String> {
    let selection = NESTED_PROOT_SELECTION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    selection.filter(|path| Path::new(path).is_file())
}

/// Remember which PRoot binary can run in a nested sandbox.
#[cfg(target_os = "linux")]
fn cache_nested_proot(path: &str) {
    *NESTED_PROOT_SELECTION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(path.to_string());
}

#[cfg(not(target_os = "linux"))]
fn reset_nested_proot_check() {}

/// Whether the process is running inside an outer ptrace sandbox.
#[cfg(not(target_os = "linux"))]
fn nested_sandbox_detected() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn nested_sandbox_detected() -> bool {
    // `TracerPid` is the reliable signal, but DroidDeck also exports
    // `PROOT_LOADER`/`PROOT_TMP_DIR` to every process it starts.  Checking
    // them as well keeps the detection working on kernels where `/proc` is
    // restricted or emulated.
    is_running_under_ptrace()
        || std::env::var_os("PROOT_LOADER").is_some()
        || std::env::var_os("PROOT_TMP_DIR").is_some()
}

/// Guest command used to check whether a PRoot binary can start processes.
///
/// `/bin/true` is the cheapest option, but some images only ship a shell or
/// BusyBox.  A probe that cannot find any executable would otherwise be
/// mistaken for "this PRoot does not work".
#[cfg(target_os = "linux")]
fn probe_guest_command(rootfs_dir: &Path) -> Vec<&'static str> {
    if rootfs_dir.join("bin/true").exists() {
        vec!["/bin/true"]
    } else if rootfs_dir.join("bin/sh").exists() {
        vec!["/bin/sh", "-c", "true"]
    } else if rootfs_dir.join("usr/bin/env").exists() {
        vec!["/usr/bin/env", "true"]
    } else if rootfs_dir.join("bin/busybox").exists() {
        vec!["/bin/busybox"]
    } else {
        // Let PRoot report the missing executable.  This yields `Failed`
        // rather than `Incompatible`, so the installed binary is kept.
        vec!["/bin/true"]
    }
}

/// Result of probing whether a PRoot binary can start guest programs.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootfsProbe {
    /// The guest command started and exited successfully.
    Ok,
    /// The PRoot loader failed (exit status 182 / signal), which is how
    /// nested PRoot builds report address conflicts.
    Incompatible,
    /// The probe failed for another reason (for example the image has no
    /// `/bin/true`), so the installed PRoot should not be replaced.
    Failed,
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

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
}

impl PRoot {
    /// Create a PRoot runtime with separate directories.
    pub fn new(runtime_dir: PathBuf, images_dir: PathBuf) -> Self {
        PRoot {
            runtime_dir,
            images_dir,
        }
    }
    pub fn new_default_images(runtime_dir: PathBuf) -> Self {
        Self::new(runtime_dir.clone(), runtime_dir.join("images"))
    }
    pub fn new_simple(runtime_dir: PathBuf) -> Self {
        Self::new(runtime_dir.clone(), runtime_dir.join("images"))
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

    /// List the PRoot versions that can be installed on Linux.
    ///
    /// The versions are the ones of the distribution packages that are
    /// published for the architecture of the host.
    async fn fetch_linux_versions(&self) -> RuntimeResult<Vec<String>> {
        let packages = proot_deb::fetch_proot_packages()
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        let versions: Vec<String> = packages
            .iter()
            .filter(|info| info.compatible)
            .map(|info| info.package.version.clone())
            .collect();

        if versions.is_empty() {
            return Err(anyhow::anyhow!(
                "No PRoot package compatible with this system was found"
            ));
        }

        Ok(versions)
    }

    /// List the PRoot versions that can be installed on this host.
    ///
    /// Unlike [`Runtime::available_versions`] this always queries the
    /// distribution repositories, so another version can be selected even
    /// when one is already installed.  When the repository cannot be reached
    /// the installed version (if any) is reported instead.
    pub(crate) async fn installable_versions(&self) -> RuntimeResult<Vec<String>> {
        #[cfg(target_os = "linux")]
        if let Ok(versions) = self.fetch_linux_versions().await {
            if !versions.is_empty() {
                return Ok(versions);
            }
        }

        self.available_versions().await
    }

    async fn fetch_android_latest_version(&self) -> RuntimeResult<Vec<String>> {
        debug!("fetch_android_latest_version: starting");
        let url = "https://packages.termux.dev/apt/termux-main/pool/main/p/proot/";

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .user_agent("colmap-openmvs-app")
            .build()
            .into();

        debug!(url = %url, "fetch_android_latest_version: fetching from Termux repository");
        let html = agent
            .get(url)
            .call()
            .map_err(|e| anyhow::anyhow!("Failed to fetch versions: {}", e))?
            .into_body()
            .read_to_string()
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
                        trace!(version = %version_part, "fetch_android_latest_version: found version");
                        versions.push(version_part.to_string());
                    }
                }
            }
        }

        versions.sort();
        versions.reverse();
        versions.dedup();

        debug!(versions = ?versions, "fetch_android_latest_version: parsed versions");
        if versions.is_empty() {
            debug!("fetch_android_latest_version: no versions found in Termux repository");
        }
        versions
            .into_iter()
            .next()
            .map(|v| vec![v])
            .ok_or_else(|| anyhow::anyhow!("No PRoot versions found in Termux repository"))
    }

    async fn fetch_libandroid_shmem_version(&self) -> RuntimeResult<Vec<String>> {
        debug!("fetch_libandroid_shmem_version: starting");
        let url = "https://packages.termux.dev/apt/termux-main/pool/main/liba/libandroid-shmem/";

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .user_agent("colmap-openmvs-app")
            .build()
            .into();

        debug!(url = %url, "fetch_libandroid_shmem_version: fetching from Termux repository");
        let html = agent
            .get(url)
            .call()
            .map_err(|e| anyhow::anyhow!("Failed to fetch libandroid-shmem versions: {}", e))?
            .into_body()
            .read_to_string()
            .map_err(|e| anyhow::anyhow!("Failed to read libandroid-shmem response: {}", e))?;

        debug!(
            html_length = html.len(),
            "fetch_libandroid_shmem_version: received HTML"
        );
        let mut versions = Vec::new();
        for line in html.lines() {
            if let Some(start) = line.find("libandroid-shmem_") {
                if let Some(end) = line[start..].find("_aarch64.deb") {
                    let filename = &line[start..start + end];
                    if let Some(version_part) = filename.strip_prefix("libandroid-shmem_") {
                        trace!("fetch_libandroid_shmem_version: found version");
                        versions.push(version_part.to_string());
                    }
                }
            }
        }

        versions.sort();
        versions.reverse();
        versions.dedup();

        debug!(versions = ?versions, "fetch_libandroid_shmem_version: parsed versions");
        if versions.is_empty() {
            debug!("fetch_libandroid_shmem_version: no versions found in Termux repository");
        }
        versions.into_iter().next().map(|v| vec![v]).ok_or_else(|| {
            anyhow::anyhow!("No libandroid-shmem versions found in Termux repository")
        })
    }

    async fn fetch_libtalloc_version(&self) -> RuntimeResult<Vec<String>> {
        debug!("fetch_libtalloc_version: starting");
        let url = "https://packages.termux.dev/apt/termux-main/pool/main/libt/libtalloc/";

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .user_agent("colmap-openmvs-app")
            .build()
            .into();

        debug!(url = %url, "fetch_libtalloc_version: fetching from Termux repository");
        let html = agent
            .get(url)
            .call()
            .map_err(|e| anyhow::anyhow!("Failed to fetch libtalloc versions: {}", e))?
            .into_body()
            .read_to_string()
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
                        trace!("fetch_libtalloc_version: found version");
                        versions.push(version_part.to_string());
                    }
                }
            }
        }

        versions.sort();
        versions.reverse();
        versions.dedup();

        debug!(versions = ?versions, "fetch_libtalloc_version: parsed versions");
        if versions.is_empty() {
            debug!("fetch_libtalloc_version: no versions found in Termux repository");
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

    /// Install PRoot on Linux from a distribution package.
    ///
    /// Upstream only publishes a pre-compiled binary for `x86_64`, so the
    /// binary is taken from the Debian/Ubuntu package pools.  The binary is
    /// extracted from the `.deb` package and — if the host does not provide
    /// `libtalloc` — the required shared library is bundled next to it.
    async fn download_linux(&self, version: &str) -> RuntimeResult<()> {
        info!(
            version = %version,
            "download_linux: installing PRoot from a distribution package"
        );

        let installed = proot_deb::install_proot(&self.runtime_dir, version)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        info!(
            version = %installed,
            runtime_dir = %self.runtime_dir.display(),
            "download_linux: PRoot installed"
        );
        Ok(())
    }

    async fn download_android(&self, version: &str) -> RuntimeResult<()> {
        info!(version = %version, "download_android: starting Android PRoot download");

        debug!(runtime_dir = %self.runtime_dir.display(), "download_android: creating runtime directory");
        tokio::fs::create_dir_all(&self.runtime_dir)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create runtime directory: {}", e))?;

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .user_agent("colmap-openmvs-app")
            .build()
            .into();
        let proot_base_url = "https://packages.termux.dev/apt/termux-main/pool/main/p/proot/";

        let proot_url = format!("{}proot_{}_aarch64.deb", proot_base_url, version);
        info!(version = %version, url = %proot_url, "download_android: downloading PRoot");
        self.download_and_extract_deb(&agent, &proot_url, "proot")
            .await?;
        debug!("download_android: PRoot download completed");

        // Fetch the latest libtalloc version from the correct directory
        debug!("download_android: fetching libtalloc version");
        let libtalloc_versions = self.fetch_libtalloc_version().await?;
        let libtalloc_version = libtalloc_versions
            .first()
            .ok_or_else(|| anyhow::anyhow!("No libtalloc version found"))?
            .clone();
        debug!(libtalloc_version = %libtalloc_version, "download_android: libtalloc version fetched");

        let libtalloc_base_url =
            "https://packages.termux.dev/apt/termux-main/pool/main/libt/libtalloc/";
        let talloc_url = format!(
            "{}libtalloc_{}_aarch64.deb",
            libtalloc_base_url, libtalloc_version
        );
        info!(libtalloc_version = %libtalloc_version, url = %talloc_url, "download_android: downloading libtalloc");
        self.download_and_extract_deb(&agent, &talloc_url, "libtalloc")
            .await?;
        debug!("download_android: libtalloc download completed");

        // Fetch and download libandroid-shmem
        debug!("download_android: fetching libandroid-shmem version");
        let libandroid_shmem_versions = self.fetch_libandroid_shmem_version().await?;
        let libandroid_shmem_version = libandroid_shmem_versions
            .first()
            .ok_or_else(|| anyhow::anyhow!("No libandroid-shmem version found"))?
            .clone();
        debug!(libandroid_shmem_version = %libandroid_shmem_version, "download_android: libandroid-shmem version fetched");

        let libandroid_shmem_base_url =
            "https://packages.termux.dev/apt/termux-main/pool/main/liba/libandroid-shmem/";
        let shmem_url = format!(
            "{}libandroid-shmem_{}_aarch64.deb",
            libandroid_shmem_base_url, libandroid_shmem_version
        );
        info!(libandroid_shmem_version = %libandroid_shmem_version, url = %shmem_url, "download_android: downloading libandroid-shmem");
        self.download_and_extract_deb(&agent, &shmem_url, "libandroid-shmem")
            .await?;
        debug!("download_android: libandroid-shmem download completed");

        info!(version = %version, "download_android: Android installation completed");
        Ok(())
    }

    async fn download_and_extract_deb(
        &self,
        agent: &ureq::Agent,
        url: &str,
        package_name: &str,
    ) -> RuntimeResult<()> {
        info!(package_name = %package_name, url = %url, "download_and_extract_deb: starting download");

        // Fetch bytes (HTTP errors are returned as Err by ureq by default)
        let mut response = agent
            .get(url)
            .call()
            .map_err(|e| anyhow::anyhow!("Failed to download {}: {}", package_name, e))?;
        let bytes = response
            .body_mut()
            .with_config()
            .limit(2 * 1024 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", package_name, e))?;

        debug!(package_name = %package_name, size = bytes.len(), "download_and_extract_deb: bytes received");

        // Validate that we received a valid ar archive (deb file)
        trace!(package_name = %package_name, "download_and_extract_deb: validating ar archive format");
        if bytes.len() < 8 || &bytes[0..8] != b"!<arch>\n" {
            error!(package_name = %package_name, size = bytes.len(), url = %url, "download_and_extract_deb: invalid deb archive format");
            return Err(anyhow::anyhow!(
                "Invalid deb package received for {}: not an ar archive. URL: {}",
                package_name,
                url
            ));
        }
        debug!(package_name = %package_name, "download_and_extract_deb: archive format is valid");

        // All subsequent work is CPU/disk-bound — offload to a blocking thread.
        let install_dir = self.runtime_dir.clone();
        let pkg = package_name.to_string();

        trace!(package_name = %package_name, "download_and_extract_deb: offloading to blocking task");
        tokio::task::spawn_blocking(move || {
            let temp_deb = install_dir.join(format!("{}.deb", pkg));
            let temp_data = install_dir.join("data.tar.xz");

            std::fs::write(&temp_deb, &bytes)
                .map_err(|e| anyhow::anyhow!("Failed to write {}.deb: {}", pkg, e))?;

            extract_from_ar_sync(&temp_deb, &temp_data)?;
            extract_tar_xz_sync(&install_dir, &temp_data, &pkg)?;

            std::fs::remove_file(&temp_deb).ok();
            std::fs::remove_file(&temp_data).ok();

            debug!(package_name = %pkg, "download_and_extract_deb: extraction completed");
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
                // Use metadata() which follows symlinks automatically
                if let Ok(meta) = entry.metadata().await {
                    if meta.is_dir() {
                        // Recurse into directory
                        size += Box::pin(self.calculate_dir_size_async(&entry.path())).await?;
                    } else {
                        // Count file size (works for both regular files and symlink targets)
                        // metadata() follows symlinks, so we get the real file size
                        size += meta.len();
                    }
                }
            }
        }
        Ok(size)
    }

    // -----------------------------------------------------------------------
    // Embedded asset helpers (Android rootfs.zip manifest)
    // -----------------------------------------------------------------------

    /// Read and parse the embedded [`RootfsManifest`] from the compile-time
    /// `rootfs.zip` (which contains `.rootfs_manifest.json`).
    /// On non-Android targets this always returns an error.
    pub(crate) async fn read_embedded_manifest(&self) -> RuntimeResult<RootfsManifest> {
        #[cfg(not(target_os = "android"))]
        {
            anyhow::bail!("No embedded rootfs.zip (non-Android target)")
        }
        #[cfg(target_os = "android")]
        {
            let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/rootfs.zip"));
            let cursor = std::io::Cursor::new(bytes);
            let mut archive = zip::ZipArchive::new(cursor)
                .map_err(|e| anyhow::anyhow!("Failed to open embedded rootfs.zip: {e}"))?;
            let mut entry = archive
                .by_name(".rootfs_manifest.json")
                .map_err(|_| anyhow::anyhow!(".rootfs_manifest.json not found in rootfs.zip"))?;
            let mut content = String::new();
            std::io::Read::read_to_string(&mut entry, &mut content)
                .map_err(|e| anyhow::anyhow!("Failed to read manifest from zip: {e}"))?;
            serde_json::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Failed to parse manifest JSON: {e}"))
        }
    }

    /// Build the lightweight rootfs skeleton from the embedded manifest.
    ///
    /// The skeleton consists of:
    /// - Directories recreated under `images_dir/<tag>/rootfs/`
    /// - Symlinks for ELF → `/mnt/jni/librootfs-<hash>.so`
    /// - Original rootfs symlinks (directory aliases, file aliases)
    ///
    /// On Android:
    /// - ELF files live in jniLibs (direct .so from APK)
    /// - Non-ELF files are extracted from assets/rootfs.zip to filesDir
    /// - proot is invoked with `-b <jniLibs>:/mnt/jni` so ELF symlink
    ///   targets resolve inside the container.
    ///
    /// On non-Android:
    /// - All files are downloaded and extracted to the writable directory.
    /// - No symlinks are needed for file access.
    ///
    /// NOTE: On Android, `setup_android_runtime()` should be called first
    /// which handles both ELF and non-ELF. This method is a fallback path.
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

        let total = (manifest.files.len() + manifest.symlinks.len()).max(1);
        let mut done = 0usize;

        // ── Create per-file symlinks ──
        // The manifest only contains ELF files; non-ELF files are extracted
        // from rootfs.zip at startup (see android_startup.rs).
        // Directories are created on-demand as parent dirs of each file path.

        // On Android, resolve jniLibs path so symlinks point directly to it.
        #[allow(unused_assignments, unused_mut)]
        let mut jnilib_base_path = String::new();
        #[cfg(target_os = "android")]
        {
            if let Some(p) = crate::settings::get_android_native_lib_dir() {
                jnilib_base_path = p;
            }
        }

        for (hash, file_info) in &manifest.files {
            let dest = rootfs_dir.join(file_info.path.trim_start_matches('/'));
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await.ok();
            }
            let _ = tokio::fs::remove_file(&dest).await;
            #[cfg(unix)]
            {
                let symlink_target = if jnilib_base_path.is_empty() {
                    // Non-Android fallback: placeholder relative symlink.
                    format!("librootfs-{}.so", hash)
                } else {
                    // Android: absolute symlink to the jniLibs location.
                    format!("{}/librootfs-{}.so", jnilib_base_path, hash)
                };
                if let Err(e) = tokio::fs::symlink(&symlink_target, &dest).await {
                    warn!(
                        path = %dest.display(),
                        target = %symlink_target,
                        error = %e,
                        "setup_rootfs_skeleton: failed to create symlink"
                    );
                }
            }
            done += 1;
            if done.is_multiple_of(500) {
                let _ = tx
                    .send(colmap_openmvs_api::PrepareProgress::ExtractingLayer {
                        layer: "Building rootfs skeleton".to_string(),
                        progress: done as f32 / total as f32 * 0.5,
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
            {
                if let Err(e) = tokio::fs::symlink(target, &dest).await {
                    warn!(
                        path = %dest.display(),
                        target = %target,
                        error = %e,
                        "setup_rootfs_skeleton: failed to create original symlink"
                    );
                }
            }
            done += 1;
        }

        // ── Write metadata ───────────────────────────────────────────────
        #[allow(unused_assignments)]
        let _ = done; // done is only used for progress reporting in the above loops
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

    /// Check if the given image tag is the embedded image on Android.
    /// Returns false on non-Android targets.
    #[cfg_attr(not(target_os = "android"), allow(unused_variables))]
    fn is_embedded_image(&self, image_tag: &str) -> bool {
        #[cfg(target_os = "android")]
        {
            if let Some(embedded_tag) = crate::settings::read_embedded_image_tag_public() {
                return image_tag == embedded_tag;
            }
        }
        false
    }

    /// Run a trivial command in the image to check that PRoot can actually
    /// start guest processes.
    #[cfg(target_os = "linux")]
    async fn probe_rootfs(&self, rootfs_dir: &Path, proot_bin: &str) -> RootfsProbe {
        let mut cmd = setup_proot_command(proot_bin);
        cmd.env_clear();
        setup_proot_environment(&mut cmd);

        // PRoot extracts its loader into a temporary directory.  Use a
        // directory owned by the app instead of relying on `/tmp`, which may
        // be missing or read-only in the guest image.
        let tmp_dir = self.images_dir.join(".proot-tmp");
        let _ = std::fs::create_dir_all(&tmp_dir);
        if let Some(tmp_str) = tmp_dir.to_str() {
            cmd.env("PROOT_TMP_DIR", tmp_str);
        }

        cmd.arg("-R").arg(rootfs_dir).arg("-w").arg("/");
        for arg in probe_guest_command(rootfs_dir) {
            cmd.arg(arg);
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                debug!(proot_bin, error = %e, "rootfs probe could not start PRoot");
                return RootfsProbe::Failed;
            }
        };
        match tokio::time::timeout(std::time::Duration::from_secs(30), child.wait()).await {
            Ok(Ok(status)) if status.success() => RootfsProbe::Ok,
            Ok(Ok(status)) => {
                let mut message = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    use tokio::io::AsyncReadExt;
                    let _ = stderr.read_to_string(&mut message).await;
                }
                debug!(
                    proot_bin,
                    ?status,
                    stderr = %message,
                    "rootfs probe failed"
                );
                // 182 is PRoot's loader `FATAL()` exit code.  A signal means the
                // inner tracer crashed; both happen when PRoot is nested under an
                // incompatible PRoot version.
                if status.code() == Some(182) || status.code().is_none() {
                    RootfsProbe::Incompatible
                } else {
                    RootfsProbe::Failed
                }
            }
            result => {
                debug!(proot_bin, ?result, "rootfs probe timed out");
                let _ = child.kill().await;
                RootfsProbe::Failed
            }
        }
    }

    /// Return a portable PRoot 5.4.0 build that can run nested.
    ///
    /// Distribution packages are built for the newest Debian/Ubuntu release,
    /// so on older guests (for example the Debian 12 rootfs used by
    /// DroidDeck) the only nested-compatible version cannot be installed at
    /// all: its binary requires a newer glibc.  Release builds therefore
    /// publish a statically linked 5.4.0 binary for x86_64 and aarch64, which
    /// is downloaded here when needed.
    #[cfg(target_os = "linux")]
    async fn portable_nested_proot(&self) -> RuntimeResult<String> {
        let arch = std::env::consts::ARCH;
        if !matches!(arch, "x86_64" | "aarch64") {
            return Err(anyhow::anyhow!(
                "no portable PRoot build is available for {arch}"
            ));
        }

        let file_name = format!("proot-{PORTABLE_NESTED_PROOT_VERSION}-{arch}");
        let local = self.runtime_dir.join(&file_name);
        if local.is_file() {
            return Ok(local.to_string_lossy().into_owned());
        }

        // Release builds may also ship the binary next to the application.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                for name in [
                    format!("proot-{PORTABLE_NESTED_PROOT_VERSION}-linux-{arch}"),
                    file_name.clone(),
                ] {
                    let candidate = dir.join(name);
                    if candidate.is_file() {
                        return Ok(candidate.to_string_lossy().into_owned());
                    }
                }
            }
        }

        let url = format!(
            "https://github.com/yeicor/colmap-openmvs-app/releases/latest/download/proot-{}-linux-{}",
            PORTABLE_NESTED_PROOT_VERSION, arch
        );
        info!(url = %url, "nested PRoot: downloading portable build");

        tokio::fs::create_dir_all(&self.runtime_dir)
            .await
            .map_err(|e| anyhow::anyhow!("cannot create the PRoot runtime directory: {e}"))?;
        let destination = local.clone();
        let download_url = url.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .user_agent(concat!("colmap-openmvs-app/", env!("CARGO_PKG_VERSION")))
                .build()
                .into();
            let mut response = agent
                .get(&download_url)
                .call()
                .map_err(|e| anyhow::anyhow!("failed to download the portable PRoot build: {e}"))?;
            let bytes = response
                .body_mut()
                .with_config()
                .limit(64 * 1024 * 1024)
                .read_to_vec()
                .map_err(|e| anyhow::anyhow!("failed to read the portable PRoot build: {e}"))?;
            if !bytes.starts_with(b"\x7fELF") {
                return Err(anyhow::anyhow!(
                    "the portable PRoot download is not an ELF binary"
                ));
            }
            let temporary = destination.with_file_name(format!("{file_name}.part"));
            std::fs::write(&temporary, &bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))?;
            }
            std::fs::rename(&temporary, &destination)?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("portable PRoot download task failed: {e}"))??;

        Ok(local.to_string_lossy().into_owned())
    }

    /// Make sure the installed PRoot can run guest programs when this process
    /// is itself running under another ptrace sandbox (nested PRoot).
    ///
    /// Not every PRoot build can be nested: 5.1.x crashes with SIGSEGV and
    /// 5.4.1+ crashes as well when started inside the Termux PRoot fork used
    /// by DroidDeck.  Upstream 5.4.0 is the version that works.  When the
    /// installed binary fails the probe, try the portable 5.4.0 build and
    /// then the compatible distribution versions (newest first, so both the
    /// Android/DroidDeck and the older-glibc cases are covered).
    ///
    /// Returns the binary that should be used for the actual run.
    async fn ensure_nested_proot_works(
        &self,
        rootfs_dir: &Path,
        proot_bin: String,
        nested: bool,
    ) -> RuntimeResult<String> {
        #[cfg(target_os = "linux")]
        {
            if nested {
                if let Some(selected) = cached_nested_proot() {
                    return Ok(selected);
                }

                let _guard = NESTED_PROOT_LOCK.lock().await;
                if let Some(selected) = cached_nested_proot() {
                    return Ok(selected);
                }

                match self.probe_rootfs(rootfs_dir, &proot_bin).await {
                    RootfsProbe::Ok => {
                        cache_nested_proot(&proot_bin);
                        return Ok(proot_bin);
                    }
                    RootfsProbe::Failed => {
                        warn!(
                            proot_bin = %proot_bin,
                            "nested PRoot: the probe could not run a guest command; \
                             keeping the installed PRoot"
                        );
                        return Ok(proot_bin);
                    }
                    RootfsProbe::Incompatible => {
                        warn!(
                            proot_bin = %proot_bin,
                            "nested PRoot: the installed PRoot cannot start guest processes"
                        );
                    }
                }

                let mut tried = vec![format!("{proot_bin} (installed)")];

                // Prefer the portable static build: it works independently of
                // the guest's glibc, which is what makes 5.4.0 usable on the
                // older Debian/Ubuntu rootfs images used by DroidDeck.  Unit
                // tests may pre-place a portable binary; without one they
                // must not start a download.
                let portable_local = self.runtime_dir.join(format!(
                    "proot-{PORTABLE_NESTED_PROOT_VERSION}-{}",
                    std::env::consts::ARCH
                ));
                if !cfg!(test) || portable_local.is_file() {
                    match self.portable_nested_proot().await {
                        Ok(candidate) => {
                            info!(
                                candidate = %candidate,
                                "nested PRoot: probing portable build"
                            );
                            match self.probe_rootfs(rootfs_dir, &candidate).await {
                                RootfsProbe::Ok => {
                                    info!(
                                        candidate = %candidate,
                                        "nested PRoot: using portable build"
                                    );
                                    cache_nested_proot(&candidate);
                                    return Ok(candidate);
                                }
                                other => {
                                    warn!(
                                        candidate = %candidate,
                                        probe = ?other,
                                        "nested PRoot: portable build cannot start guest processes"
                                    );
                                    tried.push(format!(
                                        "portable {} ({other:?})",
                                        PORTABLE_NESTED_PROOT_VERSION
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "nested PRoot: portable build is not available");
                            tried.push(format!(
                                "portable {} (unavailable)",
                                PORTABLE_NESTED_PROOT_VERSION
                            ));
                        }
                    }
                }

                let installed = self.version().await.ok();
                let versions = self.installable_versions().await.unwrap_or_default();
                for version in &versions {
                    if installed.as_deref() == Some(version.as_str()) {
                        continue;
                    }
                    info!(version = %version, "nested PRoot: trying fallback version");
                    if let Err(e) = self.download_linux(version).await {
                        debug!(
                            version = %version,
                            error = %e,
                            "nested PRoot: fallback download failed"
                        );
                        tried.push(format!("{version} (download failed)"));
                        continue;
                    }
                    let candidate = match find_proot_binary(&self.runtime_dir).await {
                        Ok(candidate) => candidate,
                        Err(_) => {
                            tried.push(format!("{version} (binary not found)"));
                            continue;
                        }
                    };
                    debug!(
                        version = %version,
                        candidate = %candidate,
                        "nested PRoot: probing fallback version"
                    );
                    match self.probe_rootfs(rootfs_dir, &candidate).await {
                        RootfsProbe::Ok => {
                            info!(version = %version, "nested PRoot: using fallback version");
                            cache_nested_proot(&candidate);
                            return Ok(candidate);
                        }
                        other => {
                            debug!(
                                version = %version,
                                probe = ?other,
                                "nested PRoot: fallback probe failed"
                            );
                            tried.push(format!("{version} ({other:?})"));
                        }
                    }
                }

                return Err(anyhow::anyhow!(
                    "PRoot cannot run inside another PRoot sandbox (nested PRoot). \
                     This happens when the Linux build is started from DroidDeck on \
                     Android, where the app already runs under PRoot. No compatible \
                     PRoot build could start a guest program (tried: {}). Use the \
                     Android app build, or start the Linux build outside DroidDeck.",
                    tried.join(", ")
                ));
            }
        }

        #[cfg(not(target_os = "linux"))]
        let _ = (rootfs_dir, nested);

        Ok(proot_bin)
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
        if self.runtime_dir.join("libproot.so").exists() {
            return Ok(());
        }

        let target_os = std::env::consts::OS;
        let target_arch = std::env::consts::ARCH;

        if which::which("proot").is_ok() {
            return Ok(());
        }

        match (target_arch, target_os) {
            ("x86_64", "linux") | ("aarch64", "linux") => Ok(()),
            ("aarch64", "android") | ("x86_64", "android") => Ok(()),
            (arch, os) => Err(anyhow::anyhow!(
                "PRoot cannot be automatically installed on this platform \\n                 (arch: {arch}, os: {os}). Supported: x86_64-linux, aarch64-linux, *-android. \\n                 Install proot manually and add it to $PATH to use it on other platforms."
            )),
        }
    }

    async fn version(&self) -> RuntimeResult<String> {
        info!("version: starting");
        let proot_bin = find_proot_binary(&self.runtime_dir).await?;
        info!(proot_bin = %proot_bin, "version: found proot binary");

        let mut cmd = setup_proot_command(&proot_bin);
        trace!(cmd = ?cmd, "version: executing proot --version");
        let output = cmd.arg("--version").output().await.map_err(|e| {
            error!(cmd = ?cmd, error = %e, "version: failed to execute proot --version");
            anyhow::anyhow!("Failed to execute proot to get version: {}", e)
        })?;
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        debug!(output = %combined, "version: proot output");
        let version = match Self::parse_proot_version(&combined) {
            Ok(version) => version,
            Err(e) => {
                // The distribution builds of PRoot do not print their version,
                // so the version of the installed package is used instead.
                let installed = proot_deb::installed_version(&self.runtime_dir)
                    .await
                    .ok_or(e)?;
                info!(version = %installed, "version: using the version of the installed package");
                installed
            }
        };
        info!(version = %version, "version: parsed version");
        Ok(version)
    }

    async fn available_versions(&self) -> RuntimeResult<Vec<String>> {
        info!("available_versions: starting");
        // Try to use the installed proot version if available
        match find_proot_binary(&self.runtime_dir).await {
            Ok(proot_bin) => {
                debug!(proot_bin = %proot_bin, "available_versions: found proot binary");
                let mut cmd = setup_proot_command(&proot_bin);
                trace!("available_versions: executing proot --version");
                let output = cmd.arg("--version").output().await;

                let output = match output {
                    Ok(o) => Some(o),
                    Err(_) if cfg!(target_os = "android") => {
                        debug!("available_versions: trying shell workaround on Android");
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
                    debug!(output = %combined, "available_versions: proot output");
                    if let Ok(v) = Self::parse_proot_version(&combined) {
                        info!(version = %v, "available_versions: returning installed version");
                        return Ok(vec![v]);
                    }
                }

                // The distribution builds of PRoot do not print their version,
                // so the version of the installed package is used instead.
                if let Some(v) = proot_deb::installed_version(&self.runtime_dir).await {
                    info!(version = %v, "available_versions: returning the version of the installed package");
                    return Ok(vec![v]);
                }
            }
            Err(_) => {
                debug!("available_versions: proot not found, fetching from repository");
            }
        }

        debug!(arch = %std::env::consts::ARCH, os = %std::env::consts::OS, "available_versions: fetching from repository");
        match (std::env::consts::ARCH, std::env::consts::OS) {
            ("x86_64", "linux") | ("aarch64", "linux") => self.fetch_linux_versions().await,
            ("aarch64", "android") | ("x86_64", "android") => {
                self.fetch_android_latest_version().await
            }
            _ => Err(anyhow::anyhow!("Unsupported platform for version fetching")),
        }
    }

    async fn download(&self, version: &str) -> RuntimeResult<()> {
        info!(version = %version, "download: starting");
        // A new binary has to be verified again before it is used in a nested
        // sandbox.
        reset_nested_proot_check();

        // On Android, if the embedded proot asset is present, skip download.
        #[cfg(target_os = "android")]
        if self.runtime_dir.join("libproot.so").exists() {
            info!(
                runtime_dir = %self.runtime_dir.display(),
                "download: embedded proot present — skipping download"
            );
            return Ok(());
        }

        // A proot binary that is provided by the system must not be replaced —
        // the binary in the runtime directory however can be updated.
        if which::which("proot").is_ok() && !self.runtime_dir.join("proot").exists() {
            debug!("download: a system proot is already available");
            return Ok(());
        }

        debug!(arch = %std::env::consts::ARCH, os = %std::env::consts::OS, "download: platform info");
        match (std::env::consts::ARCH, std::env::consts::OS) {
            ("x86_64", "linux") | ("aarch64", "linux") => self.download_linux(version).await,
            ("aarch64", "android") | ("x86_64", "android") => self.download_android(version).await,
            _ => Err(anyhow::anyhow!("Unsupported platform for download")),
        }
    }

    async fn prepare(&self, image: &str, tx: PrepareProgressTx) -> RuntimeResult<()> {
        // Helper for cleanup
        async fn cleanup_dir_if_exists(dir: &std::path::Path) {
            if dir.exists() {
                let _ = tokio::fs::remove_dir_all(dir).await;
            }
        }

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

        // Idempotency: If a previous run was cancelled, clean up any partial data
        if rootfs_dir.exists() {
            info!(rootfs = %rootfs_dir.display(), "prepare: cleaning up partial rootfs from previous run");
            cleanup_dir_if_exists(&rootfs_dir).await;
        }
        if image_dir.exists() && !rootfs_dir.exists() {
            // If image_dir exists but rootfs doesn't, it may be a partial/corrupt state
            // Remove the image_dir to ensure a clean state
            info!(image_dir = %image_dir.display(), "prepare: cleaning up partial image_dir from previous run");
            cleanup_dir_if_exists(&image_dir).await;
        }

        tokio::fs::create_dir_all(&image_dir)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create image directory: {}", e))?;

        // Pull and extract image using the shared sync function (curl-based).
        let image_owned = image.to_string();
        let rootfs_dir_for_blocking = rootfs_dir.clone();
        let pulled = tokio::task::spawn_blocking(move || {
            let (os, arch) = host_platform();
            let platform = format!("{os}/{arch}");
            shared::pull_and_extract_image(&image_owned, &platform, &rootfs_dir_for_blocking)
                .map_err(|e| anyhow::anyhow!("pull_and_extract_image failed: {e}"))
        })
        .await
        .map_err(|e| anyhow::anyhow!("Blocking task panicked: {e}"))??;

        info!(
            env_count = pulled.image_config.env.len(),
            has_entrypoint = pulled.image_config.entrypoint.is_some(),
            has_cmd = pulled.image_config.cmd.is_some(),
            working_dir = ?pulled.image_config.working_dir,
            "prepare: pulled image metadata from OCI registry"
        );

        // Persist complete image metadata
        let metadata = ImageMetadata {
            tag: image.to_string(),
            build_date: pulled.created,
            env: pulled.image_config.env,
            entrypoint: pulled.image_config.entrypoint,
            cmd: pulled.image_config.cmd,
            working_dir: pulled.image_config.working_dir.or(Some("/".to_string())),
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
        // Prevent removal of embedded image on Android
        if self.is_embedded_image(image_tag) {
            return Err(anyhow::anyhow!(
                "Cannot remove embedded image on Android.                  Embedded rootfs is part of the application and                  can only be updated by installing a new version of the app."
            ));
        }

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
        env_vars: &[(&str, &str)],
    ) -> RuntimeResult<ProcessHandle> {
        info!(image = %image, args_len = args.len(), mounts_len = mounts.len(), "run: starting container");

        let tag_dir_name = image.replace([':', '/'], "_");
        let image_dir = self.images_dir.join(&tag_dir_name);
        let rootfs_dir = image_dir.join("rootfs");

        debug!(runtime_dir = %self.runtime_dir.display(), "run: runtime directory");
        debug!(rootfs_dir = %rootfs_dir.display(), "run: checking if rootfs exists");
        if !rootfs_dir.exists() {
            return Err(anyhow::anyhow!(
                "Image not prepared: {}. Call prepare() first.",
                image
            ));
        }
        debug!("run: rootfs exists");

        // Write runtime network configuration into the rootfs so that
        // DNS resolution works inside the container.  This runs on
        // every invocation so that host network changes (VPN, roaming,
        // etc.) are picked up automatically.
        write_guest_network_config(&rootfs_dir).await;

        // Load metadata
        let metadata_path = image_dir.join("metadata.json");
        debug!(metadata_path = %metadata_path.display(), "run: loading metadata");
        let metadata: ImageMetadata = if metadata_path.exists() {
            let content = tokio::fs::read_to_string(&metadata_path)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to read metadata: {}", e))?;
            serde_json::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Failed to deserialize metadata: {}", e))?
        } else {
            warn!("run: metadata file not found, using defaults");
            ImageMetadata {
                tag: String::new(),
                build_date: None,
                env: Vec::new(),
                entrypoint: None,
                cmd: None,
                working_dir: Some("/".to_string()),
            }
        };
        trace!(metadata = ?metadata, "run: metadata loaded");

        // Locate the proot binary
        let proot_bin = find_proot_binary(&self.runtime_dir).await?;
        debug!(proot_bin = %proot_bin, "run: found proot binary");

        // PRoot cannot be nested reliably.  When this app itself runs under
        // PRoot (for example the Linux build started from DroidDeck on
        // Android), make sure the installed binary can actually start a guest
        // process, and fall back to an older compatible version otherwise.
        let proot_bin = self
            .ensure_nested_proot_works(&rootfs_dir, proot_bin, nested_sandbox_detected())
            .await?;
        debug!(proot_bin = %proot_bin, "run: proot binary ready");

        // Build the tokio async Command with proper environment setup
        let mut cmd = setup_proot_command(&proot_bin);

        // Clear all inherited environment variables from the parent process
        // Only the container's environment will be used
        cmd.env_clear();

        // Environment variables that PRoot itself needs on this platform.
        setup_proot_environment(&mut cmd);

        // PRoot extracts its loader into a temporary directory.  On Linux
        // (including the Linux build running inside DroidDeck) keep that
        // directory under the app's data dir instead of relying on `/tmp`.
        #[cfg(target_os = "linux")]
        {
            let tmp_dir = self.images_dir.join(".proot-tmp");
            let _ = std::fs::create_dir_all(&tmp_dir);
            if let Some(tmp_str) = tmp_dir.to_str() {
                cmd.env("PROOT_TMP_DIR", tmp_str);
            }
        }

        // On Android, bind the jniLibs directory so that ELF symlinks
        // (librootfs-<hash>.so) resolve inside the container.
        #[cfg(target_os = "android")]
        {
            if let Some(jnilib_dir) = crate::settings::get_android_native_lib_dir() {
                cmd.arg("-b").arg(jnilib_dir);
            }
        }

        // Set PROOT_TMP_DIR for Android (proot needs this)
        #[cfg(target_os = "android")]
        {
            let tmp_dir = self.images_dir.join(".proot-tmp");
            let _ = std::fs::create_dir_all(&tmp_dir);
            if let Some(tmp_str) = tmp_dir.to_str() {
                // Only set this minimal env for proot itself, not the container
                cmd.env("PROOT_TMP_DIR", tmp_str);
            }
        }

        // Set PROOT_LOADER for Android (required for proot to work)
        #[cfg(target_os = "android")]
        {
            if let Some(loader_path) = crate::settings::get_android_native_lib_dir()
                .map(|lib_dir| std::path::PathBuf::from(lib_dir).join("libloader.so"))
            {
                if let Some(loader_str) = loader_path.to_str() {
                    cmd.env("PROOT_LOADER", loader_str);
                }
            }
        }

        trace!(mounts_len = mounts.len(), "run: adding mount bindings");
        for mount in mounts {
            cmd.arg("-b").arg(format!(
                "{}:{}",
                mount.host_path.display(),
                mount.container_path
            ));
        }

        // Add custom mounts from settings
        let settings = crate::settings::get_settings()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to load settings: {}", e))?;
        trace!(
            custom_mounts_len = settings.custom_mounts.len(),
            "run: adding custom mounts"
        );

        for mount_spec in &settings.custom_mounts {
            // Parse mount_spec: "host_path:container_path" or "host_path"
            let (host_path, container_path) = if mount_spec.contains(':') {
                let parts: Vec<&str> = mount_spec.splitn(2, ':').collect();
                (parts[0].to_string(), parts[1].to_string())
            } else {
                (mount_spec.clone(), mount_spec.clone())
            };

            // Verify host path exists
            if std::path::Path::new(&host_path).exists() {
                debug!(host_path = %host_path, container_path = %container_path, "run: adding custom mount");
                cmd.arg("-b")
                    .arg(format!("{}:{}", host_path, container_path));
            } else {
                warn!(host_path = %host_path, "run: skipping custom mount, host path does not exist");
            }
        }

        // Add CUDA mounts if the image requires it (heuristic: image name contains "cuda")
        if image.contains("cuda") {
            debug!("run: CUDA support enabled, detecting and adding CUDA mounts");
            warn!("run: CUDA support on proot runtime will probably not work...");
            let cuda_paths = crate::settings::detect_cuda_paths();
            for (cuda_path_k, cuda_path_v) in cuda_paths.iter() {
                debug!(host = %cuda_path_k, container = %cuda_path_v, "run: adding CUDA mount");
                cmd.arg("-b")
                    .arg(format!("{}:{}", cuda_path_v, cuda_path_v));
            }
        }

        cmd.arg("-R").arg(&rootfs_dir);

        if let Some(workdir) = &metadata.working_dir {
            cmd.arg("-w").arg(workdir);
        }

        // Extract environment variables from entrypoint if it uses env -i pattern
        let mut actual_entrypoint: Vec<String> = Vec::new();
        if let Some(ep) = &metadata.entrypoint {
            actual_entrypoint.extend(ep.clone());
        }

        // Compose entrypoint + cmd + user args
        let mut full_cmd: Vec<String> = actual_entrypoint;
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

        trace!(full_cmd = ?full_cmd, "run: full command");

        // Set environment variables directly on the command
        // First, add the image's default environment variables
        for env_var in &metadata.env {
            if let Some((key, value)) = env_var.split_once('=') {
                cmd.env(key, value);
            }
        }

        // Then, add environment variables extracted from entrypoint if present
        if let Some(ep) = &metadata.entrypoint {
            if ep.len() >= 2 && ep[0] == "env" && ep[1] == "-i" {
                for arg in &ep[2..] {
                    if let Some((key, value)) = arg.split_once('=') {
                        cmd.env(key, value);
                    }
                }
            }
        }

        // Finally, add any extra environment variables passed by the caller
        // These will override image defaults and entrypoint vars if there are conflicts
        for (key, value) in env_vars {
            cmd.env(key, value);
        }

        // Add the actual command to execute
        for arg in &full_cmd {
            cmd.arg(arg);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        trace!(proot_bin = %proot_bin, cmd = ?cmd, "run: final command details");
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
                "Cannot delete the embedded proot binary — it is part of the application package and managed by Android."
            ));
        }

        #[cfg(not(target_os = "android"))]
        {
            let proot_bin = find_proot_binary(&self.runtime_dir).await?;
            let runtime_proot = self.runtime_dir.join("proot");

            // Use canonicalized paths for comparison to avoid issues with symlinks or relative paths
            let proot_bin_canon = tokio::fs::canonicalize(&proot_bin)
                .await
                .unwrap_or_else(|_| std::path::PathBuf::from(&proot_bin));
            let runtime_proot_canon = tokio::fs::canonicalize(&runtime_proot)
                .await
                .unwrap_or(runtime_proot.clone());

            if proot_bin_canon == runtime_proot_canon {
                tokio::fs::remove_file(&runtime_proot)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to delete proot binary: {}", e))?;
                // Remove the auxiliary files of a package based installation
                // (the launcher script, the binary and the bundled libtalloc).
                proot_deb::remove_bundled_files(&self.runtime_dir).await;
                reset_nested_proot_check();
                info!(
                    path = %runtime_proot.display(),
                    "delete_binary: deleted proot binary"
                );
                return Ok(());
            }

            Err(anyhow::anyhow!(
                "Cannot delete system proot binary at {}. \\n                 Only binaries downloaded into the runtime directory can be deleted.",
                proot_bin
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The nested checks share the process-wide verification flag, so the two
    /// sandbox tests must not run concurrently.
    #[cfg(target_os = "linux")]
    static NESTED_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_tracer_pid_from_proc_status() {
        assert_eq!(
            tracer_pid_from_status("Name:\tapp\nTracerPid:\t0\n"),
            Some(0)
        );
        assert_eq!(
            tracer_pid_from_status("Name:\tapp\nTracerPid:\t1234\n"),
            Some(1234)
        );
        assert_eq!(tracer_pid_from_status("Name:\tapp\n"), None);
    }

    /// The nested probe must use a command that actually exists in the image;
    /// otherwise a missing `/bin/true` looks like a broken PRoot and the
    /// fallback is skipped.
    #[cfg(target_os = "linux")]
    #[test]
    fn probe_uses_an_executable_present_in_the_image() {
        let base = std::env::temp_dir().join(format!(
            "colmap-openmvs-probe-command-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("bin")).unwrap();

        assert_eq!(probe_guest_command(&base), vec!["/bin/true"]);

        std::fs::write(base.join("bin/sh"), b"").unwrap();
        assert_eq!(probe_guest_command(&base), vec!["/bin/sh", "-c", "true"]);

        std::fs::remove_file(base.join("bin/sh")).unwrap();
        std::fs::create_dir_all(base.join("usr/bin")).unwrap();
        std::fs::write(base.join("usr/bin/env"), b"").unwrap();
        assert_eq!(probe_guest_command(&base), vec!["/usr/bin/env", "true"]);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// When the installed PRoot cannot be nested, a portable binary placed in
    /// the runtime directory is selected and the choice is remembered.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn uses_a_portable_proot_when_the_installed_one_cannot_run_nested() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = NESTED_TEST_LOCK.lock().await;
        reset_nested_proot_check();

        let runtime_dir = std::env::temp_dir().join(format!(
            "colmap-openmvs-portable-proot-test-{}",
            std::process::id()
        ));
        let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
        tokio::fs::create_dir_all(&runtime_dir).await.unwrap();

        let installed = runtime_dir.join("proot");
        tokio::fs::write(&installed, "#!/bin/sh\nexit 182\n")
            .await
            .unwrap();
        std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).unwrap();

        let portable = runtime_dir.join(format!(
            "proot-{PORTABLE_NESTED_PROOT_VERSION}-{}",
            std::env::consts::ARCH
        ));
        tokio::fs::write(&portable, "#!/bin/sh\nexit 0\n")
            .await
            .unwrap();
        std::fs::set_permissions(&portable, std::fs::Permissions::from_mode(0o755)).unwrap();

        let runtime = PRoot::new_simple(runtime_dir.clone());
        let installed = installed.to_string_lossy().into_owned();
        let selected = runtime
            .ensure_nested_proot_works(std::path::Path::new("/"), installed.clone(), true)
            .await
            .expect("the portable PRoot should be selected");
        assert_eq!(selected, portable.to_string_lossy());

        // The cached selection must not fall back to the broken binary.
        let cached = runtime
            .ensure_nested_proot_works(std::path::Path::new("/"), installed, true)
            .await
            .expect("the cached selection should be reused");
        assert_eq!(cached, selected);

        let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
    }

    /// A nested-incompatible PRoot build fails its loader probe with status
    /// 182; the runtime has to replace it with a version that can run.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires network access"]
    async fn replaces_a_proot_that_cannot_run_nested() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = NESTED_TEST_LOCK.lock().await;
        reset_nested_proot_check();

        let runtime_dir = std::env::temp_dir().join(format!(
            "colmap-openmvs-nested-proot-test-{}",
            std::process::id()
        ));
        let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
        tokio::fs::create_dir_all(&runtime_dir).await.unwrap();

        // A binary that behaves like a PRoot loader that cannot be nested.
        let fake = runtime_dir.join("proot");
        tokio::fs::write(&fake, "#!/bin/sh\nexit 182\n")
            .await
            .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let runtime = PRoot::new_simple(runtime_dir.clone());
        let selected = runtime
            .ensure_nested_proot_works(
                std::path::Path::new("/"),
                fake.to_string_lossy().into_owned(),
                true,
            )
            .await
            .expect("a nested-compatible PRoot should be installed");
        assert_eq!(
            runtime
                .probe_rootfs(std::path::Path::new("/"), &selected)
                .await,
            RootfsProbe::Ok
        );

        let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
    }

    /// The newest PRoot package (5.4.x) is the one installed by default.  It
    /// works inside DroidDeck's Termux PRoot, but not inside older Debian
    /// PRoot builds, where the runtime has to replace it with an older
    /// compatible build.  Both outcomes have to leave a working binary.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires network access and an outer PRoot sandbox"]
    async fn replaces_the_newest_proot_when_it_cannot_run_nested() {
        let _guard = NESTED_TEST_LOCK.lock().await;
        reset_nested_proot_check();

        let runtime_dir = std::env::temp_dir().join(format!(
            "colmap-openmvs-nested-newest-test-{}",
            std::process::id()
        ));
        let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
        let runtime = PRoot::new_simple(runtime_dir.clone());

        // Same choice the settings screen offers first.
        let versions = runtime.available_versions().await.expect("versions");
        assert!(!versions.is_empty(), "no versions found");
        runtime.download(&versions[0]).await.expect("download");

        let selected = runtime
            .ensure_nested_proot_works(
                std::path::Path::new("/"),
                runtime_dir.join("proot").to_string_lossy().into_owned(),
                true,
            )
            .await
            .expect("a nested-compatible PRoot should be installed");
        assert_eq!(
            runtime
                .probe_rootfs(std::path::Path::new("/"), &selected)
                .await,
            RootfsProbe::Ok
        );
        assert!(
            runtime.version().await.is_ok(),
            "the runtime must report the version of the binary it selected"
        );

        let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
    }

    /// Install PRoot through the regular runtime API and verify that the
    /// installed binary can be started.
    ///
    /// The test uses the packages of the architecture of the host, so it
    /// verifies the installation on every supported platform.
    #[tokio::test]
    #[ignore = "requires network access"]
    async fn installs_and_runs_proot() {
        let runtime_dir = std::env::temp_dir().join("colmap-openmvs-proot-runtime-test");
        let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
        let runtime = PRoot::new_simple(runtime_dir.clone());

        runtime.is_supported().expect("platform supported");

        let versions = runtime.available_versions().await.expect("versions");
        assert!(!versions.is_empty(), "no versions found");
        println!("available versions: {versions:?}");

        runtime.download(&versions[0]).await.expect("download");
        assert!(
            runtime_dir.join("proot").exists(),
            "the proot binary was not installed"
        );

        let version = runtime.version().await.expect("version");
        assert!(!version.is_empty());

        // Installing again must replace the previous installation.
        runtime.download(&versions[0]).await.expect("update");
        assert!(runtime_dir.join("proot").exists());

        runtime.delete_binary().await.expect("delete");
        assert!(!runtime_dir.join("proot").exists());

        let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
    }

    /// Install PRoot and verify that it can run a command inside a guest
    /// rootfs.
    ///
    /// A PRoot build that is unable to translate the paths of the guest
    /// rootfs starts up without any error but fails every command.
    ///
    /// PRoot needs a working ptrace, so this test can only run on a native
    /// host.  It fails inside an emulated environment, for example when an
    /// aarch64 binary is executed by qemu-user on an x86_64 host.
    #[tokio::test]
    #[ignore = "requires network access and a native ptrace"]
    async fn runs_a_command_inside_a_guest_rootfs() {
        let runtime_dir = std::env::temp_dir().join("colmap-openmvs-proot-guest-test");
        let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
        let runtime = PRoot::new_simple(runtime_dir.clone());

        let versions = runtime.available_versions().await.expect("versions");
        assert!(!versions.is_empty(), "no versions found");

        runtime.download(&versions[0]).await.expect("download");
        assert!(
            run_guest_probe(&runtime_dir.join("proot")).await,
            "the installed proot cannot run a command inside a guest rootfs"
        );

        let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
    }

    /// Run a command inside a guest rootfs and check that it is executed.
    ///
    /// The host directories that provide the guest shell and its libraries
    /// are bound into the guest, while the script that has to be read lives
    /// inside the guest rootfs.  PRoot builds that cannot translate the
    /// paths of the guest rootfs fail to find it.
    async fn run_guest_probe(proot_bin: &std::path::Path) -> bool {
        const MARKER: &str = "COLMAP_OPENMVS_PROOT_PROBE";

        if !std::path::Path::new("/bin/sh").exists() {
            return true;
        }

        let probe_dir =
            std::env::temp_dir().join(format!("colmap-openmvs-proot-probe-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&probe_dir).await;
        if tokio::fs::create_dir_all(&probe_dir).await.is_err() {
            return false;
        }
        if tokio::fs::write(probe_dir.join("probe.sh"), format!("echo {MARKER}\n"))
            .await
            .is_err()
        {
            return false;
        }

        let mut cmd = Command::new(proot_bin);
        cmd.env_clear();
        setup_proot_environment(&mut cmd);
        cmd.arg("-r").arg(&probe_dir);
        for dir in [
            "/bin",
            "/sbin",
            "/lib",
            "/lib32",
            "/lib64",
            "/libx32",
            "/usr/lib",
            "/usr/lib32",
            "/usr/lib64",
            "/usr/libx32",
        ] {
            if std::path::Path::new(dir).exists() {
                cmd.arg("-b").arg(dir);
            }
        }
        cmd.arg("/bin/sh").arg("/probe.sh");

        let output = match cmd.output().await {
            Ok(output) => output,
            Err(_) => return false,
        };
        let _ = tokio::fs::remove_dir_all(&probe_dir).await;

        String::from_utf8_lossy(&output.stdout).contains(MARKER)
    }
}

// ---------------------------------------------------------------------------
// Guest network configuration (written into rootfs on every run)
// ---------------------------------------------------------------------------

/// Write `/etc/resolv.conf` and `/etc/hosts` into the prepared rootfs
/// so that DNS resolution works inside the container, especially on
/// Android where the host's `/etc/` is a read-only mount.
///
/// On Android, DNS servers are read via `getprop net.dns{1..4}`.
/// On other platforms the host's `/etc/resolv.conf` is parsed and
/// loopback nameservers are filtered out.  Falls back to Google
/// public DNS when no usable servers are found.
///
/// Called on every [`PRoot::run`] invocation so that changes to the
/// host network (VPN, WiFi↔cellular switch, etc.) are picked up.
async fn write_guest_network_config(rootfs_dir: &std::path::Path) {
    let etc_path = rootfs_dir.join("etc");

    // If /etc itself is a symlink (e.g. pointing to the host's read-only
    // /etc/ on Android), remove it first so we can create a real directory.
    #[cfg(unix)]
    {
        if let Ok(meta) = tokio::fs::symlink_metadata(&etc_path).await {
            if meta.is_symlink() {
                let _ = tokio::fs::remove_file(&etc_path).await;
            }
        }
    }

    if let Err(e) = tokio::fs::create_dir_all(&etc_path).await {
        warn!(path = %etc_path.display(), error = %e, "write_network_config: cannot create etc dir");
        return;
    }

    // ── resolv.conf ────────────────────────────────────────────────────
    let nameservers = resolve_nameservers().await;
    let mut resolv = String::from("# Generated by colmap-openmvs-app\n");
    for ns in &nameservers {
        resolv.push_str(&format!("nameserver {}\n", ns));
    }
    resolv.push_str("options edns0 trust-ad\n");
    let resolv_path = etc_path.join("resolv.conf");
    // Remove any existing symlink first – the rootfs skeleton may have
    // linked this file into read-only jniLibs storage.
    let _ = tokio::fs::remove_file(&resolv_path).await;
    if let Err(e) = tokio::fs::write(&resolv_path, &resolv).await {
        warn!(error = %e, "write_network_config: failed to write resolv.conf");
    }

    // ── hosts ──────────────────────────────────────────────────────────
    let hosts = "\
127.0.0.1\tlocalhost
::1\tlocalhost ip6-localhost ip6-loopback
";
    let hosts_path = etc_path.join("hosts");
    let _ = tokio::fs::remove_file(&hosts_path).await;
    if let Err(e) = tokio::fs::write(&hosts_path, hosts).await {
        warn!(error = %e, "write_network_config: failed to write hosts");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::Permissions::from_mode(0o644);
        let _ = tokio::fs::set_permissions(&resolv_path, perm.clone()).await;
        let _ = tokio::fs::set_permissions(&hosts_path, perm).await;
    }
}

/// Collect nameserver addresses from the running system.
///
/// On Android the `getprop` command is queried for `net.dns1`–`net.dns4`.
/// On other platforms `/etc/resolv.conf` is parsed; loopback addresses
/// (127.x.x.x, ::1) are discarded because they are unreachable from
/// inside a container.
async fn resolve_nameservers() -> Vec<String> {
    #[cfg(target_os = "android")]
    {
        let mut servers = Vec::new();
        for i in 1..=4 {
            let prop = format!("net.dns{}", i);
            let tried_paths = ["/system/bin/getprop", "getprop"];
            for bin in tried_paths {
                if let Ok(output) = tokio::process::Command::new(bin).arg(&prop).output().await {
                    let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !val.is_empty() && val != "0.0.0.0" {
                        servers.push(val);
                        break;
                    }
                }
            }
        }
        if !servers.is_empty() {
            return servers;
        }
        info!("resolve_nameservers: no DNS properties found via getprop, using fallback");
        vec!["8.8.8.8".into(), "8.8.4.4".into()]
    }

    #[cfg(not(target_os = "android"))]
    {
        let mut servers = Vec::new();
        if let Ok(content) = tokio::fs::read_to_string("/etc/resolv.conf").await {
            for line in content.lines() {
                let line = line.trim();
                if let Some(ns) = line.strip_prefix("nameserver ") {
                    let ns = ns.trim();
                    if !ns.starts_with("127.") && !ns.starts_with("::1") {
                        servers.push(ns.to_string());
                    }
                }
            }
        }
        if !servers.is_empty() {
            return servers;
        }
        info!("resolve_nameservers: no valid nameservers in /etc/resolv.conf, using fallback");
        vec!["8.8.8.8".into(), "8.8.4.4".into()]
    }
}

// ---------------------------------------------------------------------------
// Blocking helpers (run inside tokio::task::spawn_blocking)
// ---------------------------------------------------------------------------

/// Extract `data.tar.xz` from a Debian `.ar` archive.
/// Extract `data.tar.xz` from a Debian `.ar` archive using the shared
/// in-memory extraction, then write the result to `output_path`.
fn extract_from_ar_sync(ar_path: &Path, output_path: &Path) -> RuntimeResult<()> {
    debug!(ar_path = %ar_path.display(), output_path = %output_path.display(), "extract_from_ar_sync: starting extraction");

    let file_data =
        std::fs::read(ar_path).map_err(|e| anyhow::anyhow!("Failed to read ar file: {}", e))?;

    let data = extract_data_tar_from_ar(&file_data)
        .map_err(|e| anyhow::anyhow!("Failed to extract from ar archive: {}", e))?;

    std::fs::write(output_path, &data)
        .map_err(|e| anyhow::anyhow!("Failed to write extracted data: {}", e))?;

    debug!(output_path = %output_path.display(), bytes_written = data.len(), "extract_from_ar_sync: extracted successfully");
    Ok(())
}

/// Extract proot/libtalloc binaries from a `data.tar.xz`.
fn extract_tar_xz_sync(
    images_dir: &Path,
    tar_xz_path: &Path,
    package_name: &str,
) -> RuntimeResult<()> {
    debug!(images_dir = %images_dir.display(), tar_xz_path = %tar_xz_path.display(), package_name = %package_name, "extract_tar_xz_sync: starting extraction");

    let tar_xz_data =
        std::fs::read(tar_xz_path).map_err(|e| anyhow::anyhow!("Failed to read tar.xz: {}", e))?;

    debug!(
        tar_xz_size = tar_xz_data.len(),
        "extract_tar_xz_sync: decompressing xz data"
    );
    let tar_data = decompress_xz(&tar_xz_data)
        .map_err(|e| anyhow::anyhow!("Failed to decompress xz: {}", e))?;
    debug!(
        tar_size = tar_data.len(),
        "extract_tar_xz_sync: tar data decompressed"
    );

    let mut archive = tar::Archive::new(&tar_data[..]);

    trace!("extract_tar_xz_sync: iterating through tar entries");
    for entry in archive
        .entries()
        .map_err(|e| anyhow::anyhow!("Failed to read tar: {}", e))?
    {
        let mut entry = entry.map_err(|e| anyhow::anyhow!("Failed to read tar entry: {}", e))?;
        let path = entry
            .path()
            .map_err(|e| anyhow::anyhow!("Failed to get entry path: {}", e))?;
        let path_str = path.to_string_lossy().into_owned();

        trace!(entry_path = %path_str, package_name = %package_name, "extract_tar_xz_sync: processing tar entry");
        if package_name == "proot" && path_str.ends_with("usr/bin/proot") {
            let dest = images_dir.join("proot");
            debug!(dest = %dest.display(), "extract_tar_xz_sync: extracting proot binary");
            entry
                .unpack(&dest)
                .map_err(|e| anyhow::anyhow!("Failed to unpack proot: {}", e))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(0o755);
                std::fs::set_permissions(&dest, perms).ok();
                debug!(dest = %dest.display(), "extract_tar_xz_sync: set executable permissions on proot");
            }
        } else if package_name == "libtalloc" && path_str.contains("usr/lib/libtalloc") {
            let dest = images_dir.join(path.file_name().unwrap_or_default());
            debug!(dest = %dest.display(), "extract_tar_xz_sync: extracting libtalloc library");
            entry
                .unpack(&dest)
                .map_err(|e| anyhow::anyhow!("Failed to unpack library: {}", e))?;
        } else if package_name == "libandroid-shmem"
            && path_str.contains("usr/lib/libandroid-shmem.so")
        {
            let dest = images_dir.join("libandroid-shmem.so");
            debug!(dest = %dest.display(), "extract_tar_xz_sync: extracting libandroid-shmem library");
            entry
                .unpack(&dest)
                .map_err(|e| anyhow::anyhow!("Failed to unpack library: {}", e))?;
        }
    }

    debug!(package_name = %package_name, "extract_tar_xz_sync: extraction completed");
    Ok(())
}

/// Metadata persisted alongside each prepared image on disk.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ImageMetadata {
    #[serde(default)]
    pub(crate) tag: String,
    #[serde(default)]
    pub(crate) build_date: Option<String>,
    #[serde(default)]
    pub(crate) env: Vec<String>,
    #[serde(default)]
    pub(crate) entrypoint: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) cmd: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) working_dir: Option<String>,
}

// ---------------------------------------------------------------------------
// Host platform detection
// ---------------------------------------------------------------------------

/// Return (os, architecture) matching OCI platform conventions.
pub(crate) fn host_platform() -> (String, String) {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "windows",
        "android" => "android",
        _ => "linux",
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "arm",
        "i686" => "386",
        _ => "amd64",
    };
    (os.to_string(), arch.to_string())
}
