use colmap_openmvs_api::Settings;
use dioxus::Result;
use std::convert::Infallible;
use tokio::sync::RwLock;

pub static SETTINGS: dioxus::fullstack::Lazy<RwLock<Settings>> = dioxus::fullstack::Lazy::new(
    || async move { Ok::<_, Infallible>(RwLock::new(default_settings())) },
);

fn default_settings() -> Settings {
    let projects_folder = if cfg!(target_os = "android") {
        // Android: use app-specific files dir
        "/data/data/com.github.yeicor.colmap_openmvs_app/files/projects".to_string()
    } else if cfg!(target_os = "ios") {
        // iOS: use Documents directory (typical for user data)
        "~/Documents/projects".to_string()
    } else if cfg!(target_os = "windows") {
        // Windows: use %APPDATA%/colmap_openmvs/projects
        match std::env::var("APPDATA") {
            Ok(appdata) => format!("{}/colmap_openmvs/projects", appdata),
            Err(_) => "./projects".to_string(),
        }
    } else if cfg!(target_os = "macos") {
        // macOS: use ~/Library/Application Support/colmap_openmvs/projects
        match std::env::var("HOME") {
            Ok(home) => format!(
                "{}/Library/Application Support/colmap_openmvs/projects",
                home
            ),
            Err(_) => "./projects".to_string(),
        }
    } else {
        // Linux and other Unix: use ~/.local/share/colmap_openmvs/projects
        match std::env::var("HOME") {
            Ok(home) => format!("{}/.local/share/colmap_openmvs/projects", home),
            Err(_) => "./projects".to_string(),
        }
    };
    Settings { projects_folder }
}

pub async fn get_settings() -> Result<Settings> {
    Ok(SETTINGS.read().await.clone())
}

pub async fn update_settings(new_settings: Settings) -> Result<()> {
    let mut settings = SETTINGS.write().await;
    *settings = new_settings;
    // TODO: Persist and reload settings from disk, and handle any necessary migrations or validations.
    Ok(())
}
