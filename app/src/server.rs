//! Server functions with Dioxus fullstack macros
//! These wrap the backend implementations and provide the RPC interface for the client

use dioxus::{
    fullstack::{FileStream, ServerEvents},
    prelude::*,
};

use colmap_openmvs_api::{DemoProgressEvent, Project, ResizeProgressEvent, Settings};

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
