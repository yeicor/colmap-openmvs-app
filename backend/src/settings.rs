use colmap_openmvs_api::Settings;
use dioxus::Result;
use std::path::PathBuf;
use std::{collections::HashMap, convert::Infallible};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

pub static SETTINGS: dioxus::fullstack::Lazy<RwLock<Settings>> =
    dioxus::fullstack::Lazy::new(|| async move {
        Ok::<_, Infallible>(RwLock::new(load_settings_from_disk()))
    });

pub(crate) fn default_projects_folder() -> String {
    if cfg!(debug_assertions) {
        "./devstorage/projects".to_string()
    } else if cfg!(target_os = "android") {
        "/data/data/com.github.yeicor.colmap_openmvs_app/files/projects".to_string()
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

/// Read the embedded image tag from `librootfs-manifest.so` bundled in the APK's jniLibs.
///
/// Despite the `.so` extension, the file is a JSON document with at least a `"tag"` field, e.g.:
/// ```json
/// { "tag": "mirror.gcr.io/yeicor/colmap-openmvs:20240101-abc123" }
/// ```
/// The `.so` extension is used so Android's AGP packaging includes it automatically
/// without needing a custom Gradle merge task.
#[cfg(target_os = "android")]
pub fn read_embedded_image_tag() -> Option<String> {
    let jnilib_dir = get_android_native_lib_dir()?;
    let metadata_path = format!("{}/librootfs-manifest.so", jnilib_dir);

    match std::fs::read_to_string(&metadata_path) {
        Ok(contents) => match serde_json::from_str::<serde_json::Value>(&contents) {
            Ok(json) => {
                let tag = json.get("tag").and_then(|v| v.as_str()).map(str::to_owned);
                if tag.is_none() {
                    warn!(path = %metadata_path, "Embedded rootfs manifest has no 'tag' field");
                }
                tag
            }
            Err(e) => {
                warn!(path = %metadata_path, error = %e, "Failed to parse embedded metadata JSON");
                None
            }
        },
        Err(e) => {
            warn!(path = %metadata_path, error = %e, "Failed to read embedded metadata file");
            None
        }
    }
}

/// Public, cross-platform wrapper around `read_embedded_image_tag`.
/// Returns `None` on non-Android targets.
pub fn read_embedded_image_tag_public() -> Option<String> {
    #[cfg(target_os = "android")]
    {
        read_embedded_image_tag()
    }
    #[cfg(not(target_os = "android"))]
    {
        None
    }
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
    if cfg!(debug_assertions) {
        "./devstorage/proot-images".to_string()
    } else if cfg!(target_os = "android") {
        "/data/data/com.github.yeicor.colmap_openmvs_app/files/proot-images".to_string()
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
    let default_image_tag = read_embedded_image_tag();
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
        Ok(contents) => match serde_json::from_str::<serde_json::Value>(&contents) {
            Ok(mut value) => {
                // Handle migration from old proot_folder to new split format
                // `.map(str::to_owned)` clones the value immediately, releasing the
                // immutable borrow on `value` before we mutably index into it below.
                if let Some(old_proot_folder) = value
                    .get("proot_folder")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                {
                    info!(path = %path.display(), "Migrating old proot_folder to split format");
                    if value.get("proot_binary_dir").is_none() {
                        value["proot_binary_dir"] =
                            serde_json::Value::String(default_proot_binary_dir());
                    }
                    if value.get("proot_images_dir").is_none() {
                        value["proot_images_dir"] =
                            serde_json::Value::String(old_proot_folder.to_string());
                    }
                    if let Some(obj) = value.as_object_mut() {
                        obj.remove("proot_folder");
                    }
                } else {
                    // Fresh migration: add any missing fields with their defaults
                    if value.get("proot_binary_dir").is_none() {
                        info!(path = %path.display(), "Adding missing proot_binary_dir to settings");
                        value["proot_binary_dir"] =
                            serde_json::Value::String(default_proot_binary_dir());
                    }
                    if value.get("proot_images_dir").is_none() {
                        info!(path = %path.display(), "Adding missing proot_images_dir to settings");
                        value["proot_images_dir"] =
                            serde_json::Value::String(default_proot_images_dir());
                    }
                }
                // Remove proot_custom_command if it exists (migrating away from it)
                if let Some(obj) = value.as_object_mut() {
                    obj.remove("proot_custom_command");
                }
                match serde_json::from_value::<Settings>(value) {
                    Ok(settings) => {
                        info!(path = %path.display(), "Settings loaded successfully from disk");
                        settings
                    }
                    Err(e) => {
                        error!(path = %path.display(), error = %e, "Failed to parse settings file, using defaults");
                        default_settings()
                    }
                }
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
    Ok(SETTINGS.read().await.clone())
}

pub async fn update_settings(new_settings: Settings) -> Result<()> {
    debug!(projects_folder = %new_settings.projects_folder, "Updating settings");

    let mut settings = SETTINGS.write().await;
    *settings = new_settings.clone();

    // Persist to disk
    debug!(folder = %settings.projects_folder, "Creating projects folder");
    if let Err(e) = std::fs::create_dir_all(&settings.projects_folder) {
        error!(folder = %settings.projects_folder, error = %e, "Failed to create projects folder");
    }

    // Determine the effective settings file path
    let path = get_effective_settings_path(&settings);

    // Ensure the directory for settings file exists
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                error!(dir = %parent.display(), error = %e, "Failed to create settings directory");
            }
        }
    }

    debug!(path = %path.display(), "Serializing settings for persistence");
    match serde_json::to_string_pretty(&new_settings) {
        Ok(json) => {
            debug!(path = %path.display(), json_len = json.len(), "Writing settings to disk");
            if let Err(e) = std::fs::write(&path, json) {
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
