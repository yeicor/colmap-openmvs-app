use clap::Parser;
use colmap_openmvs_api::Settings;
use dioxus::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

// ─── CLI argument parsing ────────────────────────────────────────────────────

/// COLMAP + OpenMVS backend server.
///
/// All settings can be configured via:
///   • a JSON config file (see --config / COLMAP_CONFIG)
///   • environment variables
///   • command-line flags (highest precedence)
#[derive(Parser, Debug, Clone)]
#[command(name = "colmap-openmvs-server", version, about, long_about)]
pub struct CliConfig {
    /// Path to an alternative settings.json config file.
    ///
    /// When set, the server loads settings from this file instead of the
    /// default location (projects_folder/settings.json). Values in this
    /// file can still be overridden by individual CLI flags or env vars.
    #[arg(short = 'c', long = "config", env = "COLMAP_CONFIG")]
    pub config: Option<String>,

    /// Root directory for all project data.
    #[arg(long = "projects-folder", env = "COLMAP_PROJECTS_FOLDER")]
    pub projects_folder: Option<String>,

    /// Directory containing the PRoot binary and supporting libraries.
    #[arg(long = "proot-binary-dir", env = "COLMAP_PROOT_BINARY_DIR")]
    pub proot_binary_dir: Option<String>,

    /// Directory for large PRoot runtime images.
    #[arg(long = "proot-images-dir", env = "COLMAP_PROOT_IMAGES_DIR")]
    pub proot_images_dir: Option<String>,

    /// Default container image tag (format: "proot:tag" or "docker:tag").
    #[arg(long = "default-image-tag", env = "COLMAP_DEFAULT_IMAGE_TAG")]
    pub default_image_tag: Option<String>,

    /// Additional filesystem mount for the PRoot/Docker runtime
    /// (format: "/host/path:/container/path").
    /// Can be specified multiple times.
    #[arg(long = "custom-mount", env = "COLMAP_CUSTOM_MOUNT")]
    pub custom_mounts: Vec<String>,

    /// Override the settings.json path. Leave unset to use
    /// projects_folder/settings.json.
    #[arg(long = "settings-path", env = "COLMAP_SETTINGS_PATH")]
    pub settings_file_path: Option<String>,

    /// Server bind address (IP). Also read by Dioxus as `IP`.
    #[arg(long = "ip", env = "IP", default_value = "0.0.0.0")]
    pub ip: String,

    /// Server port. Also read by Dioxus as `PORT`.
    #[arg(short = 'p', long = "port", env = "PORT", default_value = "8080")]
    pub port: u16,
}

// ─── Global settings singleton ───────────────────────────────────────────────

static SETTINGS: OnceLock<RwLock<Settings>> = OnceLock::new();

/// Initialize the global settings **before** any async code reads them.
///
/// This must be called once at program startup (server-side only). If it is
/// never called the first call to [`get_settings`] will fall back to loading
/// the default settings file from disk (legacy behaviour).
pub fn initialize(settings: Settings) {
    SETTINGS
        .set(RwLock::new(settings))
        .unwrap_or_else(|_| panic!("settings already initialized"));
}

/// Parse CLI args (with env-var fallback), merge with config-file, and
/// initialize the global settings singleton.  Sets `IP` / `PORT` env vars
/// so Dioxus picks them up when the server starts.
pub fn initialize_from_env() -> CliConfig {
    // Parse CLI arguments (clap also reads env vars marked with `env = ...`).
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    let cli = CliConfig::parse();
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let cli = CliConfig::parse_from([""]);

    // Ensure Dioxus sees the correct IP / PORT.
    std::env::set_var("IP", &cli.ip);
    std::env::set_var("PORT", cli.port.to_string());

    // 1. Start with platform-specific defaults.
    let mut settings = default_settings();

    // 2. Load from the config file if explicitly provided (--config / COLMAP_CONFIG).
    if let Some(ref config_path) = cli.config {
        debug!(path = %config_path, "Loading settings from --config file");
        match std::fs::read_to_string(config_path) {
            Ok(contents) => match serde_json::from_str::<Settings>(&contents) {
                Ok(file_settings) => {
                    info!(path = %config_path, "Config file loaded successfully");
                    apply_file_override(&mut settings, file_settings);
                }
                Err(e) => {
                    error!(path = %config_path, error = %e, "Failed to parse config file, ignoring");
                }
            },
            Err(e) => {
                error!(path = %config_path, error = %e, "Failed to read config file, ignoring");
            }
        }
    } else {
        // No explicit config → try the default settings.json path.
        let default_path = settings_file_path(&settings.projects_folder);
        debug!(path = %default_path.display(), "Loading settings from default path");
        if let Ok(contents) = std::fs::read_to_string(&default_path) {
            match serde_json::from_str::<Settings>(&contents) {
                Ok(file_settings) => {
                    info!(path = %default_path.display(), "Default settings file loaded");
                    apply_file_override(&mut settings, file_settings);
                }
                Err(e) => {
                    error!(
                        path = %default_path.display(),
                        error = %e,
                        "Failed to parse default settings file, ignoring"
                    );
                }
            }
        } else {
            debug!("No default settings file found, using platform defaults");
        }
    }

    // 3. Apply CLI / env-var overrides (clap resolved env vars).
    apply_cli_overrides(&mut settings, &cli);

    // 4. Persist the merged result in the global singleton.
    initialize(settings);

    cli
}

/// Merge the fields from `file` into `base` (file is lower-priority than CLI).
fn apply_file_override(base: &mut Settings, file: Settings) {
    base.projects_folder = file.projects_folder;
    base.proot_binary_dir = file.proot_binary_dir;
    base.proot_images_dir = file.proot_images_dir;
    if file.default_image_tag.is_some() {
        base.default_image_tag = file.default_image_tag;
    }
    if !file.custom_mounts.is_empty() {
        base.custom_mounts = file.custom_mounts;
    }
    if file.settings_file_path.is_some() {
        base.settings_file_path = file.settings_file_path;
    }
}

/// Overlay CLI/env values on top of the current settings (highest priority).
fn apply_cli_overrides(settings: &mut Settings, cli: &CliConfig) {
    if let Some(ref v) = cli.projects_folder {
        settings.projects_folder = v.clone();
    }
    if let Some(ref v) = cli.proot_binary_dir {
        settings.proot_binary_dir = v.clone();
    }
    if let Some(ref v) = cli.proot_images_dir {
        settings.proot_images_dir = v.clone();
    }
    if let Some(ref v) = cli.default_image_tag {
        settings.default_image_tag = Some(v.clone());
    }
    if !cli.custom_mounts.is_empty() {
        settings.custom_mounts.clone_from(&cli.custom_mounts);
    }
    if let Some(ref v) = cli.settings_file_path {
        settings.settings_file_path = Some(v.clone());
    }
}

// ─── Platform-specific defaults ──────────────────────────────────────────────

pub(crate) fn default_projects_folder() -> String {
    if cfg!(target_os = "android") {
        "/data/data/com.github.yeicor.colmap_openmvs_app/files/projects".to_string()
    } else if cfg!(debug_assertions) {
        "./devstorage/projects".to_string()
    } else if cfg!(target_os = "ios") {
        "~/Documents/projects".to_string()
    } else if cfg!(target_os = "windows") {
        match std::env::var("APPDATA") {
            Ok(appdata) => format!("{}/colmap_openmvs/projects", appdata),
            Err(_) => "./projects".to_string(),
        }
    } else if cfg!(target_os = "macos") {
        match std::env::var("HOME") {
            Ok(home) => format!(
                "{}/Library/Application Support/colmap_openmvs/projects",
                home
            ),
            Err(_) => "./projects".to_string(),
        }
    } else {
        match std::env::var("HOME") {
            Ok(home) => format!("{}/.local/share/colmap_openmvs/projects", home),
            Err(_) => "./projects".to_string(),
        }
    }
}

// ─── Android-specific helpers ────────────────────────────────────────────────

/// Returns the app's private files directory on Android.
#[cfg(target_os = "android")]
pub fn get_android_files_dir() -> String {
    "/data/data/com.github.yeicor.colmap_openmvs_app/files".to_string()
}

/// Extract the native lib directory from `/proc/self/maps` on Android.
///
/// With `extractNativeLibs=true` the OS extracts `.so` files to a path such as:
///   `/data/app/<PACKAGE>-<HASH>/lib/arm64/libXXX.so`
///
/// Rules applied in order of preference:
/// 1. Path contains the app package name  → best match, returned immediately.
/// 2. Path looks like a native lib dir but no package name → kept as fallback.
/// 3. Paths that contain `!` (APK-internal, `extractNativeLibs=false`) are skipped.
#[cfg(target_os = "android")]
pub fn get_android_native_lib_dir() -> Option<String> {
    use std::fs::File;
    use std::io::{BufRead, BufReader};

    const PACKAGE_NAME: &str = "com.github.yeicor.colmap_openmvs_app";

    match File::open("/proc/self/maps") {
        Ok(file) => {
            let reader = BufReader::new(file);
            let mut fallback: Option<String> = None;

            for line in reader.lines() {
                let Ok(line) = line else { continue };

                if !line.contains(".so") {
                    continue;
                }

                // pathname is the last whitespace-separated field
                let Some(path) = line.split_whitespace().last() else {
                    continue;
                };

                // Skip APK-internal paths produced when extractNativeLibs=false
                if path.contains('!') {
                    continue;
                }

                // Must look like a native-library directory
                let is_lib_path = path.contains("/lib/arm64")
                    || path.contains("/lib/x86_64")
                    || path.contains("/lib/x86")
                    || path.contains("/lib64");

                if !is_lib_path {
                    continue;
                }

                let parent = std::path::Path::new(path)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned());

                if let Some(dir) = parent {
                    if path.contains(PACKAGE_NAME) {
                        // Best match: belongs to our package
                        return Some(dir);
                    } else if fallback.is_none() {
                        fallback = Some(dir);
                    }
                }
            }

            if fallback.is_none() {
                warn!("Could not find native lib dir in /proc/self/maps");
            }
            fallback
        }
        Err(e) => {
            warn!(error = %e, "Failed to read /proc/self/maps");
            None
        }
    }
}

/// Read the embedded image tag that was baked into the binary by `backend/build.rs`.
///
/// The tag is set via `cargo:rustc-env=EMBEDDED_IMAGE_TAG=...` during the Android
/// build, so `option_env!()` resolves it at compile time. On non-Android targets
/// the env var is not present and this function returns `None`.
pub fn read_embedded_image_tag() -> Option<String> {
    option_env!("EMBEDDED_IMAGE_TAG").map(|s| s.to_string())
}

/// Public, cross-platform wrapper around `read_embedded_image_tag`.
/// Returns `None` on non-Android targets (where `EMBEDDED_IMAGE_TAG` is not set).
pub fn read_embedded_image_tag_public() -> Option<String> {
    read_embedded_image_tag()
}

// ─── CUDA detection ──────────────────────────────────────────────────────────

/// Detect CUDA libraries on the system
/// Returns a list of CUDA mount paths that can be added to containers
pub fn detect_cuda_paths() -> HashMap<String, String> {
    use glob::glob;
    let mut cuda_mounts = Vec::new();

    // CUDA runtime directories (add if directory exists)
    let cuda_dirs = vec!["/usr/local/cuda/lib64", "/usr/local/cuda/lib"];
    for dir in cuda_dirs {
        if std::path::Path::new(dir).is_dir() {
            cuda_mounts.push(dir.to_string());
        }
    }

    // CUDA libraries and compat libraries (glob patterns)
    let cuda_lib_globs = vec![
        // Standard Ubuntu/Debian libraries
        "/usr/lib/**/libcuda.so*",
        "/usr/lib/**/libcudart.so*",
        "/usr/lib/**/libcublas.so*",
        "/usr/lib/**/libcufft.so*",
        "/usr/lib/**/libcudnn.so*",
        "/usr/lib/**/libnvrtc.so*",
        "/usr/lib/**/libnvidia-ml.so*",
        // Compat libraries
        "/usr/lib/**/libcuda-compat.so*",
        "/usr/lib/**/libcuda-compat.so.*",
        // Misc
        "/dev/nvidia*",
        "/proc/driver/nvidia*",
        "/usr/bin/nvidia*",
    ];
    for pattern in cuda_lib_globs {
        if let Ok(paths) = glob(pattern) {
            for entry in paths.flatten() {
                if entry.exists() {
                    cuda_mounts.push(entry.display().to_string());
                }
            }
        }
    }
    cuda_mounts.sort();
    cuda_mounts.dedup();
    // Map host CUDA files to /usr/lib/x86_64-linux-gnu/*.so in the container
    let mut cuda_map = HashMap::new();
    for host_path in cuda_mounts {
        let mut map_as_is = true;
        if host_path.starts_with("/usr/lib")
            && (host_path.ends_with(".so") || host_path.contains(".so."))
        {
            // Only map .so files
            if let Some(filename) = std::path::Path::new(&host_path).file_name() {
                let container_path =
                    format!("/usr/lib/x86_64-linux-gnu/{}", filename.to_string_lossy());
                cuda_map.insert(host_path.clone(), container_path);
                map_as_is = false;
            }
        }
        if map_as_is {
            cuda_map.insert(host_path.clone(), host_path); // Directories are mounted as-is
        }
    }
    cuda_map
}

// ─── Platform defaults ────────────────────────────────────────────────────────

/// Directory for the PRoot binary itself (non-modifiable on Android, embedded in JNI libs).
pub fn default_proot_binary_dir() -> String {
    #[cfg(target_os = "android")]
    {
        // On Android, libproot.so lives in the extracted native-lib directory.
        get_android_native_lib_dir().unwrap_or_else(|| get_android_files_dir())
    }
    #[cfg(not(target_os = "android"))]
    {
        // Debug builds use relative path for easier local development; release builds use platform-specific app data dirs.
        if cfg!(debug_assertions) {
            "./devstorage".to_string()
        } else if cfg!(target_os = "ios") {
            "~/Library/Application Support/colmap_openmvs".to_string()
        } else if cfg!(target_os = "windows") {
            match std::env::var("APPDATA") {
                Ok(appdata) => format!("{}/colmap_openmvs", appdata),
                Err(_) => "./bin/proot".to_string(),
            }
        } else if cfg!(target_os = "macos") {
            match std::env::var("HOME") {
                Ok(home) => format!("{}/Library/Application Support/colmap_openmvs", home),
                Err(_) => "./bin/proot".to_string(),
            }
        } else {
            match std::env::var("HOME") {
                Ok(home) => format!("{}/.local/share/colmap_openmvs", home),
                Err(_) => "./bin/proot".to_string(),
            }
        }
    }
}

/// Directory for large PRoot runtime images (user configurable, but on Android defaults to app files).
pub fn default_proot_images_dir() -> String {
    if cfg!(target_os = "android") {
        "/data/data/com.github.yeicor.colmap_openmvs_app/files/proot-images".to_string()
    } else if cfg!(debug_assertions) {
        "./devstorage/proot-images".to_string()
    } else if cfg!(target_os = "ios") {
        "~/Documents/proot-images".to_string()
    } else if cfg!(target_os = "windows") {
        match std::env::var("APPDATA") {
            Ok(appdata) => format!("{}/colmap_openmvs/proot-images", appdata),
            Err(_) => "./proot-images".to_string(),
        }
    } else if cfg!(target_os = "macos") {
        match std::env::var("HOME") {
            Ok(home) => format!(
                "{}/Library/Application Support/colmap_openmvs/proot-images",
                home
            ),
            Err(_) => "./proot-images".to_string(),
        }
    } else {
        match std::env::var("HOME") {
            Ok(home) => format!("{}/.local/share/colmap_openmvs/proot-images", home),
            Err(_) => "./proot-images".to_string(),
        }
    }
}

fn default_settings() -> Settings {
    // On Android, try to auto-detect the embedded image tag from jniLibs metadata.
    #[cfg(target_os = "android")]
    let default_image_tag = read_embedded_image_tag().map(|tag| format!("proot:{}", tag));
    #[cfg(not(target_os = "android"))]
    let default_image_tag = None;

    Settings {
        projects_folder: default_projects_folder(),
        proot_binary_dir: default_proot_binary_dir(),
        proot_images_dir: default_proot_images_dir(),
        default_image_tag,
        custom_mounts: Vec::new(),
        settings_file_path: None,
    }
}

// ─── Persistence ─────────────────────────────────────────────────────────────

/// Resolve the effective settings file path, checking environment variable first.
pub fn get_effective_settings_path(settings: &Settings) -> PathBuf {
    // Check for environment variable override
    if let Ok(env_path) = std::env::var("COLMAP_SETTINGS_PATH") {
        debug!(env_path = %env_path, "Using COLMAP_SETTINGS_PATH environment variable");
        return PathBuf::from(env_path);
    }

    // Use configured path if set
    if let Some(ref path) = settings.settings_file_path {
        debug!(configured_path = %path, "Using configured settings_file_path");
        return PathBuf::from(path);
    }

    // Default: projects_folder/settings.json
    let default_path = PathBuf::from(&settings.projects_folder).join("settings.json");
    debug!(default_path = %default_path.display(), "Using default settings path");
    default_path
}

pub fn settings_file_path(projects_folder: &str) -> PathBuf {
    // Check for environment variable override
    if let Ok(env_path) = std::env::var("COLMAP_SETTINGS_PATH") {
        return PathBuf::from(env_path);
    }

    // Default: projects_folder/settings.json
    PathBuf::from(projects_folder).join("settings.json")
}

fn load_settings_from_disk() -> Settings {
    let default_projects = default_projects_folder();
    let path = settings_file_path(&default_projects);
    debug!(path = %path.display(), "Loading settings from disk");

    match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<Settings>(&contents) {
            Ok(settings) => {
                info!(path = %path.display(), "Settings loaded successfully from disk");
                settings
            }
            Err(e) => {
                error!(path = %path.display(), error = %e, "Failed to parse settings file, using defaults");
                default_settings()
            }
        },
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(path = %path.display(), error = %e, "Failed to read settings file, using defaults");
            } else {
                debug!(path = %path.display(), "Settings file not found, using defaults");
            }
            default_settings()
        }
    }
}

pub async fn get_settings() -> Result<Settings> {
    debug!("Retrieving current settings");
    let lock = SETTINGS.get_or_init(|| RwLock::new(load_settings_from_disk()));
    Ok(lock.read().await.clone())
}

pub async fn update_settings(new_settings: Settings) -> Result<()> {
    debug!(projects_folder = %new_settings.projects_folder, "Updating settings");

    // Compute everything we need from new_settings BEFORE taking the write lock,
    // so the lock is held only for the in-memory swap and released before any I/O.
    let projects_folder = new_settings.projects_folder.clone();
    let path = get_effective_settings_path(&new_settings);
    let json_result = serde_json::to_string_pretty(&new_settings);

    // Hold the write lock ONLY to swap the in-memory value — released immediately.
    {
        let lock = SETTINGS.get_or_init(|| RwLock::new(load_settings_from_disk()));
        let mut settings = lock.write().await;
        *settings = new_settings;
    }

    // All I/O happens after the write lock is released, so readers are never
    // blocked by slow disk operations.
    debug!(folder = %projects_folder, "Creating projects folder");
    if let Err(e) = tokio::fs::create_dir_all(&projects_folder).await {
        error!(folder = %projects_folder, error = %e, "Failed to create projects folder");
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                error!(dir = %parent.display(), error = %e, "Failed to create settings directory");
            }
        }
    }

    debug!(path = %path.display(), "Serializing settings for persistence");
    match json_result {
        Ok(json) => {
            debug!(path = %path.display(), json_len = json.len(), "Writing settings to disk");
            if let Err(e) = tokio::fs::write(&path, &json).await {
                error!(path = %path.display(), error = %e, "Failed to write settings to disk");
            } else {
                info!(path = %path.display(), "Settings persisted to disk successfully");
            }
        }
        Err(e) => {
            error!(error = %e, "Failed to serialize settings");
        }
    }

    Ok(())
}
