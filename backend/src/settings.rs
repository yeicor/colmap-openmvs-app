use colmap_openmvs_api::Settings;
use dioxus::Result;
use std::convert::Infallible;
use std::path::PathBuf;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

pub static SETTINGS: dioxus::fullstack::Lazy<RwLock<Settings>> =
    dioxus::fullstack::Lazy::new(|| async move {
        Ok::<_, Infallible>(RwLock::new(load_settings_from_disk()))
    });

pub(crate) fn default_projects_folder() -> String {
    if cfg!(target_os = "android") {
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

pub(crate) fn default_proot_dir() -> String {
    if cfg!(target_os = "android") {
        "/data/data/com.github.yeicor.colmap_openmvs_app/files/runtimes/proot".to_string()
    } else if cfg!(target_os = "ios") {
        "~/Documents/runtimes/proot".to_string()
    } else if cfg!(target_os = "windows") {
        match std::env::var("APPDATA") {
            Ok(appdata) => format!("{}/colmap_openmvs/runtimes/proot", appdata),
            Err(_) => "./runtimes/proot".to_string(),
        }
    } else if cfg!(target_os = "macos") {
        match std::env::var("HOME") {
            Ok(home) => format!(
                "{}/Library/Application Support/colmap_openmvs/runtimes/proot",
                home
            ),
            Err(_) => "./runtimes/proot".to_string(),
        }
    } else {
        match std::env::var("HOME") {
            Ok(home) => format!("{}/.local/share/colmap_openmvs/runtimes/proot", home),
            Err(_) => "./runtimes/proot".to_string(),
        }
    }
}

fn default_settings() -> Settings {
    Settings {
        projects_folder: default_projects_folder(),
        default_image_tag: None,
    }
}

pub fn settings_file_path(projects_folder: &str) -> PathBuf {
    PathBuf::from(projects_folder).join("settings.json")
}

fn load_settings_from_disk() -> Settings {
    let path = settings_file_path(&default_projects_folder());
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
    Ok(SETTINGS.read().await.clone())
}

pub async fn update_settings(new_settings: Settings) -> Result<()> {
    debug!(projects_folder = %new_settings.projects_folder, "Updating settings");

    let mut settings = SETTINGS.write().await;
    *settings = new_settings;

    // Persist to disk
    debug!(folder = %settings.projects_folder, "Creating projects folder");
    if let Err(e) = std::fs::create_dir_all(&settings.projects_folder) {
        error!(folder = %settings.projects_folder, error = %e, "Failed to create projects folder");
    } else {
        let path = settings_file_path(&settings.projects_folder);
        debug!(path = %path.display(), "Serializing settings for persistence");
        match serde_json::to_string_pretty(&*settings) {
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
    }

    Ok(())
}
