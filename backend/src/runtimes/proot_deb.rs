//! Installation of PRoot from Debian/Ubuntu binary packages (Linux).
//!
//! Upstream PRoot only publishes a pre-compiled binary for `x86_64`
//! (<https://proot.gitlab.io/proot/bin/proot>) and the `proot-static-build`
//! releases are `x86_64` only as well.
//!
//! Distributions however ship `proot` for every architecture they support.
//! This module downloads the very same `.deb` packages that are used to
//! install the Termux build on Android, extracts the `proot` binary from them
//! and — when the host does not provide `libtalloc` — bundles the required
//! shared library next to it so that the binary can be started without any
//! system modification.
//!
//! The distribution builds of PRoot rely on a seccomp accelerator that is
//! broken on the kernels affected by Ubuntu bug #1202161, so the runtime
//! starts PRoot with `PROOT_NO_SECCOMP=1` (see `setup_proot_environment`).

use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;
use tracing::{debug, info, warn};

/// Distribution pools that publish the `proot` package.
const PROOT_POOLS: &[&str] = &[
    "http://deb.debian.org/debian/pool/main/p/proot/",
    "http://ports.ubuntu.com/ubuntu-ports/pool/universe/p/proot/",
];

/// Distribution pools that publish the `libtalloc2` package.
const TALLOC_POOLS: &[&str] = &[
    "http://deb.debian.org/debian/pool/main/t/talloc/",
    "http://ports.ubuntu.com/ubuntu-ports/pool/main/t/talloc/",
];

/// Name of the shared library PRoot depends on.
const TALLOC_SONAME: &str = "libtalloc.so.2";

/// File name of the launcher script that is generated when PRoot has to be
/// started with a bundled `libtalloc`.
const LAUNCHER_NAME: &str = "proot";

/// File name of the PRoot executable inside the runtime directory.
const BINARY_NAME: &str = "proot.bin";

/// File that stores the version of the installed distribution package.
///
/// The distribution builds of PRoot do not embed the version number, so it is
/// recorded next to the binary while it is installed.
const VERSION_FILE: &str = "proot.version";

/// Cache for the package metadata — the pools are queried at most once per
/// package name and application run.
static PACKAGE_CACHE: Lazy<Mutex<HashMap<String, Vec<PackageInfo>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Suffix of the distribution packages of the given architecture.
///
/// Only the architectures that the distributions publish a `proot` package
/// for are supported.
pub(crate) fn package_suffix(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("_amd64.deb"),
        "aarch64" => Some("_arm64.deb"),
        _ => None,
    }
}

/// Suffix of the distribution packages of the current host.
fn host_suffix() -> Result<&'static str> {
    package_suffix(std::env::consts::ARCH).ok_or_else(|| {
        anyhow!(
            "No PRoot distribution packages are available for {}",
            std::env::consts::ARCH
        )
    })
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A `major.minor.patch` version that can be compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version {
    pub(crate) major: u64,
    pub(crate) minor: u64,
    pub(crate) patch: u64,
}

impl Version {
    /// Parse a version string such as `2.38` or `2.36-9+deb12u13`.
    ///
    /// Only the leading numeric components are taken into account so that
    /// distribution suffixes (`-9+deb12u13`) do not influence the result.
    pub(crate) fn parse(text: &str) -> Option<Version> {
        let text = text.trim().trim_start_matches('v');

        let mut numbers: Vec<u64> = Vec::new();
        for part in text.split('.') {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                break;
            }
            numbers.push(digits.parse().ok()?);
        }

        let major = *numbers.first()?;
        let minor = numbers.get(1).copied().unwrap_or(0);
        let patch = numbers.get(2).copied().unwrap_or(0);
        Some(Version {
            major,
            minor,
            patch,
        })
    }
}

/// A `.deb` package published by a distribution pool.
#[derive(Debug, Clone)]
pub(crate) struct Package {
    /// Package version as used by the distribution (e.g. `5.4.0-3`).
    pub(crate) version: String,
    /// Absolute URL of the package.
    pub(crate) url: String,
}

/// A package together with the information required to decide whether it can
/// be installed on the current host.
#[derive(Debug, Clone)]
pub(crate) struct PackageInfo {
    pub(crate) package: Package,
    /// Minimum glibc version required by the package (if declared).
    pub(crate) min_glibc: Option<Version>,
    /// `true` when the package is known to be usable on this host.
    pub(crate) compatible: bool,
}

/// The `control` file of a Debian package.
#[derive(Debug, Default)]
pub(crate) struct DebControl {
    pub(crate) version: String,
    pub(crate) depends: String,
}

impl DebControl {
    /// Minimum glibc version required by the package.
    ///
    /// Debian declares this as `libc6 (>= 2.38)` inside the `Depends` field.
    pub(crate) fn min_glibc(&self) -> Option<Version> {
        for dependency in self.depends.split(',') {
            let dependency = dependency.trim();
            let Some(rest) = dependency.strip_prefix("libc6") else {
                continue;
            };
            let Some((_, constraint)) = rest.split_once('(') else {
                continue;
            };
            let constraint = constraint.trim().trim_end_matches(')').trim();
            let Some(required) = constraint.strip_prefix(">=") else {
                continue;
            };
            if let Some(version) = Version::parse(required.trim()) {
                return Some(version);
            }
        }
        None
    }
}

/// Result of executing a freshly extracted PRoot binary.
enum ProbeOutcome {
    /// The binary started successfully.
    Ok,
    /// The binary is missing `libtalloc` and has to be bundled.
    MissingTalloc,
    /// The binary could not be started.
    Failed(String),
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Fetch the `proot` packages that are published for the current host.
pub(crate) async fn fetch_proot_packages() -> Result<Vec<PackageInfo>> {
    fetch_packages("proot", PROOT_POOLS).await
}

/// Fetch the `libtalloc2` packages that are published for the current host.
pub(crate) async fn fetch_libtalloc_packages() -> Result<Vec<PackageInfo>> {
    fetch_packages("libtalloc2", TALLOC_POOLS).await
}

/// Install the PRoot binary for Linux from a distribution package.
///
/// `requested_version` selects the package that should be installed.  When
/// that package cannot be used on this host (for example because it requires
/// a newer glibc) the newest compatible package is installed instead.
pub(crate) async fn install_proot(runtime_dir: &Path, requested_version: &str) -> Result<String> {
    let packages = fetch_proot_packages().await?;

    // Try the requested version first, then fall back to the remaining
    // packages (newest first) so that an incompatible build does not block
    // the installation completely.
    let mut candidates: Vec<&PackageInfo> = packages.iter().filter(|p| p.compatible).collect();
    candidates.sort_by_key(|info| !version_matches(&info.package.version, requested_version));

    for info in packages.iter().filter(|info| !info.compatible) {
        debug!(
            version = %info.package.version,
            required_glibc = ?info.min_glibc,
            "install_proot: skipping package, the host glibc is too old"
        );
    }

    if candidates.is_empty() {
        return Err(anyhow!(
            "No PRoot package compatible with this system was found"
        ));
    }

    let mut last_error: Option<anyhow::Error> = None;
    for info in candidates {
        match install_package(runtime_dir, info).await {
            Ok(version) => {
                if !version_matches(&info.package.version, requested_version) {
                    info!(
                        requested = %requested_version,
                        installed = %version,
                        "install_proot: requested version is not usable on this host, installed the newest compatible one"
                    );
                }
                return Ok(version);
            }
            Err(e) => {
                warn!(
                    version = %info.package.version,
                    url = %info.package.url,
                    error = %e,
                    "install_proot: package could not be installed, trying an older one"
                );
                last_error = Some(e);
            }
        }
    }

    Err(last_error
        .unwrap_or_else(|| anyhow!("Failed to install PRoot from the distribution packages")))
}

// ---------------------------------------------------------------------------
// Package discovery
// ---------------------------------------------------------------------------

/// Fetch the metadata of all packages of `name` that are published for the
/// current host.
async fn fetch_packages(name: &str, pools: &[&str]) -> Result<Vec<PackageInfo>> {
    if let Some(cached) = cached_packages(name) {
        return Ok(cached);
    }

    let packages = download_package_list(name, pools, host_suffix()?).await?;
    let host_glibc = host_glibc_version().await;

    let mut infos = Vec::with_capacity(packages.len());
    for package in packages {
        let min_glibc = match download_control(&package.url).await {
            Ok(control) => control.min_glibc(),
            Err(e) => {
                warn!(url = %package.url, error = %e, "fetch_packages: cannot read package metadata");
                None
            }
        };

        let compatible = match (min_glibc, host_glibc) {
            (Some(required), Some(host)) => required <= host,
            // Without a known glibc version the package is only rejected later
            // when the extracted binary fails to start.
            _ => true,
        };

        if !compatible {
            debug!(
                version = %package.version,
                url = %package.url,
                required_glibc = ?min_glibc,
                "fetch_packages: package requires a newer glibc than the host provides"
            );
        }

        infos.push(PackageInfo {
            package,
            min_glibc,
            compatible,
        });
    }

    store_packages(name, infos.clone());
    Ok(infos)
}

/// Return the cached package metadata of `name` (if any).
fn cached_packages(name: &str) -> Option<Vec<PackageInfo>> {
    let cache = PACKAGE_CACHE.lock().ok()?;
    cache.get(name).cloned()
}

/// Store the package metadata of `name` in the cache.
fn store_packages(name: &str, packages: Vec<PackageInfo>) {
    if let Ok(mut cache) = PACKAGE_CACHE.lock() {
        cache.insert(name.to_string(), packages);
    }
}

/// List all packages of `name` that are published for the given architecture
/// in `pools`.
async fn download_package_list(name: &str, pools: &[&str], suffix: &str) -> Result<Vec<Package>> {
    let agent = build_agent();
    let mut packages: Vec<Package> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for pool in pools {
        let url = pool.to_string();
        match agent.get(&url).call() {
            Ok(response) => match response.into_body().read_to_string() {
                Ok(html) => packages.extend(parse_pool_listing(&html, name, pool, suffix)),
                Err(e) => errors.push(format!("{url}: {e}")),
            },
            Err(e) => errors.push(format!("{url}: {e}")),
        }
    }

    if packages.is_empty() {
        return Err(anyhow!(
            "No {name} packages for {suffix} were found in the distribution pools ({})",
            if errors.is_empty() {
                "no pool responded".to_string()
            } else {
                errors.join("; ")
            }
        ));
    }

    // Newest version first, deduplicated by version.
    packages.sort_by(|a, b| compare_package_versions(&b.version, &a.version));
    packages.dedup_by(|a, b| a.version == b.version);
    Ok(packages)
}

/// Extract the package links of a distribution pool listing.
fn parse_pool_listing(html: &str, name: &str, pool: &str, suffix: &str) -> Vec<Package> {
    let mut packages = Vec::new();
    let mut rest = html;

    while let Some(start) = rest.find("href=\"") {
        rest = &rest[start + "href=\"".len()..];
        let Some(end) = rest.find('"') else {
            break;
        };
        let file_name = &rest[..end];
        rest = &rest[end..];

        let Some(version) = file_name
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('_'))
            .and_then(|rest| rest.strip_suffix(suffix))
        else {
            continue;
        };
        if version.is_empty() {
            continue;
        }

        packages.push(Package {
            version: version.to_string(),
            url: format!("{pool}{file_name}"),
        });
    }

    packages
}

/// Compare two distribution package versions (e.g. `5.4.0-3`).
fn compare_package_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let a_segments = split_version(a);
    let b_segments = split_version(b);

    for index in 0..a_segments.len().max(b_segments.len()) {
        let ordering = match (a_segments.get(index), b_segments.get(index)) {
            (None, None) => break,
            // A missing segment sorts before every existing one.
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some((true, a)), Some((true, b))) => match (a.parse::<u64>(), b.parse::<u64>()) {
                (Ok(a), Ok(b)) => a.cmp(&b),
                _ => a.cmp(b),
            },
            // Numeric segments sort after alphabetic ones (Debian convention).
            (Some((true, _)), Some((false, _))) => std::cmp::Ordering::Greater,
            (Some((false, _)), Some((true, _))) => std::cmp::Ordering::Less,
            (Some((false, a)), Some((false, b))) => a.cmp(b),
        };
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }

    std::cmp::Ordering::Equal
}

/// Split a version string into its numeric and alphabetic segments.
fn split_version(version: &str) -> Vec<(bool, String)> {
    let mut segments: Vec<(bool, String)> = Vec::new();
    let mut current = String::new();
    let mut current_is_numeric = false;
    let mut has_current = false;

    for character in version.chars() {
        if character.is_ascii_alphanumeric() {
            let is_numeric = character.is_ascii_digit();
            if has_current && is_numeric != current_is_numeric {
                segments.push((current_is_numeric, std::mem::take(&mut current)));
                has_current = false;
            }
            if !has_current {
                current_is_numeric = is_numeric;
                has_current = true;
            }
            current.push(character);
        } else if has_current {
            segments.push((current_is_numeric, std::mem::take(&mut current)));
            has_current = false;
        }
    }

    if has_current {
        segments.push((current_is_numeric, current));
    }

    segments
}

/// Check whether `candidate` satisfies the `requested` version.
///
/// Both the exact distribution version (`5.4.0-3`) and the plain upstream
/// version (`5.4.0`) are accepted.
fn version_matches(candidate: &str, requested: &str) -> bool {
    if candidate == requested {
        return true;
    }
    let candidate_base = candidate.split(['-', '+']).next().unwrap_or(candidate);
    let requested_base = requested
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or(requested);
    candidate_base == requested_base
}

// ---------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------

/// Install a single distribution package into `runtime_dir`.
async fn install_package(runtime_dir: &Path, info: &PackageInfo) -> Result<String> {
    let deb = download(&info.package.url).await?;

    let files = extract_from_data_tar(&deb, |path| {
        let path = path.trim_start_matches("./");
        path.ends_with("usr/bin/proot")
    })?;
    let (_, proot_bytes) = files
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("the package does not contain a proot binary"))?;

    tokio::fs::create_dir_all(runtime_dir)
        .await
        .map_err(|e| anyhow!("Failed to create runtime directory: {e}"))?;

    // Stage the binary next to its final destination so that a failing
    // package never replaces a working installation.
    let staged = runtime_dir.join("proot.download");
    tokio::fs::write(&staged, &proot_bytes)
        .await
        .map_err(|e| anyhow!("Failed to write proot binary: {e}"))?;
    set_executable(&staged).await?;

    match probe(&staged).await {
        ProbeOutcome::Ok => {
            install_binary(runtime_dir, &staged).await?;
            write_version(runtime_dir, &info.package.version).await?;
            debug!(version = %info.package.version, "install_package: proot installed");
            Ok(info.package.version.clone())
        }
        ProbeOutcome::MissingTalloc => {
            info!(
                version = %info.package.version,
                "install_package: host does not provide libtalloc, bundling it"
            );
            let talloc_packages = fetch_libtalloc_packages().await?;
            bundle_talloc(runtime_dir, &talloc_packages).await?;

            let binary = runtime_dir.join(BINARY_NAME);
            tokio::fs::rename(&staged, &binary)
                .await
                .map_err(|e| anyhow!("Failed to install proot binary: {e}"))?;
            write_launcher(runtime_dir).await?;

            match probe(&runtime_dir.join(LAUNCHER_NAME)).await {
                ProbeOutcome::Ok => {
                    write_version(runtime_dir, &info.package.version).await?;
                    debug!(version = %info.package.version, "install_package: proot installed with bundled libtalloc");
                    Ok(info.package.version.clone())
                }
                ProbeOutcome::MissingTalloc => {
                    Err(anyhow!("the bundled libtalloc could not be loaded"))
                }
                ProbeOutcome::Failed(message) => Err(anyhow!(
                    "the extracted proot binary could not be started: {message}"
                )),
            }
        }
        ProbeOutcome::Failed(message) => Err(anyhow!(
            "the extracted proot binary could not be started: {message}"
        )),
    }
}

/// Record the version of the installed package next to the binary.
///
/// The distribution builds of PRoot do not print their version, so it is
/// stored while the package is installed.
async fn write_version(runtime_dir: &Path, version: &str) -> Result<()> {
    tokio::fs::write(runtime_dir.join(VERSION_FILE), version)
        .await
        .map_err(|e| anyhow!("Failed to write the proot version: {e}"))
}

/// Read the version of the installed distribution package.
pub(crate) async fn installed_version(runtime_dir: &Path) -> Option<String> {
    let content = tokio::fs::read_to_string(runtime_dir.join(VERSION_FILE))
        .await
        .ok()?;
    let version = content.trim();
    if version.is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

/// Move the staged binary to its final location and remove leftovers of a
/// previous installation.
async fn install_binary(runtime_dir: &Path, staged: &Path) -> Result<()> {
    let _ = tokio::fs::remove_file(runtime_dir.join(LAUNCHER_NAME)).await;
    let _ = tokio::fs::remove_file(runtime_dir.join(BINARY_NAME)).await;
    let _ = tokio::fs::remove_file(runtime_dir.join(TALLOC_SONAME)).await;

    tokio::fs::rename(staged, runtime_dir.join(LAUNCHER_NAME))
        .await
        .map_err(|e| anyhow!("Failed to install proot binary: {e}"))?;
    Ok(())
}

/// Download `libtalloc` and store it next to the PRoot binary.
///
/// The packages are tried from the newest to the oldest one so that a
/// fallback is used when the newest package cannot be extracted.
async fn bundle_talloc(runtime_dir: &Path, packages: &[PackageInfo]) -> Result<()> {
    let mut last_error: Option<anyhow::Error> = None;

    for info in packages.iter().filter(|info| info.compatible) {
        match download_talloc(&info.package, runtime_dir).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                warn!(
                    version = %info.package.version,
                    error = %e,
                    "bundle_talloc: libtalloc package could not be used"
                );
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("No usable libtalloc package was found")))
}

/// Download a `libtalloc2` package and extract the shared library.
async fn download_talloc(package: &Package, runtime_dir: &Path) -> Result<()> {
    let deb = download(&package.url).await?;

    let files = extract_from_data_tar(&deb, |path| {
        let path = path.trim_start_matches("./");
        let Some(file_name) = path.rsplit('/').next() else {
            return false;
        };
        // The regular file is named `libtalloc.so.2.4.0`, the `libtalloc.so.2`
        // entry is a symlink and therefore not part of the result.
        file_name.starts_with(&format!("{TALLOC_SONAME}."))
    })?;

    let (_, library) = files
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("the package does not contain {TALLOC_SONAME}"))?;

    tokio::fs::write(runtime_dir.join(TALLOC_SONAME), &library)
        .await
        .map_err(|e| anyhow!("Failed to write {TALLOC_SONAME}: {e}"))?;
    Ok(())
}

/// Write the launcher script that starts PRoot with the bundled `libtalloc`.
async fn write_launcher(runtime_dir: &Path) -> Result<()> {
    let directory = shell_quote(&runtime_dir.display().to_string());
    let script = format!(
        "#!/bin/sh\n\
         # Generated by colmap-openmvs-app — starts the PRoot binary that was\n\
         # installed from a distribution package and points the dynamic linker\n\
         # at the bundled libtalloc.\n\
         PROOT_DIR={directory}\n\
         LD_LIBRARY_PATH=\"$PROOT_DIR${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}}\"\n\
         export LD_LIBRARY_PATH\n\
         exec \"$PROOT_DIR/{BINARY_NAME}\" \"$@\"\n"
    );

    let launcher = runtime_dir.join(LAUNCHER_NAME);
    tokio::fs::write(&launcher, script)
        .await
        .map_err(|e| anyhow!("Failed to write proot launcher: {e}"))?;
    set_executable(&launcher).await?;
    Ok(())
}

/// Quote a value so that it can be used inside a double quoted shell string.
fn shell_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '$' => quoted.push_str("\\$"),
            '`' => quoted.push_str("\\`"),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

/// Remove the auxiliary files of a previous installation.
pub(crate) async fn remove_bundled_files(runtime_dir: &Path) {
    let _ = tokio::fs::remove_file(runtime_dir.join(BINARY_NAME)).await;
    let _ = tokio::fs::remove_file(runtime_dir.join(TALLOC_SONAME)).await;
    let _ = tokio::fs::remove_file(runtime_dir.join("proot.download")).await;
    let _ = tokio::fs::remove_file(runtime_dir.join(VERSION_FILE)).await;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Try to execute `binary` and classify the result.
async fn probe(binary: &Path) -> ProbeOutcome {
    let output = match tokio::process::Command::new(binary)
        .arg("--version")
        .output()
        .await
    {
        Ok(output) => output,
        Err(e) => return ProbeOutcome::Failed(e.to_string()),
    };

    if output.status.success() {
        return ProbeOutcome::Ok;
    }

    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .trim()
    .to_string();

    if message.contains(TALLOC_SONAME)
        && (message.contains("cannot open shared object file")
            || message.contains("error while loading shared libraries"))
    {
        return ProbeOutcome::MissingTalloc;
    }

    ProbeOutcome::Failed(message)
}

/// Detect the glibc version of the host system.
async fn host_glibc_version() -> Option<Version> {
    let output = tokio::process::Command::new("ldd")
        .arg("--version")
        .output()
        .await
        .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_glibc_version(&text)
}

/// Extract the glibc version from the output of `ldd --version`.
fn parse_glibc_version(text: &str) -> Option<Version> {
    let start = text.find("GLIBC ")? + "GLIBC ".len();
    let rest = &text[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    Version::parse(&rest[..end])
}

/// Build an HTTP agent for the package downloads.
fn build_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent("colmap-openmvs-app")
        .build()
        .into()
}

/// Download a file into memory.
async fn download(url: &str) -> Result<Vec<u8>> {
    let agent = build_agent();
    let mut response = agent
        .get(url)
        .call()
        .map_err(|e| anyhow!("Failed to download {url}: {e}"))?;

    let bytes = response
        .body_mut()
        .with_config()
        .limit(256 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| anyhow!("Failed to read {url}: {e}"))?;
    Ok(bytes)
}

/// Set the executable bit of `path`.
async fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o755);
        tokio::fs::set_permissions(path, permissions)
            .await
            .map_err(|e| anyhow!("Failed to set executable bit on {}: {e}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// `.deb` archive handling
// ---------------------------------------------------------------------------

/// Split a `.deb` (ar archive) into its members.
fn ar_members(deb: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    if deb.len() < 8 || &deb[..8] != b"!<arch>\n" {
        return Err(anyhow!("Invalid deb package: bad magic signature"));
    }

    let mut members = Vec::new();
    let mut offset = 8usize;
    while offset + 60 <= deb.len() {
        let name = String::from_utf8_lossy(&deb[offset..offset + 16])
            .trim_end()
            .trim_end_matches('/')
            .to_string();
        let size: usize = String::from_utf8_lossy(&deb[offset + 48..offset + 58])
            .trim_end()
            .parse()
            .map_err(|_| anyhow!("Invalid deb package: bad member size"))?;

        let start = offset + 60;
        let end = start + size;
        if end > deb.len() {
            break;
        }
        members.push((name, deb[start..end].to_vec()));
        offset = end + (size & 1);
    }

    Ok(members)
}

/// Decompress an archive member based on its file name extension.
fn decompress(name: &str, data: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    if name.ends_with(".xz") {
        xz2::read::XzDecoder::new(data).read_to_end(&mut output)?;
    } else if name.ends_with(".gz") {
        flate2::read::GzDecoder::new(data).read_to_end(&mut output)?;
    } else if name.ends_with(".zst") {
        zstd::stream::read::Decoder::new(data)?.read_to_end(&mut output)?;
    } else {
        output.extend_from_slice(data);
    }
    Ok(output)
}

/// Extract all regular files of the `data.tar` member that match `matcher`.
fn extract_from_data_tar(
    deb: &[u8],
    matcher: impl Fn(&str) -> bool,
) -> Result<Vec<(String, Vec<u8>)>> {
    let members = ar_members(deb)?;
    let (name, data) = members
        .iter()
        .find(|(name, _)| name.starts_with("data.tar."))
        .ok_or_else(|| anyhow!("data.tar member not found in deb package"))?;

    let tar_bytes = decompress(name, data)?;
    let mut archive = tar::Archive::new(&tar_bytes[..]);
    let mut files = Vec::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.to_string_lossy().into_owned();
        if !matcher(&path) {
            continue;
        }
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        files.push((path, content));
    }

    Ok(files)
}

/// Read the `control` file of a `.deb` package.
fn parse_control(deb: &[u8]) -> Result<DebControl> {
    let members = ar_members(deb)?;
    let (name, data) = members
        .iter()
        .find(|(name, _)| name.starts_with("control.tar."))
        .ok_or_else(|| anyhow!("control.tar member not found in deb package"))?;

    let tar_bytes = decompress(name, data)?;
    let mut archive = tar::Archive::new(&tar_bytes[..]);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        if path.trim_start_matches("./") != "control" {
            continue;
        }
        let mut text = String::new();
        entry.read_to_string(&mut text)?;
        return Ok(parse_control_text(&text));
    }

    Err(anyhow!("control file not found in deb package"))
}

/// Parse the fields of a Debian `control` file.
fn parse_control_text(text: &str) -> DebControl {
    let mut control = DebControl::default();
    let mut in_depends = false;

    for line in text.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            // Continuation of the previous field.
            if in_depends {
                control.depends.push(' ');
                control.depends.push_str(line.trim());
            }
            continue;
        }

        in_depends = false;
        if let Some(value) = line.strip_prefix("Version:") {
            control.version = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("Depends:") {
            control.depends = value.trim().to_string();
            in_depends = true;
        }
    }

    control
}

/// Download a package and read its `control` file.
async fn download_control(url: &str) -> Result<DebControl> {
    let deb = download(url).await?;
    parse_control(&deb)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pool_listing() {
        let html = r#"<a href="proot_5.4.0-3_arm64.deb">proot_5.4.0-3_arm64.deb</a>
            <a href="proot_5.4.0-3_amd64.deb">proot_5.4.0-3_amd64.deb</a>
            <a href="proot_5.1.0-1.3+b1_arm64.deb">proot_5.1.0-1.3+b1_arm64.deb</a>
            <a href="proot_5.1.0.orig.tar.gz">proot_5.1.0.orig.tar.gz</a>"#;

        let packages = parse_pool_listing(html, "proot", "http://example.org/", "_arm64.deb");
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].version, "5.4.0-3");
        assert_eq!(
            packages[0].url,
            "http://example.org/proot_5.4.0-3_arm64.deb"
        );
        assert_eq!(packages[1].version, "5.1.0-1.3+b1");
    }

    #[test]
    fn sorts_package_versions() {
        let mut versions = vec![
            "5.1.0-1.3".to_string(),
            "5.4.0-3".to_string(),
            "3.0.2-1".to_string(),
            "5.1.0-1.3+b1".to_string(),
        ];
        versions.sort_by(|a, b| compare_package_versions(b, a));
        assert_eq!(
            versions,
            vec![
                "5.4.0-3".to_string(),
                "5.1.0-1.3+b1".to_string(),
                "5.1.0-1.3".to_string(),
                "3.0.2-1".to_string(),
            ]
        );
    }

    #[test]
    fn matches_requested_versions() {
        assert!(version_matches("5.4.0-3", "5.4.0-3"));
        assert!(version_matches("5.4.0-3", "5.4.0"));
        assert!(version_matches("5.4.0-3", "v5.4.0"));
        assert!(!version_matches("5.4.0-3", "5.1.0"));
    }

    #[test]
    fn parses_glibc_requirement() {
        let control = parse_control_text(
            "Package: proot\nVersion: 5.4.0-3\nDepends: libc6 (>= 2.38), libtalloc2 (>= 2.0.4~git20101213)\n",
        );
        assert_eq!(control.version, "5.4.0-3");
        assert_eq!(
            control.min_glibc(),
            Some(Version {
                major: 2,
                minor: 38,
                patch: 0
            })
        );
    }

    #[test]
    fn parses_folded_glibc_requirement() {
        let control = parse_control_text(
            "Package: proot\nDepends: libc6 (>= 2.38),\n libtalloc2 (>= 2.0.4)\n",
        );
        assert_eq!(
            control.min_glibc(),
            Some(Version {
                major: 2,
                minor: 38,
                patch: 0
            })
        );
    }

    #[test]
    fn parses_host_glibc_version() {
        let version = parse_glibc_version("ldd (Debian GLIBC 2.36-9+deb12u13) 2.36").unwrap();
        assert_eq!(
            version,
            Version {
                major: 2,
                minor: 36,
                patch: 0
            }
        );
        assert!(parse_glibc_version("musl libc (aarch64)\nVersion 1.2.4").is_none());
    }

    #[test]
    fn maps_architectures_to_package_suffixes() {
        assert_eq!(package_suffix("x86_64"), Some("_amd64.deb"));
        assert_eq!(package_suffix("aarch64"), Some("_arm64.deb"));
        assert_eq!(package_suffix("riscv64"), None);
    }

    #[test]
    fn quotes_shell_values() {
        assert_eq!(shell_quote("/tmp/runtime"), "\"/tmp/runtime\"");
        assert_eq!(shell_quote("/tmp/with space"), "\"/tmp/with space\"");
        assert_eq!(shell_quote("/tmp/$HOME"), "\"/tmp/\\$HOME\"");
        assert_eq!(shell_quote("/tmp/\"quoted\""), "\"/tmp/\\\"quoted\\\"\"");
    }

    /// The launcher must start the bundled binary and point the dynamic
    /// linker at the runtime directory.
    #[tokio::test]
    async fn launcher_starts_the_bundled_binary() {
        let runtime_dir = std::env::temp_dir().join("colmap-openmvs-proot-launcher-test");
        let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
        tokio::fs::create_dir_all(&runtime_dir)
            .await
            .expect("runtime dir");

        // A stand-in for the extracted PRoot binary.
        let binary = runtime_dir.join(BINARY_NAME);
        tokio::fs::write(&binary, "#!/bin/sh\necho \"proot $* [$LD_LIBRARY_PATH]\"\n")
            .await
            .expect("write binary");
        set_executable(&binary).await.expect("executable bit");

        write_launcher(&runtime_dir).await.expect("launcher");

        let output = tokio::process::Command::new(runtime_dir.join(LAUNCHER_NAME))
            .arg("--version")
            .output()
            .await
            .expect("run launcher");
        assert!(output.status.success());
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            text.contains("proot --version"),
            "unexpected output: {text}"
        );
        assert!(text.contains(&runtime_dir.display().to_string()));

        let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
    }

    // ── Network tests (disabled by default) ─────────────────────────────

    /// Download a package into memory.
    fn fetch_package(url: &str) -> Vec<u8> {
        let agent = build_agent();
        let mut response = agent.get(url).call().expect("download package");
        response
            .body_mut()
            .with_config()
            .limit(256 * 1024 * 1024)
            .read_to_vec()
            .expect("read package")
    }

    #[test]
    #[ignore = "requires network access"]
    fn extracts_proot_from_xz_package() {
        let deb =
            fetch_package("http://deb.debian.org/debian/pool/main/p/proot/proot_5.4.0-3_amd64.deb");

        let control = parse_control(&deb).expect("control file");
        assert_eq!(control.version, "5.4.0-3");
        assert_eq!(
            control.min_glibc(),
            Some(Version {
                major: 2,
                minor: 38,
                patch: 0
            })
        );

        let files =
            extract_from_data_tar(&deb, |path| path.ends_with("usr/bin/proot")).expect("data tar");
        assert_eq!(files.len(), 1);
        assert!(files[0].1.starts_with(b"\x7fELF"));
    }

    #[test]
    #[ignore = "requires network access"]
    fn extracts_proot_from_zstd_package() {
        let deb = fetch_package(
            "http://ports.ubuntu.com/ubuntu-ports/pool/universe/p/proot/proot_5.4.0-3_arm64.deb",
        );

        let control = parse_control(&deb).expect("control file");
        assert_eq!(control.version, "5.4.0-3");

        let files =
            extract_from_data_tar(&deb, |path| path.ends_with("usr/bin/proot")).expect("data tar");
        assert_eq!(files.len(), 1);
        assert!(files[0].1.starts_with(b"\x7fELF"));
    }

    #[test]
    #[ignore = "requires network access"]
    fn extracts_libtalloc_from_package() {
        let deb = fetch_package(
            "http://deb.debian.org/debian/pool/main/t/talloc/libtalloc2_2.4.0-f2_amd64.deb",
        );

        let files = extract_from_data_tar(&deb, |path| {
            path.trim_start_matches("./")
                .rsplit('/')
                .next()
                .map(|name| name.starts_with("libtalloc.so.2."))
                .unwrap_or(false)
        })
        .expect("data tar");
        assert_eq!(files.len(), 1);
        assert!(files[0].1.starts_with(b"\x7fELF"));
    }

    #[tokio::test]
    #[ignore = "requires network access"]
    async fn lists_packages_from_pools() {
        let suffix = host_suffix().expect("host architecture");
        let packages = download_package_list("proot", PROOT_POOLS, suffix)
            .await
            .expect("package list");
        assert!(!packages.is_empty());
        assert!(packages.iter().all(|p| p.url.ends_with(suffix)));
        assert!(packages.iter().any(|p| p.version.starts_with("5.")));
    }

    /// Install PRoot from the distribution packages and verify that the
    /// resulting executable can be started.
    ///
    /// The test uses the packages of the architecture of the host, so it
    /// verifies the installation on every supported platform.
    #[tokio::test]
    #[ignore = "requires network access"]
    async fn installs_proot_from_distribution_package() {
        let packages = fetch_proot_packages().await.expect("proot packages");
        assert!(!packages.is_empty(), "no proot packages found");

        let runtime_dir = std::env::temp_dir().join("colmap-openmvs-proot-install-test");
        let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
        tokio::fs::create_dir_all(&runtime_dir)
            .await
            .expect("runtime dir");

        // Install the newest compatible package.
        let info = packages
            .iter()
            .find(|info| info.compatible)
            .expect("compatible package");
        println!(
            "installing {} from {} ({} compatible packages)",
            info.package.version,
            info.package.url,
            packages.iter().filter(|info| info.compatible).count()
        );
        let version = install_package(&runtime_dir, info)
            .await
            .expect("installation failed");
        assert_eq!(version, info.package.version);

        // The launcher must exist and the binary must be executable.
        let launcher = runtime_dir.join(LAUNCHER_NAME);
        assert!(launcher.exists(), "launcher was not created");

        let output = tokio::process::Command::new(&launcher)
            .arg("--version")
            .output()
            .await
            .expect("run proot");
        assert!(
            output.status.success(),
            "proot failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("proot"));

        // A second installation must replace the previous one.
        let version = install_package(&runtime_dir, info)
            .await
            .expect("second installation failed");
        assert_eq!(version, info.package.version);

        remove_bundled_files(&runtime_dir).await;
        let _ = tokio::fs::remove_dir_all(&runtime_dir).await;
    }
}
