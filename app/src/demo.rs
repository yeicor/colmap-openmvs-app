use colmap_openmvs_api::{
    ConfigSchema, ImageTagInfo, LoadedProjectConfig, OutputFile, PreparedImageInfo, Project,
    ProjectRunStatus, RuntimeInfo, Settings, TaskEventBatch, TaskInfo,
};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

include!(concat!(env!("OUT_DIR"), "/demo_assets.rs"));

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

pub fn startup() -> Result<()> {
    Ok(())
}

pub fn get_projects() -> Result<Vec<Project>> {
    Ok(get_manifest().projects)
}

pub fn get_settings() -> Result<Settings> {
    Ok(get_manifest().settings)
}

pub fn get_project_images(_project_name: String) -> Result<Vec<String>> {
    Ok(get_manifest().project.images)
}

pub fn get_project_image_bytes(_project_name: String, image_name: String) -> Result<Vec<u8>> {
    match demo_image_bytes(&image_name) {
        Some(bytes) => Ok(bytes.to_vec()),
        None => Err(dioxus::CapturedError::msg("Image not found in demo data")),
    }
}

pub fn get_runtime_info() -> Result<RuntimeInfo> {
    Ok(get_manifest().runtime_info.clone())
}

pub fn get_docker_runtime_info() -> Result<RuntimeInfo> {
    Ok(get_manifest().runtime_info)
}

pub fn get_available_runtime_versions() -> Result<Vec<String>> {
    Ok(vec![])
}

pub fn list_runtime_images() -> Result<Vec<PreparedImageInfo>> {
    Ok(vec![])
}

pub fn list_docker_images() -> Result<Vec<PreparedImageInfo>> {
    Ok(vec![])
}

pub fn list_available_image_tags() -> Result<Vec<ImageTagInfo>> {
    Ok(vec![])
}

pub fn get_embedded_image_tag() -> Result<Option<String>> {
    Ok(None)
}

pub fn get_image_config(_image_tag: String) -> Result<ConfigSchema> {
    Ok(get_manifest().project.config_schema)
}

pub fn load_project_config(_project_name: String) -> Result<LoadedProjectConfig> {
    Ok(get_manifest().project.project_config)
}

pub fn list_tasks(
    _kind_filter: Option<String>,
    _context_key_filter: Option<String>,
) -> Result<Vec<TaskInfo>> {
    Ok(vec![])
}

pub fn get_task_info(_task_id: String) -> Result<Option<TaskInfo>> {
    Ok(None)
}

pub fn poll_task_events(_task_id: String, cursor: usize) -> Result<TaskEventBatch> {
    Ok(TaskEventBatch {
        events: vec![],
        cursor,
        is_terminal: true,
        task_found: false,
    })
}

pub fn get_project_run_status(_project_name: String) -> Result<ProjectRunStatus> {
    Ok(get_manifest().project.run_status)
}

pub fn list_project_outputs(_project_name: String) -> Result<Vec<OutputFile>> {
    Ok(get_manifest().project.outputs)
}

pub fn get_project_output_bytes(_project_name: String, relative_path: String) -> Result<Vec<u8>> {
    match demo_output_bytes(&relative_path) {
        Some(bytes) => Ok(bytes.to_vec()),
        None => Err(dioxus::CapturedError::msg("Output not found in demo data")),
    }
}

pub fn get_project_output_for_viewer(project_name: String, relative_path: String) -> Result<Vec<u8>> {
    get_project_output_bytes(project_name, relative_path)
}

pub fn get_dark_mode() -> Result<Option<bool>> {
    Ok(get_manifest().dark_mode)
}

pub fn create_project(_name: String) -> Result<Project> {
    read_only_error()
}
pub fn delete_project(_name: String) -> Result<()> {
    read_only_error()
}
pub fn rename_project(_name: String, _new_name: String) -> Result<Project> {
    read_only_error()
}
pub fn update_settings(_new_settings: Settings) -> Result<()> {
    read_only_error()
}
#[cfg(not(feature = "fullstack"))]
type ByteStream = Vec<u8>;
#[cfg(feature = "fullstack")]
use dioxus::fullstack::ByteStream;
pub fn add_project_image(_project_name: String, _image_name: String, _body: ByteStream) -> Result<()> {
    read_only_error()
}
pub fn delete_project_image(_project_name: String, _image_name: String) -> Result<()> {
    read_only_error()
}
pub fn clear_project_images(_project_name: String) -> Result<()> {
    read_only_error()
}
pub fn batch_resize_images(_project_name: String, _max_dimension: u32) -> Result<String> {
    read_only_error()
}
pub fn download_demo_images(_project_name: String, _source_id: String) -> Result<String> {
    read_only_error()
}
pub fn download_runtime_version(_version: String) -> Result<()> {
    read_only_error()
}
pub fn delete_runtime_binary() -> Result<()> {
    read_only_error()
}
pub fn prepare_runtime_image(_image: String) -> Result<String> {
    read_only_error()
}
pub fn remove_runtime_image(_image_tag: String) -> Result<()> {
    read_only_error()
}
pub fn repair_android_settings() -> Result<String> {
    read_only_error()
}
pub fn save_project_config(_project_name: String, _config: colmap_openmvs_api::SavedProjectConfig) -> Result<()> {
    read_only_error()
}
pub fn cancel_task(_task_id: String) -> Result<()> {
    read_only_error()
}
pub fn run_pipeline(_project_name: String, _dry_run: bool) -> Result<String> {
    read_only_error()
}
pub fn prepare_docker_image(_image: String) -> Result<String> {
    read_only_error()
}
pub fn remove_docker_image(_image_tag: String) -> Result<()> {
    read_only_error()
}
pub fn delete_project_output(_project_name: String, _relative_path: String) -> Result<()> {
    read_only_error()
}
pub fn clear_project_outputs(_project_name: String) -> Result<()> {
    read_only_error()
}
pub fn pick_and_import_images(_project_name: String) -> Result<Vec<String>> {
    read_only_error()
}
pub fn pick_projects_folder() -> Result<String> {
    read_only_error()
}
pub fn pick_settings_file() -> Result<String> {
    read_only_error()
}
pub fn save_output_as(_project_name: String, _relative_path: String) -> Result<String> {
    read_only_error()
}
