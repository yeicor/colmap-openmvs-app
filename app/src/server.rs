//! Server functions with Dioxus fullstack macros
//! These wrap the backend implementations and provide the RPC interface for the client

use dioxus::{fullstack::FileStream, prelude::*};

use colmap_openmvs_api::{
    ConfigSchema, ImageTagInfo, LoadedProjectConfig, OutputFile, PreparedImageInfo, Project,
    ProjectRunStatus, RuntimeInfo, SavedProjectConfig, Settings, TaskInfo,
};

#[cfg(feature = "server")]
use tracing::{info, warn};

#[cfg(feature = "server")]
use colmap_openmvs_backend as backend;

// Initialize Android runtime environment on first server startup
#[cfg(feature = "server")]
static ANDROID_STARTUP: std::sync::OnceLock<std::sync::Arc<tokio::sync::Mutex<bool>>> =
    std::sync::OnceLock::new();

#[cfg(feature = "server")]
async fn ensure_android_startup() {
    let started =
        ANDROID_STARTUP.get_or_init(|| std::sync::Arc::new(tokio::sync::Mutex::new(false)));

    let mut done = started.lock().await;
    if !*done {
        // First repair any invalid settings paths from a previous app install
        if let Err(e) = backend::repair_android_settings().await {
            warn!(error = %e, "Android startup: settings repair failed or skipped");
        }
        // Then set up the runtime with the repaired/validated settings
        match backend::setup_android_runtime().await {
            Ok(()) => info!("Android startup: completed successfully"),
            Err(e) => warn!(error = %e, "Android startup: failed or skipped"),
        }
        *done = true;
    }
}

#[get("/api/projects")]
pub async fn get_projects() -> Result<Vec<Project>> {
    #[cfg(feature = "server")]
    ensure_android_startup().await;
    backend::get_projects().await
}

#[post("/api/projects/{name}")]
pub async fn create_project(name: String) -> Result<Project> {
    backend::create_project(name).await
}

#[delete("/api/projects/{name}")]
pub async fn delete_project(name: String) -> Result<()> {
    backend::delete_project(name).await
}

#[patch("/api/projects/{name}")]
pub async fn rename_project(name: String, new_name: String) -> Result<Project> {
    backend::rename_project(name, new_name).await
}

#[get("/api/settings")]
pub async fn get_settings() -> Result<Settings> {
    backend::get_settings().await
}

#[post("/api/settings")]
pub async fn update_settings(new_settings: Settings) -> Result<()> {
    backend::update_settings(new_settings).await
}

#[get("/api/projects/{project_name}/images")]
pub async fn get_project_images(project_name: String) -> Result<Vec<String>> {
    backend::get_project_images(project_name).await
}

#[get("/api/projects/{project_name}/images/{image_name}")]
pub async fn get_project_image(project_name: String, image_name: String) -> Result<FileStream> {
    backend::get_project_image(project_name, image_name).await
}

#[post("/api/projects/{project_name}/images/{image_name}")]
pub async fn add_project_image(
    project_name: String,
    image_name: String,
    body: Vec<u8>,
) -> Result<()> {
    backend::add_project_image(project_name, image_name, body).await
}

#[delete("/api/projects/{project_name}/images/{image_name}")]
pub async fn delete_project_image(project_name: String, image_name: String) -> Result<()> {
    backend::delete_project_image(project_name, image_name).await
}

#[delete("/api/projects/{project_name}/images")]
pub async fn clear_project_images(project_name: String) -> Result<()> {
    backend::clear_project_images(project_name).await
}

#[post("/api/projects/{project_name}/images/resize/{max_dimension}")]
pub async fn batch_resize_images(project_name: String, max_dimension: u32) -> Result<String> {
    backend::batch_resize_images(project_name, max_dimension).await
}

#[post("/api/projects/{project_name}/images/demo/{source_id}")]
pub async fn download_demo_images(project_name: String, source_id: String) -> Result<String> {
    backend::download_demo_images(project_name, source_id).await
}

// ---------------------------------------------------------------------------
// Runtime management
// ---------------------------------------------------------------------------

#[get("/api/runtimes/proot/info")]
pub async fn get_runtime_info() -> Result<RuntimeInfo> {
    #[cfg(feature = "server")]
    ensure_android_startup().await;
    backend::get_runtime_info().await
}

#[get("/api/runtimes/proot/versions")]
pub async fn get_available_runtime_versions() -> Result<Vec<String>> {
    backend::get_available_runtime_versions().await
}

#[post("/api/runtimes/proot/install")]
pub async fn download_runtime_version(version: String) -> Result<()> {
    backend::download_runtime_version(version).await
}

#[delete("/api/runtimes/proot/binary")]
pub async fn delete_runtime_binary() -> Result<()> {
    backend::delete_runtime_binary().await
}

#[get("/api/runtimes/proot/images")]
pub async fn list_runtime_images() -> Result<Vec<PreparedImageInfo>> {
    #[cfg(feature = "server")]
    ensure_android_startup().await;
    backend::list_runtime_images().await
}

#[post("/api/runtimes/proot/images/prepare")]
pub async fn prepare_runtime_image(image: String) -> Result<String> {
    backend::prepare_runtime_image(image).await
}

#[delete("/api/runtimes/proot/images/remove")]
pub async fn remove_runtime_image(image_tag: String) -> Result<()> {
    backend::remove_runtime_image(image_tag).await
}

#[get("/api/runtimes/proot/images/available-tags")]
pub async fn list_available_image_tags() -> Result<Vec<ImageTagInfo>> {
    #[cfg(feature = "server")]
    ensure_android_startup().await;
    backend::list_available_image_tags().await
}

#[get("/api/runtimes/proot/images/embedded-tag")]
pub async fn get_embedded_image_tag() -> Result<Option<String>> {
    backend::get_embedded_image_tag().await
}

#[post("/api/repair-android-settings")]
pub async fn repair_android_settings() -> Result<String> {
    backend::repair_android_settings().await
}

// ---------------------------------------------------------------------------
// Configuration schema
// ---------------------------------------------------------------------------

#[post("/api/config")]
pub async fn get_image_config(image_tag: String) -> Result<ConfigSchema> {
    Ok(backend::get_image_config(image_tag).await?)
}

#[get("/api/projects/{project_name}/config")]
pub async fn load_project_config(project_name: String) -> Result<LoadedProjectConfig> {
    // Get the project to retrieve its path
    let project = backend::get_projects()
        .await?
        .into_iter()
        .find(|p| p.name == project_name)
        .ok_or_else(|| dioxus::prelude::ServerFnError::new("Project not found"))?;

    Ok(backend::load_project_config(project.path).await?)
}

#[post("/api/projects/{project_name}/config")]
pub async fn save_project_config(project_name: String, config: SavedProjectConfig) -> Result<()> {
    // Get the project to retrieve its path
    let project = backend::get_projects()
        .await?
        .into_iter()
        .find(|p| p.name == project_name)
        .ok_or_else(|| dioxus::prelude::ServerFnError::new("Project not found"))?;

    Ok(backend::save_project_config(project.path, config).await?)
}

// ---------------------------------------------------------------------------
// Task management
// ---------------------------------------------------------------------------

#[get("/api/tasks?kind_filter&context_key_filter")]
pub async fn list_tasks(
    kind_filter: Option<String>,
    context_key_filter: Option<String>,
) -> Result<Vec<TaskInfo>> {
    let kind = kind_filter.and_then(|s| match s.as_str() {
        "PrepareImage" => Some(colmap_openmvs_api::TaskKind::PrepareImage),
        "DownloadDemo" => Some(colmap_openmvs_api::TaskKind::DownloadDemo),
        "BatchResize" => Some(colmap_openmvs_api::TaskKind::BatchResize),
        "RunPipeline" => Some(colmap_openmvs_api::TaskKind::RunPipeline),
        "DryRunPipeline" => Some(colmap_openmvs_api::TaskKind::DryRunPipeline),
        _ => None,
    });
    backend::list_tasks(kind, context_key_filter).await
}

#[get("/api/tasks/{task_id}")]
pub async fn get_task_info(task_id: String) -> Result<Option<TaskInfo>> {
    backend::get_task_info(task_id).await
}

#[get("/api/tasks/{task_id}/poll?cursor")]
pub async fn poll_task_events(
    task_id: String,
    cursor: usize,
) -> Result<colmap_openmvs_api::TaskEventBatch> {
    backend::poll_task_events(task_id, cursor).await
}

#[delete("/api/tasks/{task_id}")]
pub async fn cancel_task(task_id: String) -> Result<()> {
    backend::cancel_task(task_id).await
}

// ---------------------------------------------------------------------------
// Pipeline execution
// ---------------------------------------------------------------------------

#[post("/api/projects/{project_name}/pipeline")]
pub async fn run_pipeline(project_name: String, dry_run: bool) -> Result<String> {
    Ok(backend::run_pipeline(project_name, dry_run).await?)
}

#[get("/api/projects/{project_name}/run-status")]
pub async fn get_project_run_status(project_name: String) -> Result<ProjectRunStatus> {
    backend::get_project_run_status(project_name).await
}

// ---------------------------------------------------------------------------
// Docker runtime
// ---------------------------------------------------------------------------

#[get("/api/runtimes/docker/info")]
pub async fn get_docker_runtime_info() -> Result<RuntimeInfo> {
    #[cfg(feature = "server")]
    ensure_android_startup().await;
    backend::get_docker_runtime_info().await
}

#[get("/api/runtimes/docker/images")]
pub async fn list_docker_images() -> Result<Vec<PreparedImageInfo>> {
    backend::list_docker_images().await
}

#[post("/api/runtimes/docker/images/prepare")]
pub async fn prepare_docker_image(image: String) -> Result<String> {
    backend::prepare_docker_image(image).await
}

#[delete("/api/runtimes/docker/images/remove")]
pub async fn remove_docker_image(image_tag: String) -> Result<()> {
    backend::remove_docker_image(image_tag).await
}

// ---------------------------------------------------------------------------
// Project outputs
// ---------------------------------------------------------------------------

#[get("/api/projects/{project_name}/outputs")]
pub async fn list_project_outputs(project_name: String) -> Result<Vec<OutputFile>> {
    backend::list_project_outputs(project_name).await
}

/// Return the raw bytes of an output file (used for download links).
/// `relative_path` is a query parameter (e.g. ?relative_path=colmap%2Fdatabase.db).
#[get("/api/projects/{project_name}/outputs/file?relative_path")]
pub async fn get_project_output(project_name: String, relative_path: String) -> Result<FileStream> {
    backend::get_project_output(project_name, relative_path).await
}

/// Return an output file in a viewer-friendly PLY format.
/// For PLY files this is a pass-through; for points3D.bin it converts to ASCII PLY.
#[get("/api/projects/{project_name}/outputs/view?relative_path")]
pub async fn get_project_output_for_viewer(
    project_name: String,
    relative_path: String,
) -> Result<Vec<u8>> {
    backend::get_project_output_for_viewer(project_name, relative_path).await
}

/// Delete an output file or directory.
#[post("/api/projects/{project_name}/outputs/delete")]
pub async fn delete_project_output(project_name: String, relative_path: String) -> Result<()> {
    backend::delete_project_output(project_name, relative_path).await
}

/// Delete all output files/directories, preserving only `images/` and `config.sh`.
#[post("/api/projects/{project_name}/outputs/clear")]
pub async fn clear_project_outputs(project_name: String) -> Result<()> {
    backend::clear_project_outputs(project_name).await
}

// ---------------------------------------------------------------------------
// Theme / color-scheme detection
// ---------------------------------------------------------------------------

/// Returns the server-side color-scheme preference.
///
/// * `None`        – no override; let the browser's `prefers-color-scheme`
///                   CSS media query decide.
/// * `Some(false)` – force light mode.
/// * `Some(true)`  – force dark mode.
///
/// On Android the WebView may not propagate `prefers-color-scheme` reliably,
/// so the server probes the system UI mode.  Currently defaults to
/// `Some(false)` (light) on Android until JNI detection is wired up.
#[get("/api/theme/dark-mode")]
pub async fn get_dark_mode() -> Result<Option<bool>> {
    #[cfg(feature = "server")]
    return Ok(backend::get_dark_mode().await?);
    #[cfg(not(feature = "server"))]
    Ok(None)
}
