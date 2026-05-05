//! Server functions with Dioxus fullstack macros
//! These wrap the backend implementations and provide the RPC interface for the client

use dioxus::{
    fullstack::{FileStream, ServerEvents},
    prelude::*,
};

use colmap_openmvs_api::{
    ConfigSchema, DemoProgressEvent, ImageTagInfo, LoadedProjectConfig, PrepareProgress,
    PreparedImageInfo, Project, ResizeProgressEvent, RuntimeInfo, SavedProjectConfig, Settings,
};

#[cfg(feature = "server")]
use colmap_openmvs_backend as backend;

#[get("/projects")]
pub async fn get_projects() -> Result<Vec<Project>> {
    backend::get_projects().await.map_err(Into::into)
}

#[post("/projects/:name")]
pub async fn create_project(name: String) -> Result<Project> {
    backend::create_project(name).await.map_err(Into::into)
}

#[delete("/projects/:name")]
pub async fn delete_project(name: String) -> Result<()> {
    backend::delete_project(name).await.map_err(Into::into)
}

#[patch("/projects/:name")]
pub async fn rename_project(name: String, new_name: String) -> Result<Project> {
    backend::rename_project(name, new_name).await
}

#[get("/settings")]
pub async fn get_settings() -> Result<Settings> {
    backend::get_settings().await.map_err(Into::into)
}

#[post("/settings")]
pub async fn update_settings(new_settings: Settings) -> Result<()> {
    backend::update_settings(new_settings).await
}

#[get("/projects/:project_name/images")]
pub async fn get_project_images(project_name: String) -> Result<Vec<String>> {
    backend::get_project_images(project_name).await
}

#[get("/projects/:project_name/images/:image_name")]
pub async fn get_project_image(project_name: String, image_name: String) -> Result<FileStream> {
    backend::get_project_image(project_name, image_name).await
}

#[post("/projects/:project_name/images/:image_name")]
pub async fn add_project_image(
    project_name: String,
    image_name: String,
    body: Vec<u8>,
) -> Result<()> {
    backend::add_project_image(project_name, image_name, body).await
}

#[delete("/projects/:project_name/images/:image_name")]
pub async fn delete_project_image(project_name: String, image_name: String) -> Result<()> {
    backend::delete_project_image(project_name, image_name).await
}

#[delete("/projects/:project_name/images")]
pub async fn clear_project_images(project_name: String) -> Result<()> {
    backend::clear_project_images(project_name).await
}

#[post("/projects/:project_name/images/resize/:max_dimension")]
pub async fn batch_resize_images(
    project_name: String,
    max_dimension: u32,
) -> Result<ServerEvents<ResizeProgressEvent>> {
    backend::batch_resize_images(project_name, max_dimension).await
}

#[post("/projects/:project_name/images/demo")]
pub async fn download_demo_images(project_name: String) -> Result<ServerEvents<DemoProgressEvent>> {
    backend::download_demo_images(project_name).await
}

// ---------------------------------------------------------------------------
// Runtime management
// ---------------------------------------------------------------------------

#[get("/runtimes/proot/info")]
pub async fn get_runtime_info() -> Result<RuntimeInfo> {
    backend::get_runtime_info().await
}

#[get("/runtimes/proot/versions")]
pub async fn get_available_runtime_versions() -> Result<Vec<String>> {
    backend::get_available_runtime_versions().await
}

#[post("/runtimes/proot/install")]
pub async fn download_runtime_version(version: String) -> Result<()> {
    backend::download_runtime_version(version).await
}

#[get("/runtimes/proot/images")]
pub async fn list_runtime_images() -> Result<Vec<PreparedImageInfo>> {
    backend::list_runtime_images().await
}

#[post("/runtimes/proot/images/prepare")]
pub async fn prepare_runtime_image(image: String) -> Result<ServerEvents<PrepareProgress>> {
    backend::prepare_runtime_image(image).await
}

#[delete("/runtimes/proot/images/remove")]
pub async fn remove_runtime_image(image_tag: String) -> Result<()> {
    backend::remove_runtime_image(image_tag).await
}

#[get("/runtimes/proot/images/available-tags")]
pub async fn list_available_image_tags() -> Result<Vec<ImageTagInfo>> {
    backend::list_available_image_tags().await
}

// ---------------------------------------------------------------------------
// Configuration schema
// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Configuration schema
// ---------------------------------------------------------------------------

#[post("/config")]
pub async fn get_image_config(image_tag: String) -> Result<ConfigSchema> {
    backend::get_image_config(image_tag)
        .await
        .map_err(Into::into)
}

#[get("/projects/:project_name/config")]
pub async fn load_project_config(project_name: String) -> Result<LoadedProjectConfig> {
    // Get the project to retrieve its path
    let project = backend::get_projects()
        .await?
        .into_iter()
        .find(|p| p.name == project_name)
        .ok_or_else(|| dioxus::prelude::ServerFnError::new("Project not found"))?;

    backend::load_project_config(project.path)
        .await
        .map_err(Into::into)
}

#[post("/projects/:project_name/config")]
pub async fn save_project_config(project_name: String, config: SavedProjectConfig) -> Result<()> {
    // Get the project to retrieve its path
    let project = backend::get_projects()
        .await?
        .into_iter()
        .find(|p| p.name == project_name)
        .ok_or_else(|| dioxus::prelude::ServerFnError::new("Project not found"))?;

    backend::save_project_config(project.path, config)
        .await
        .map_err(Into::into)
}
