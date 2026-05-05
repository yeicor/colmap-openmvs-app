use colmap_openmvs_api::Settings;
use dioxus::Result;
use std::convert::Infallible;
use std::path::PathBuf;
use tokio::sync::RwLock;

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
    match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<Settings>(&contents) {
            Ok(settings) => settings,
            Err(e) => {
                eprintln!(
                    "Failed to parse settings from {}: {}. Using defaults.",
                    path.display(),
                    e
                );
                default_settings()
            }
        },
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "Failed to read settings from {}: {}. Using defaults.",
                    path.display(),
                    e
                );
            }
            default_settings()
        }
    }
}

pub async fn get_settings() -> Result<Settings> {
    Ok(SETTINGS.read().await.clone())
}

pub async fn update_settings(new_settings: Settings) -> Result<()> {
    let mut settings = SETTINGS.write().await;
    *settings = new_settings;

    // Persist to disk
    if let Err(e) = std::fs::create_dir_all(&settings.projects_folder) {
        eprintln!(
            "Failed to create projects folder '{}': {}",
            settings.projects_folder, e
        );
    } else {
        let path = settings_file_path(&settings.projects_folder);
        match serde_json::to_string_pretty(&*settings) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    eprintln!("Failed to write settings to {}: {}", path.display(), e);
                }
            }
            Err(e) => {
                eprintln!("Failed to serialize settings: {}", e);
            }
        }
    }

    Ok(())
}
