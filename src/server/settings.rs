use std::convert::Infallible;

use dioxus::{fullstack::Lazy, prelude::*};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// The settings struct that will hold all the settings for the application
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// The path to the folder containing all the projects
    pub projects_folder: String,
}

#[cfg(feature = "server")]
impl Default for Settings {
    fn default() -> Self {
        Self {
            projects_folder: "./projects".to_string(),
        }
    }
}

#[cfg(feature = "server")]
static SETTINGS: Lazy<RwLock<Settings>> =
    Lazy::new(|| async move { Ok::<_, Infallible>(RwLock::new(Settings::default())) });

#[get("/settings")]
pub async fn get_settings() -> Result<Settings> {
    Ok(SETTINGS.read().await.clone())
}

#[post("/settings")]
pub async fn update_settings(new_settings: Settings) -> Result<()> {
    let mut settings = SETTINGS.write().await;
    *settings = new_settings;
    Ok(())
}
