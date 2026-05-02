use dioxus::fullstack::Lazy;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    pub projects_folder: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            projects_folder: "./projects".to_string(),
        }
    }
}

#[cfg(feature = "server")]
pub static SETTINGS: Lazy<RwLock<Settings>> =
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
