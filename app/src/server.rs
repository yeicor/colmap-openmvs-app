//! default functions with Dioxus fullstack macros
//! These wrap the backend implementations and provide the RPC interface for the client

use dioxus::prelude::*;

use crate::fullstack_compat::ByteStream;

use colmap_openmvs_api::{
    ConfigSchema, ImageTagInfo, LoadedProjectConfig, OutputFile, PreparedImageInfo, Project,
    ProjectRunStatus, RuntimeInfo, SavedProjectConfig, Settings, TaskInfo,
};

#[cfg(all(feature = "server", not(feature = "demo")))]
use colmap_openmvs_backend as backend;

#[cfg(feature = "demo")]
use crate::demo as backend;

#[cfg_attr(not(feature = "demo"), post("/api/startup"))]
pub async fn startup() -> Result<String> {
    backend::startup().await
}

#[cfg_attr(not(feature = "demo"), get("/api/projects"))]
pub async fn get_projects() -> Result<Vec<Project>> {
    backend::get_projects().await
}

#[cfg_attr(not(feature = "demo"), post("/api/projects/{name}"))]
pub async fn create_project(name: String) -> Result<Project> {
    backend::create_project(name).await
}

#[cfg_attr(not(feature = "demo"), delete("/api/projects/{name}"))]
pub async fn delete_project(name: String) -> Result<()> {
    backend::delete_project(name).await
}

#[cfg_attr(not(feature = "demo"), patch("/api/projects/{name}"))]
pub async fn rename_project(name: String, new_name: String) -> Result<Project> {
    backend::rename_project(name, new_name).await
}

#[cfg_attr(not(feature = "demo"), get("/api/settings"))]
pub async fn get_settings() -> Result<Settings> {
    backend::get_settings().await
}

#[cfg_attr(not(feature = "demo"), post("/api/settings"))]
pub async fn update_settings(new_settings: Settings) -> Result<()> {
    backend::update_settings(new_settings).await
}

#[cfg_attr(not(feature = "demo"), get("/api/projects/{project_name}/images"))]
pub async fn get_project_images(project_name: String) -> Result<Vec<String>> {
    backend::get_project_images(project_name).await
}

/// Fetch an image as raw bytes via the Dioxus default-function protocol.
///
/// Prefer this over constructing a URL manually — the framework routes the
/// request through the configured default URL regardless of deployment topology
/// (bundled desktop, web behind a custom origin, etc.).
#[cfg_attr(
    not(feature = "demo"),
    get("/api/projects/{project_name}/images/{image_name}/bytes")
)]
pub async fn get_project_image_bytes(
    project_name: String,
    image_name: String,
) -> Result<ByteStream> {
    backend::get_project_image_bytes(project_name, image_name).await
}

#[cfg_attr(
    not(feature = "demo"),
    post("/api/projects/{project_name}/images/{image_name}")
)]
pub async fn add_project_image(
    project_name: String,
    image_name: String,
    body: ByteStream,
) -> Result<()> {
    backend::add_project_image(project_name, image_name, body).await
}

#[cfg_attr(
    not(feature = "demo"),
    delete("/api/projects/{project_name}/images/{image_name}")
)]
pub async fn delete_project_image(project_name: String, image_name: String) -> Result<()> {
    backend::delete_project_image(project_name, image_name).await
}

#[cfg_attr(not(feature = "demo"), delete("/api/projects/{project_name}/images"))]
pub async fn clear_project_images(project_name: String) -> Result<()> {
    backend::clear_project_images(project_name).await
}

#[cfg_attr(
    not(feature = "demo"),
    post("/api/projects/{project_name}/images/resize/{max_dimension}")
)]
pub async fn batch_resize_images(project_name: String, max_dimension: u32) -> Result<String> {
    backend::batch_resize_images(project_name, max_dimension).await
}

#[cfg_attr(
    not(feature = "demo"),
    post("/api/projects/{project_name}/images/demo/{source_id}")
)]
pub async fn download_demo_images(project_name: String, source_id: String) -> Result<String> {
    backend::download_demo_images(project_name, source_id).await
}

// ---------------------------------------------------------------------------
// Runtime management
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "demo"), get("/api/runtimes/proot/info"))]
pub async fn get_runtime_info() -> Result<RuntimeInfo> {
    backend::get_runtime_info().await
}

#[cfg_attr(not(feature = "demo"), get("/api/runtimes/proot/versions"))]
pub async fn get_available_runtime_versions() -> Result<Vec<String>> {
    backend::get_available_runtime_versions().await
}

#[cfg_attr(not(feature = "demo"), post("/api/runtimes/proot/install"))]
pub async fn download_runtime_version(version: String) -> Result<()> {
    backend::download_runtime_version(version).await
}

#[cfg_attr(not(feature = "demo"), delete("/api/runtimes/proot/binary"))]
pub async fn delete_runtime_binary() -> Result<()> {
    backend::delete_runtime_binary().await
}

#[cfg_attr(not(feature = "demo"), get("/api/runtimes/proot/images"))]
pub async fn list_runtime_images() -> Result<Vec<PreparedImageInfo>> {
    backend::list_runtime_images().await
}

#[cfg_attr(not(feature = "demo"), post("/api/runtimes/proot/images/prepare"))]
pub async fn prepare_runtime_image(image: String) -> Result<String> {
    backend::prepare_runtime_image(image).await
}

#[cfg_attr(not(feature = "demo"), delete("/api/runtimes/proot/images/remove"))]
pub async fn remove_runtime_image(image_tag: String) -> Result<()> {
    backend::remove_runtime_image(image_tag).await
}

#[cfg_attr(
    not(feature = "demo"),
    get("/api/runtimes/proot/images/available-tags")
)]
pub async fn list_available_image_tags() -> Result<Vec<ImageTagInfo>> {
    backend::list_available_image_tags().await
}

#[cfg_attr(not(feature = "demo"), get("/api/runtimes/proot/images/embedded-tag"))]
pub async fn get_embedded_image_tag() -> Result<Option<String>> {
    backend::get_embedded_image_tag().await
}

// ---------------------------------------------------------------------------
// Configuration schema
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "demo"), post("/api/config"))]
pub async fn get_image_config(image_tag: String) -> Result<ConfigSchema> {
    backend::get_image_config(image_tag).await
}

#[cfg_attr(not(feature = "demo"), get("/api/projects/{project_name}/config"))]
pub async fn load_project_config(project_name: String) -> Result<LoadedProjectConfig> {
    backend::load_project_config(project_name).await
}

#[cfg_attr(not(feature = "demo"), post("/api/projects/{project_name}/config"))]
pub async fn save_project_config(project_name: String, config: SavedProjectConfig) -> Result<()> {
    backend::save_project_config(project_name, config).await
}

// ---------------------------------------------------------------------------
// Task management
// ---------------------------------------------------------------------------

#[cfg_attr(
    not(feature = "demo"),
    get("/api/tasks?kind_filter&context_key_filter")
)]
pub async fn list_tasks(
    kind_filter: Option<colmap_openmvs_api::TaskKind>,
    context_key_filter: Option<String>,
) -> Result<Vec<TaskInfo>> {
    backend::list_tasks(kind_filter, context_key_filter).await
}

#[cfg_attr(not(feature = "demo"), get("/api/tasks/{task_id}"))]
pub async fn get_task_info(task_id: String) -> Result<Option<TaskInfo>> {
    backend::get_task_info(task_id).await
}

#[cfg_attr(not(feature = "demo"), get("/api/tasks/{task_id}/poll?cursor&limit"))]
pub async fn poll_task_events(
    task_id: String,
    cursor: usize,
    limit: Option<usize>,
) -> Result<colmap_openmvs_api::TaskEventBatch> {
    backend::poll_task_events(task_id, cursor, limit).await
}

#[cfg_attr(not(feature = "demo"), delete("/api/tasks/{task_id}"))]
pub async fn cancel_task(task_id: String) -> Result<()> {
    backend::cancel_task(task_id).await
}

// ---------------------------------------------------------------------------
// Pipeline execution
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "demo"), post("/api/projects/{project_name}/pipeline"))]
pub async fn run_pipeline(project_name: String, recover_logs: bool) -> Result<String> {
    backend::run_pipeline(project_name, recover_logs).await
}

#[cfg_attr(not(feature = "demo"), get("/api/projects/{project_name}/run-status"))]
pub async fn get_project_run_status(project_name: String) -> Result<ProjectRunStatus> {
    backend::get_project_run_status(project_name).await
}

// ---------------------------------------------------------------------------
// Docker runtime
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "demo"), get("/api/runtimes/docker/info"))]
pub async fn get_docker_runtime_info() -> Result<RuntimeInfo> {
    backend::get_docker_runtime_info().await
}

#[cfg_attr(not(feature = "demo"), get("/api/runtimes/docker/images"))]
pub async fn list_docker_images() -> Result<Vec<PreparedImageInfo>> {
    backend::list_docker_images().await
}

#[cfg_attr(not(feature = "demo"), post("/api/runtimes/docker/images/prepare"))]
pub async fn prepare_docker_image(image: String) -> Result<String> {
    backend::prepare_docker_image(image).await
}

#[cfg_attr(not(feature = "demo"), delete("/api/runtimes/docker/images/remove"))]
pub async fn remove_docker_image(image_tag: String) -> Result<()> {
    backend::remove_docker_image(image_tag).await
}

// ---------------------------------------------------------------------------
// Project outputs
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "demo"), get("/api/projects/{project_name}/outputs"))]
pub async fn list_project_outputs(project_name: String) -> Result<Vec<OutputFile>> {
    backend::list_project_outputs(project_name).await
}

/// Return the raw bytes of an output file (used for download links).

/// Fetch an output file as raw bytes via the Dioxus default-function protocol.
///
/// Prefer this over constructing a URL — the framework routes the request
/// through the configured default URL regardless of deployment topology.
#[cfg_attr(
    not(feature = "demo"),
    get("/api/projects/{project_name}/outputs/bytes?relative_path")
)]
pub async fn get_project_output_bytes(
    project_name: String,
    relative_path: String,
) -> Result<ByteStream> {
    backend::get_project_output_bytes(project_name, relative_path).await
}

/// Delete an output file or directory.
#[cfg_attr(
    not(feature = "demo"),
    post("/api/projects/{project_name}/outputs/delete")
)]
pub async fn delete_project_output(project_name: String, relative_path: String) -> Result<()> {
    backend::delete_project_output(project_name, relative_path).await
}

/// Write an output file (bytes uploaded from the client).
#[cfg_attr(
    not(feature = "demo"),
    post("/api/projects/{project_name}/outputs/write?relative_path")
)]
pub async fn write_project_output(
    project_name: String,
    relative_path: String,
    body: ByteStream,
) -> Result<()> {
    backend::write_project_output(project_name, relative_path, body).await
}

/// Delete all output files/directories, preserving only `images/` and `config.sh`.
#[cfg_attr(
    not(feature = "demo"),
    post("/api/projects/{project_name}/outputs/clear")
)]
pub async fn clear_project_outputs(project_name: String) -> Result<()> {
    backend::clear_project_outputs(project_name).await
}

// ---------------------------------------------------------------------------
// Theme / color-scheme detection
// ---------------------------------------------------------------------------

/// Returns the default-side color-scheme preference.
///
/// * `None`        – no override; let the browser's `prefers-color-scheme`
///                   CSS media query decide.
/// * `Some(false)` – force light mode.
/// * `Some(true)`  – force dark mode.
///
/// On Android the WebView may not propagate `prefers-color-scheme` reliably,
/// so the default probes the system UI mode.  Currently defaults to
/// `Some(false)` (light) on Android until JNI detection is wired up.
#[cfg_attr(not(feature = "demo"), get("/api/theme/dark-mode"))]
pub async fn get_dark_mode() -> Result<Option<bool>> {
    backend::get_dark_mode().await
}
