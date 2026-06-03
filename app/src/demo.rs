use colmap_openmvs_api::{
    ConfigSchema, ImageTagInfo, LoadedProjectConfig, OutputFile, PreparedImageInfo, Project,
    ProjectRunStatus, RuntimeInfo, Settings, TaskEvent, TaskEventBatch, TaskInfo,
};
use dioxus::Result;

use dioxus::fullstack::ByteStream;

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/demo_assets.rs"));

fn get_download_events() -> &'static Vec<TaskEvent> {
    static DOWNLOAD_EVENTS: OnceLock<Vec<TaskEvent>> = OnceLock::new();
    DOWNLOAD_EVENTS.get_or_init(|| {
        serde_json::from_str(DOWNLOAD_EVENTS_JSON).expect("Failed to parse DOWNLOAD_EVENTS_JSON")
    })
}

fn get_pipeline_events() -> &'static Vec<TaskEvent> {
    static PIPELINE_EVENTS: OnceLock<Vec<TaskEvent>> = OnceLock::new();
    PIPELINE_EVENTS.get_or_init(|| {
        serde_json::from_str(PIPELINE_EVENTS_JSON).expect("Failed to parse PIPELINE_EVENTS_JSON")
    })
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DemoManifest {
    pub projects: Vec<Project>,
    pub settings: Settings,
    pub dark_mode: Option<bool>,
    pub project: DemoProject,
    pub runtime_info: RuntimeInfo,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DemoProject {
    pub images: Vec<String>,
    pub config_schema: ConfigSchema,
    pub project_config: LoadedProjectConfig,
    pub outputs: Vec<OutputFile>,
    pub run_status: ProjectRunStatus,
}

fn get_manifest() -> DemoManifest {
    serde_json::from_str(DEMO_MANIFEST).expect("Failed to parse DEMO_MANIFEST")
}

pub fn read_only_error<T>() -> Result<T> {
    Err(dioxus::CapturedError::msg(
        "This is a read-only demo. Download the full version to manage projects and run pipelines.",
    ))
}

pub async fn on_frontend_started() -> Result<()> {
    Ok(())
}

pub async fn get_projects() -> Result<Vec<Project>> {
    Ok(get_manifest().projects)
}

pub async fn get_settings() -> Result<Settings> {
    Ok(get_manifest().settings)
}

pub async fn get_project_images(_project_name: String) -> Result<Vec<String>> {
    Ok(get_manifest().project.images)
}

pub async fn get_project_image_bytes(
    _project_name: String,
    image_name: String,
) -> Result<ByteStream> {
    match demo_image_bytes(image_name.as_str()) {
        Some(bytes) => Ok(ByteStream::new(
            futures::stream::once(async move {
                dioxus::fullstack::body::Bytes::from(bytes.to_vec())
            }),
        )),
        None => Err(dioxus::CapturedError::msg("Image not found in demo data")),
    }
}

pub async fn get_runtime_info() -> Result<RuntimeInfo> {
    Ok(get_manifest().runtime_info.clone())
}

pub async fn get_docker_runtime_info() -> Result<RuntimeInfo> {
    Ok(get_manifest().runtime_info)
}

pub async fn get_available_runtime_versions() -> Result<Vec<String>> {
    Ok(vec![])
}

pub async fn list_runtime_images() -> Result<Vec<PreparedImageInfo>> {
    Ok(vec![])
}

pub async fn list_docker_images() -> Result<Vec<PreparedImageInfo>> {
    Ok(vec![])
}

pub async fn list_available_image_tags() -> Result<Vec<ImageTagInfo>> {
    Ok(vec![])
}

pub async fn get_embedded_image_tag() -> Result<Option<String>> {
    Ok(None)
}

pub async fn get_image_config(_image_tag: String) -> Result<ConfigSchema> {
    Ok(get_manifest().project.config_schema)
}

pub async fn load_project_config(_project_name: String) -> Result<LoadedProjectConfig> {
    Ok(get_manifest().project.project_config)
}

pub async fn list_tasks(
    _kind_filter: Option<colmap_openmvs_api::TaskKind>,
    _context_key_filter: Option<String>,
) -> Result<Vec<TaskInfo>> {
    Ok(vec![])
}

pub async fn get_task_info(_task_id: String) -> Result<Option<TaskInfo>> {
    Ok(None)
}

pub async fn poll_task_events(task_id: String, cursor: usize) -> Result<TaskEventBatch> {
    let events = match task_id.as_str() {
        "demo-download-task" => get_download_events(),
        "demo-pipeline-task" => get_pipeline_events(),
        "demo-pipeline-dry-run" => get_pipeline_events(),
        _ => {
            return Ok(TaskEventBatch {
                events: vec![],
                cursor,
                is_terminal: true,
                task_found: false,
            });
        }
    };

    let chunk_size = if task_id == "demo-pipeline-dry-run" {
        events.len()
    } else {
        10usize
    };
    let new_events: Vec<TaskEvent> =
        events.iter().skip(cursor).take(chunk_size).cloned().collect();
    let new_cursor = cursor + new_events.len();
    let is_terminal = new_cursor >= events.len()
        || new_events
            .iter()
            .any(|e| matches!(e, TaskEvent::Completed | TaskEvent::Failed(_)));

    Ok(TaskEventBatch {
        events: new_events,
        cursor: new_cursor,
        is_terminal,
        task_found: true,
    })
}

pub async fn get_project_run_status(_project_name: String) -> Result<ProjectRunStatus> {
    Ok(get_manifest().project.run_status)
}

pub async fn list_project_outputs(_project_name: String) -> Result<Vec<OutputFile>> {
    Ok(get_manifest().project.outputs)
}

pub async fn get_project_output_bytes(
    _project_name: String,
    relative_path: String,
) -> Result<ByteStream> {
    match demo_output_bytes(&relative_path) {
        Some(bytes) => Ok(ByteStream::new(
            futures::stream::once(async move { dioxus::fullstack::body::Bytes::from(bytes.to_vec()) }),
        )),
        None => Err(dioxus::CapturedError::msg("Output not found in demo data")),
    }
}

pub async fn get_dark_mode() -> Result<Option<bool>> {
    Ok(get_manifest().dark_mode)
}

pub async fn create_project(_name: String) -> Result<Project> {
    read_only_error()
}
pub async fn delete_project(_name: String) -> Result<()> {
    read_only_error()
}
pub async fn rename_project(_name: String, _new_name: String) -> Result<Project> {
    read_only_error()
}
pub async fn update_settings(_new_settings: Settings) -> Result<()> {
    read_only_error()
}
pub async fn add_project_image(
    _project_name: String,
    _image_name: String,
    _body: ByteStream,
) -> Result<()> {
    read_only_error()
}
pub async fn delete_project_image(_project_name: String, _image_name: String) -> Result<()> {
    read_only_error()
}
pub async fn clear_project_images(_project_name: String) -> Result<()> {
    read_only_error()
}
pub async fn batch_resize_images(_project_name: String, _max_dimension: u32) -> Result<String> {
    read_only_error()
}
pub async fn download_demo_images(_project_name: String, _source_id: String) -> Result<String> {
    Ok("demo-download-task".to_string())
}
pub async fn download_runtime_version(_version: String) -> Result<()> {
    read_only_error()
}
pub async fn delete_runtime_binary() -> Result<()> {
    read_only_error()
}
pub async fn prepare_runtime_image(_image: String) -> Result<String> {
    read_only_error()
}
pub async fn remove_runtime_image(_image_tag: String) -> Result<()> {
    read_only_error()
}
pub async fn repair_android_settings() -> Result<String> {
    read_only_error()
}
pub async fn save_project_config(
    _project_name: String,
    _config: colmap_openmvs_api::SavedProjectConfig,
) -> Result<()> {
    read_only_error()
}
pub async fn cancel_task(_task_id: String) -> Result<()> {
    read_only_error()
}
pub async fn run_pipeline(_project_name: String, dry_run: bool) -> Result<String> {
    if dry_run {
        Ok("demo-pipeline-dry-run".to_string())
    } else {
        Ok("demo-pipeline-task".to_string())
    }
}
pub async fn prepare_docker_image(_image: String) -> Result<String> {
    read_only_error()
}
pub async fn remove_docker_image(_image_tag: String) -> Result<()> {
    read_only_error()
}
pub async fn delete_project_output(_project_name: String, _relative_path: String) -> Result<()> {
    read_only_error()
}
pub async fn clear_project_outputs(_project_name: String) -> Result<()> {
    read_only_error()
}
pub async fn pick_and_import_images(_project_name: String) -> Result<Vec<String>> {
    read_only_error()
}
pub async fn pick_projects_folder() -> Result<String> {
    read_only_error()
}
pub async fn pick_settings_file() -> Result<String> {
    read_only_error()
}
pub async fn save_output_as(_project_name: String, _relative_path: String) -> Result<String> {
    read_only_error()
}
