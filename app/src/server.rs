//! default functions with Dioxus fullstack macros
//! These wrap the backend implementations and provide the RPC interface for the client

use dioxus::prelude::*;

use colmap_openmvs_api::{
    ConfigSchema, ImageTagInfo, LoadedProjectConfig, OutputFile, PreparedImageInfo, Project,
    ProjectRunStatus, RuntimeInfo, SavedProjectConfig, Settings, TaskInfo,
};

#[cfg(all(feature = "fullstack", feature = "demo"))]
compile_error!(
    "Cannot enable both fullstack and demo (disable default features when enabling demo)"
);

#[cfg(feature = "server")]
use colmap_openmvs_backend as backend;

#[cfg(feature = "fullstack")]
use dioxus::fullstack::ByteStream;
#[cfg(not(feature = "fullstack"))]
type ByteStream = Vec<u8>;

macro_rules! fullstack_only {
    ($expr:expr) => {{
        #[cfg(feature = "fullstack")]
        {
            return $expr.await;
        }

        #[cfg(not(feature = "fullstack"))]
        {
            Err(dioxus::CapturedError::msg(format!(
                "Fullstack-only call blocked in static builds: {}. Please run the fullstack version of the app to access this functionality.",
                stringify!($expr)
            )))
        }
    }};
}

#[cfg_attr(not(feature = "demo"), get("/api/startup"))]
pub async fn startup() -> Result<()>  {
    #[cfg(feature = "demo")]
    { return crate::demo::startup(); }
    fullstack_only!(backend::on_frontend_started())
}

#[cfg_attr(not(feature = "demo"), get("/api/projects"))]
pub async fn get_projects() -> Result<Vec<Project>>  {
    #[cfg(feature = "demo")]
    { return crate::demo::get_projects(); }
    fullstack_only!(backend::get_projects())
}

#[cfg_attr(not(feature = "demo"), post("/api/projects/{name}"))]
pub async fn create_project(name: String) -> Result<Project>  {
    #[cfg(feature = "demo")]
    { return crate::demo::create_project(name); }
    fullstack_only!(backend::create_project(name))
}

#[cfg_attr(not(feature = "demo"), delete("/api/projects/{name}"))]
pub async fn delete_project(name: String) -> Result<()>  {
    #[cfg(feature = "demo")]
    { return crate::demo::delete_project(name); }
    fullstack_only!(backend::delete_project(name))
}

#[cfg_attr(not(feature = "demo"), patch("/api/projects/{name}"))]
pub async fn rename_project(name: String, new_name: String) -> Result<Project>  {
    #[cfg(feature = "demo")]
    { return crate::demo::rename_project(name, new_name); }
    fullstack_only!(backend::rename_project(name, new_name))
}

#[cfg_attr(not(feature = "demo"), get("/api/settings"))]
pub async fn get_settings() -> Result<Settings>  {
    #[cfg(feature = "demo")]
    { return crate::demo::get_settings(); }
    fullstack_only!(backend::get_settings())
}

#[cfg_attr(not(feature = "demo"), post("/api/settings"))]
pub async fn update_settings(new_settings: Settings) -> Result<()>  {
    #[cfg(feature = "demo")]
    { return crate::demo::update_settings(new_settings); }
    fullstack_only!(backend::update_settings(new_settings))
}

#[cfg_attr(not(feature = "demo"), get("/api/projects/{project_name}/images"))]
pub async fn get_project_images(project_name: String) -> Result<Vec<String>>  {
    #[cfg(feature = "demo")]
    { return crate::demo::get_project_images(project_name); }
    fullstack_only!(backend::get_project_images(project_name))
}

/// Fetch an image as raw bytes via the Dioxus default-function protocol.
///
/// Prefer this over constructing a URL manually — the framework routes the
/// request through the configured default URL regardless of deployment topology
/// (bundled desktop, web behind a custom origin, etc.).
#[cfg_attr(
    not(feature = "demo"),
    get("/api/projects/{project_name}/images/{image_name}/bytes")
)] // TODO: Streaming response rather than json
pub async fn get_project_image_bytes(project_name: String, image_name: String) -> Result<Vec<u8>>  {
    #[cfg(feature = "demo")]
    { return crate::demo::get_project_image_bytes(project_name, image_name); }
    fullstack_only!(backend::get_project_image_bytes(project_name, image_name))
}

#[cfg_attr(
    not(feature = "demo"),
    post("/api/projects/{project_name}/images/{image_name}")
)]
pub async fn add_project_image(
    project_name: String,
    image_name: String,
    body: ByteStream,
) -> Result<()>  {
    #[cfg(feature = "demo")]
    { return crate::demo::add_project_image(project_name, image_name, body); }
    fullstack_only!(backend::add_project_image(project_name, image_name, body))
}

#[cfg_attr(
    not(feature = "demo"),
    delete("/api/projects/{project_name}/images/{image_name}")
)]
pub async fn delete_project_image(project_name: String, image_name: String) -> Result<()>  {
    #[cfg(feature = "demo")]
    { return crate::demo::delete_project_image(project_name, image_name); }
    fullstack_only!(backend::delete_project_image(project_name, image_name))
}

#[cfg_attr(not(feature = "demo"), delete("/api/projects/{project_name}/images"))]
pub async fn clear_project_images(project_name: String) -> Result<()>  {
    #[cfg(feature = "demo")]
    { return crate::demo::clear_project_images(project_name); }
    fullstack_only!(backend::clear_project_images(project_name))
}

#[cfg_attr(
    not(feature = "demo"),
    post("/api/projects/{project_name}/images/resize/{max_dimension}")
)]
pub async fn batch_resize_images(project_name: String, max_dimension: u32) -> Result<String>  {
    #[cfg(feature = "demo")]
    { return crate::demo::batch_resize_images(project_name, max_dimension); }
    fullstack_only!(backend::batch_resize_images(project_name, max_dimension))
}

#[cfg_attr(
    not(feature = "demo"),
    post("/api/projects/{project_name}/images/demo/{source_id}")
)]
pub async fn download_demo_images(project_name: String, source_id: String) -> Result<String>  {
    #[cfg(feature = "demo")]
    { return crate::demo::download_demo_images(project_name, source_id); }
    fullstack_only!(backend::download_demo_images(project_name, source_id))
}

// ---------------------------------------------------------------------------
// Runtime management
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "demo"), get("/api/runtimes/proot/info"))]
pub async fn get_runtime_info() -> Result<RuntimeInfo>  {
    #[cfg(feature = "demo")]
    { return crate::demo::get_runtime_info(); }
    fullstack_only!(backend::get_runtime_info())
}

#[cfg_attr(not(feature = "demo"), get("/api/runtimes/proot/versions"))]
pub async fn get_available_runtime_versions() -> Result<Vec<String>>  {
    #[cfg(feature = "demo")]
    { return crate::demo::get_available_runtime_versions(); }
    fullstack_only!(backend::get_available_runtime_versions())
}

#[cfg_attr(not(feature = "demo"), post("/api/runtimes/proot/install"))]
pub async fn download_runtime_version(version: String) -> Result<()>  {
    #[cfg(feature = "demo")]
    { return crate::demo::download_runtime_version(version); }
    fullstack_only!(backend::download_runtime_version(version))
}

#[cfg_attr(not(feature = "demo"), delete("/api/runtimes/proot/binary"))]
pub async fn delete_runtime_binary() -> Result<()>  {
    #[cfg(feature = "demo")]
    { return crate::demo::delete_runtime_binary(); }
    fullstack_only!(backend::delete_runtime_binary())
}

#[cfg_attr(not(feature = "demo"), get("/api/runtimes/proot/images"))]
pub async fn list_runtime_images() -> Result<Vec<PreparedImageInfo>>  {
    #[cfg(feature = "demo")]
    { return crate::demo::list_runtime_images(); }
    fullstack_only!(backend::list_runtime_images())
}

#[cfg_attr(not(feature = "demo"), post("/api/runtimes/proot/images/prepare"))]
pub async fn prepare_runtime_image(image: String) -> Result<String>  {
    #[cfg(feature = "demo")]
    { return crate::demo::prepare_runtime_image(image); }
    fullstack_only!(backend::prepare_runtime_image(image))
}

#[cfg_attr(not(feature = "demo"), delete("/api/runtimes/proot/images/remove"))]
pub async fn remove_runtime_image(image_tag: String) -> Result<()>  {
    #[cfg(feature = "demo")]
    { return crate::demo::remove_runtime_image(image_tag); }
    fullstack_only!(backend::remove_runtime_image(image_tag))
}

#[cfg_attr(
    not(feature = "demo"),
    get("/api/runtimes/proot/images/available-tags")
)]
pub async fn list_available_image_tags() -> Result<Vec<ImageTagInfo>>  {
    #[cfg(feature = "demo")]
    { return crate::demo::list_available_image_tags(); }
    fullstack_only!(backend::list_available_image_tags())
}

#[cfg_attr(not(feature = "demo"), get("/api/runtimes/proot/images/embedded-tag"))]
pub async fn get_embedded_image_tag() -> Result<Option<String>>  {
    #[cfg(feature = "demo")]
    { return crate::demo::get_embedded_image_tag(); }
    fullstack_only!(backend::get_embedded_image_tag())
}

#[cfg_attr(not(feature = "demo"), post("/api/repair-android-settings"))]
pub async fn repair_android_settings() -> Result<String>  {
    #[cfg(feature = "demo")]
    { return crate::demo::repair_android_settings(); }
    fullstack_only!(backend::repair_android_settings())
}

// ---------------------------------------------------------------------------
// Configuration schema
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "demo"), post("/api/config"))]
pub async fn get_image_config(image_tag: String) -> Result<ConfigSchema>  {
    #[cfg(feature = "demo")]
    { return crate::demo::get_image_config(image_tag); }
    fullstack_only!(backend::get_image_config(image_tag))
}

#[cfg_attr(not(feature = "demo"), get("/api/projects/{project_name}/config"))]
pub async fn load_project_config(project_name: String) -> Result<LoadedProjectConfig>  {
    #[cfg(feature = "demo")]
    { return crate::demo::load_project_config(project_name); }
    fullstack_only!(backend::load_named_project_config(project_name))
}

#[cfg_attr(not(feature = "demo"), post("/api/projects/{project_name}/config"))]
pub async fn save_project_config(project_name: String, config: SavedProjectConfig) -> Result<()>  {
    #[cfg(feature = "demo")]
    { return crate::demo::save_project_config(project_name, config); }
    fullstack_only!(backend::save_named_project_config(project_name, config))
}

// ---------------------------------------------------------------------------
// Task management
// ---------------------------------------------------------------------------

#[cfg_attr(
    not(feature = "demo"),
    get("/api/tasks?kind_filter&context_key_filter")
)]
pub async fn list_tasks(
    kind_filter: Option<String>,
    context_key_filter: Option<String>,
) -> Result<Vec<TaskInfo>> {
    #[cfg(feature = "demo")]
    { return crate::demo::list_tasks(kind_filter, context_key_filter); }
    let kind = kind_filter.and_then(|s| match s.as_str() {
        "PrepareImage" => Some(colmap_openmvs_api::TaskKind::PrepareImage),
        "DownloadDemo" => Some(colmap_openmvs_api::TaskKind::DownloadDemo),
        "BatchResize" => Some(colmap_openmvs_api::TaskKind::BatchResize),
        "RunPipeline" => Some(colmap_openmvs_api::TaskKind::RunPipeline),
        "DryRunPipeline" => Some(colmap_openmvs_api::TaskKind::DryRunPipeline),
        _ => None,
    });
    fullstack_only!(backend::list_tasks(kind, context_key_filter))
}

#[cfg_attr(not(feature = "demo"), get("/api/tasks/{task_id}"))]
pub async fn get_task_info(task_id: String) -> Result<Option<TaskInfo>>  {
    #[cfg(feature = "demo")]
    { return crate::demo::get_task_info(task_id); }
    fullstack_only!(backend::get_task_info(task_id))
}

#[cfg_attr(not(feature = "demo"), get("/api/tasks/{task_id}/poll?cursor"))]
pub async fn poll_task_events(
    task_id: String,
    cursor: usize,
) -> Result<colmap_openmvs_api::TaskEventBatch>  {
    #[cfg(feature = "demo")]
    { return crate::demo::poll_task_events(task_id, cursor); }
    fullstack_only!(backend::poll_task_events(task_id, cursor))
}

#[cfg_attr(not(feature = "demo"), delete("/api/tasks/{task_id}"))]
pub async fn cancel_task(task_id: String) -> Result<()>  {
    #[cfg(feature = "demo")]
    { return crate::demo::cancel_task(task_id); }
    fullstack_only!(backend::cancel_task(task_id))
}

// ---------------------------------------------------------------------------
// Pipeline execution
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "demo"), post("/api/projects/{project_name}/pipeline"))]
pub async fn run_pipeline(project_name: String, dry_run: bool) -> Result<String>  {
    #[cfg(feature = "demo")]
    { return crate::demo::run_pipeline(project_name, dry_run); }
    fullstack_only!(backend::run_pipeline(project_name, dry_run))
}

#[cfg_attr(not(feature = "demo"), get("/api/projects/{project_name}/run-status"))]
pub async fn get_project_run_status(project_name: String) -> Result<ProjectRunStatus>  {
    #[cfg(feature = "demo")]
    { return crate::demo::get_project_run_status(project_name); }
    fullstack_only!(backend::get_project_run_status(project_name))
}

// ---------------------------------------------------------------------------
// Docker runtime
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "demo"), get("/api/runtimes/docker/info"))]
pub async fn get_docker_runtime_info() -> Result<RuntimeInfo>  {
    #[cfg(feature = "demo")]
    { return crate::demo::get_docker_runtime_info(); }
    fullstack_only!(backend::get_docker_runtime_info())
}

#[cfg_attr(not(feature = "demo"), get("/api/runtimes/docker/images"))]
pub async fn list_docker_images() -> Result<Vec<PreparedImageInfo>>  {
    #[cfg(feature = "demo")]
    { return crate::demo::list_docker_images(); }
    fullstack_only!(backend::list_docker_images())
}

#[cfg_attr(not(feature = "demo"), post("/api/runtimes/docker/images/prepare"))]
pub async fn prepare_docker_image(image: String) -> Result<String>  {
    #[cfg(feature = "demo")]
    { return crate::demo::prepare_docker_image(image); }
    fullstack_only!(backend::prepare_docker_image(image))
}

#[cfg_attr(not(feature = "demo"), delete("/api/runtimes/docker/images/remove"))]
pub async fn remove_docker_image(image_tag: String) -> Result<()>  {
    #[cfg(feature = "demo")]
    { return crate::demo::remove_docker_image(image_tag); }
    fullstack_only!(backend::remove_docker_image(image_tag))
}

// ---------------------------------------------------------------------------
// Project outputs
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "demo"), get("/api/projects/{project_name}/outputs"))]
pub async fn list_project_outputs(project_name: String) -> Result<Vec<OutputFile>>  {
    #[cfg(feature = "demo")]
    { return crate::demo::list_project_outputs(project_name); }
    fullstack_only!(backend::list_project_outputs(project_name))
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
) -> Result<Vec<u8>>  {
    #[cfg(feature = "demo")]
    { return crate::demo::get_project_output_bytes(project_name, relative_path); }
    fullstack_only!(backend::get_project_output_bytes(
        project_name,
        relative_path
    ))
}

/// Return an output file in a viewer-friendly PLY format.
/// For PLY files this is a pass-through; for points3D.bin it converts to ASCII PLY.
#[cfg_attr(
    not(feature = "demo"),
    get("/api/projects/{project_name}/outputs/view?relative_path")
)]
pub async fn get_project_output_for_viewer(
    project_name: String,
    relative_path: String,
) -> Result<Vec<u8>>  {
    #[cfg(feature = "demo")]
    { return crate::demo::get_project_output_for_viewer(project_name, relative_path); }
    fullstack_only!(backend::get_project_output_for_viewer(
        project_name,
        relative_path
    ))
}

/// Delete an output file or directory.
#[cfg_attr(
    not(feature = "demo"),
    post("/api/projects/{project_name}/outputs/delete")
)]
pub async fn delete_project_output(project_name: String, relative_path: String) -> Result<()>  {
    #[cfg(feature = "demo")]
    { return crate::demo::delete_project_output(project_name, relative_path); }
    fullstack_only!(backend::delete_project_output(project_name, relative_path))
}

/// Delete all output files/directories, preserving only `images/` and `config.sh`.
#[cfg_attr(
    not(feature = "demo"),
    post("/api/projects/{project_name}/outputs/clear")
)]
pub async fn clear_project_outputs(project_name: String) -> Result<()>  {
    #[cfg(feature = "demo")]
    { return crate::demo::clear_project_outputs(project_name); }
    fullstack_only!(backend::clear_project_outputs(project_name))
}

// ---------------------------------------------------------------------------
// Native file-picker dialogs (backend-driven, works on all platforms)
// ---------------------------------------------------------------------------

/// Open a native file-picker on the default and import the chosen image files
/// into the project.  Returns the file names of successfully imported images,
/// or an empty list if the user cancelled.
#[cfg_attr(
    not(feature = "demo"),
    post("/api/projects/{project_name}/images/pick")
)]
pub async fn pick_and_import_images(project_name: String) -> Result<Vec<String>>  {
    #[cfg(feature = "demo")]
    { return crate::demo::pick_and_import_images(project_name); }
    fullstack_only!(backend::pick_and_import_images(project_name))
}

/// Open a native folder-picker on the default and return the chosen path.
/// Used by the General settings tab to set the projects folder.
/// Returns an error on Android (path management is automatic there).
#[cfg_attr(not(feature = "demo"), post("/api/settings/pick-folder"))]
pub async fn pick_projects_folder() -> Result<String>  {
    #[cfg(feature = "demo")]
    { return crate::demo::pick_projects_folder(); }
    fullstack_only!(backend::pick_projects_folder())
}

/// Open a native file-picker on the default for a JSON settings file and return
/// the chosen path.  Returns an error on Android.
#[cfg_attr(not(feature = "demo"), post("/api/settings/pick-file"))]
pub async fn pick_settings_file() -> Result<String>  {
    #[cfg(feature = "demo")]
    { return crate::demo::pick_settings_file(); }
    fullstack_only!(backend::pick_settings_file())
}

/// Save an output file to disk.  On desktop opens a save-dialog; on Android
/// writes directly to `/sdcard/Download/`.  Returns a human-readable
/// confirmation/success message.
#[cfg_attr(
    not(feature = "demo"),
    post("/api/projects/{project_name}/outputs/save-as")
)]
pub async fn save_output_as(project_name: String, relative_path: String) -> Result<String>  {
    #[cfg(feature = "demo")]
    { return crate::demo::save_output_as(project_name, relative_path); }
    fullstack_only!(backend::save_output_as(project_name, relative_path))
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
pub async fn get_dark_mode() -> Result<Option<bool>>  {
    #[cfg(feature = "demo")]
    { return crate::demo::get_dark_mode(); }
    fullstack_only!(backend::get_dark_mode())
}
