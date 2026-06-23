//! Re-export types for backend - these are defined in the server package
//! but used by both server (with macros) and backend (implementations)

use serde::{Deserialize, Serialize};

/// Project information
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub name: String,
    pub path: String,
}

/// List of project images and metadata about video frames
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectImages {
    /// Regular (non-frame) image file names, sorted
    pub images: Vec<String>,
    /// Number of auto-generated frame images currently in the project
    /// (extracted from uploaded videos, not shown in the gallery)
    pub frame_count: usize,
}

/// Application settings
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    pub projects_folder: String,
    /// Directory containing the PRoot binary and supporting libraries
    pub proot_binary_dir: String,
    /// Directory containing large PRoot runtime images
    pub proot_images_dir: String,
    /// The default container image tag to use for all runtime commands.
    /// Format: "runtime:tag" where runtime is "proot" or "docker"
    /// Example: "proot:mirror.gcr.io/yeicor/colmap-openmvs:latest"
    /// or "docker:mirror.gcr.io/yeicor/colmap-openmvs:latest"
    #[serde(default)]
    pub default_image_tag: Option<String>,
    /// Custom mounts for all runtimes (for CUDA, debuggers, etc.)
    /// Format: "host_path:container_path" or "host_path" (defaults to same path in container)
    /// Example: "/usr/lib/x86_64-linux-gnu/libcuda.so.1:/usr/lib/x86_64-linux-gnu/libcuda.so.1"
    #[serde(default)]
    pub custom_mounts: Vec<String>,
    /// Path to the settings.json file. Can be overridden via COLMAPOPENMVSAPP_SETTINGS_PATH environment variable.
    /// Defaults to projects_folder/settings.json if not specified.
    #[serde(default)]
    pub settings_file_path: Option<String>,

    /// Force a specific color-scheme theme, overriding the system / server preference.
    ///
    /// * `None` / empty   — use the default (server-detected or CSS media query).
    /// * `Some("light")`  — force light theme.
    /// * `Some("dark")`   — force dark theme.
    ///
    /// Requires an app restart to take full effect on the server-side
    /// (the frontend reads this at startup from `get_dark_mode`).
    #[serde(default)]
    pub theme_override: Option<String>,
}

impl Settings {
    /// Parse the runtime and image tag from the default_image_tag setting.
    /// Returns (runtime, tag) or (None, None) if not set.
    pub fn parse_default_image(&self) -> (Option<&str>, Option<&str>) {
        match &self.default_image_tag {
            Some(s) => match s.split_once(':') {
                Some((runtime, tag)) => (Some(runtime), Some(tag)),
                None => (None, Some(s.as_str())),
            },
            None => (None, None),
        }
    }

    /// Set the default image tag with a runtime prefix.
    pub fn set_default_image(&mut self, runtime: &str, tag: &str) {
        self.default_image_tag = Some(format!("{}:{}", runtime, tag));
    }

    /// Clear the default image tag.
    pub fn clear_default_image(&mut self) {
        self.default_image_tag = None;
    }
}

/// Events emitted during demo image download
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum DemoProgressEvent {
    /// Fetching file list from GitHub API
    FetchingFileList,
    /// Downloading an individual file
    DownloadProgress {
        filename: String,
        downloaded: usize,
        total: usize,
    },
    /// An error occurred
    Error { message: String },
}

/// Events emitted during image batch resize
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ResizeProgressEvent {
    ResizeProgress {
        name: String,
        completed: usize,
        total_files: usize,
    },
    Error {
        message: String,
    },
}

/// Events emitted during ZIP download of project outputs
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ZipProgressEvent {
    Packing {
        completed: usize,
        total: usize,
        current_file: String,
    },
    Error {
        message: String,
    },
}

/// Events emitted during ZIP restore of project outputs
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum RestoreProgressEvent {
    Extracting {
        completed: usize,
        total: usize,
        current_file: String,
    },
    Error {
        message: String,
    },
}

/// Status information about the PRoot container runtime
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeInfo {
    pub name: String,
    pub supported: bool,
    pub unsupported_reason: Option<String>,
    pub installed: bool,
    pub version: Option<String>,
}

/// A prepared container image ready to run
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PreparedImageInfo {
    pub tag: String,
    pub hash: String,
    pub size: u64,
    pub size_readable: String,
    #[serde(default)]
    pub build_date: Option<String>,
}

/// Available image tag with metadata
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageTagInfo {
    pub name: String,
    pub build_date: Option<String>,
}

/// Progress events during container image preparation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PrepareProgress {
    Downloading {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    ExtractingLayer {
        layer: String,
        progress: f32,
    },
    Error {
        message: String,
    },
}

/// User-configured environment variable for saving to config.sh
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnvVarConfig {
    /// Environment variable name (e.g., "COLMAP_NUM_THREADS")
    pub name: String,
    /// User-provided value for this environment variable
    pub value: String,
}

/// Environment variable with optional help text for UI display
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnvVarWithHelp {
    /// Environment variable name (e.g., "COLMAP_NUM_THREADS")
    pub name: String,
    /// Optional help text describing what this variable does
    pub help: Option<String>,
}

/// Complete configuration schema with environment variables for UI
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigSchema {
    /// Image tag this schema is from
    pub image_tag: String,
    /// Build date of the image
    pub build_date: Option<String>,
    /// Top-level environment variables (in order) with optional help text
    pub environment_variables: Vec<EnvVarWithHelp>,
}

/// Configuration to be saved to config.sh in a project
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedProjectConfig {
    /// Image tag used for this configuration
    pub image_tag: String,
    /// All configured environment variables
    pub environment_variables: Vec<EnvVarConfig>,
    /// Custom script content to append to config.sh (optional)
    pub custom_script: Option<String>,
}

/// Configuration loaded from config.sh in a project
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LoadedProjectConfig {
    /// Image tag this configuration was generated from
    pub image_tag: String,
    /// Environment variables parsed from config.sh
    pub environment_variables: Vec<EnvVarConfig>,
    /// Custom script content found after environment variables
    pub custom_script: String,
}

/// Output file in a project's work directory
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutputFile {
    /// Relative path from project root (e.g. "openmvs/scene_mesh.ply")
    pub relative_path: String,
    /// File name only
    pub name: String,
    /// File size in bytes
    pub size: u64,
    /// Whether this file can be displayed in the 3D viewer
    pub is_viewable: bool,
    /// Whether the file has a GLB version available for download/viewing
    #[serde(default)]
    pub glb_available: bool,
    /// Last-modified Unix timestamp in milliseconds (0 if unavailable)
    #[serde(default)]
    pub modified_at: u64,
}

/// Format options for downloading an output file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum GlbFormat {
    #[serde(rename = "raw")]
    Raw,
    #[serde(rename = "glb")]
    Glb,
}

/// Unique task identifier (UUID v4 string)
pub type TaskId = String;

/// The kind of a background task
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TaskKind {
    PrepareImage,
    DownloadDemo,
    BatchResize,
    RunPipeline,
    RecoverPipelineLogs,
    AndroidSettingsRepair,
    /// Generic system startup — runs all registered startup steps.
    Startup,
    /// Zipping project outputs for download.
    ZipOutputs,
    /// Restoring project outputs from uploaded ZIP.
    RestoreOutputs,
}

/// Status of a pipeline stage as reported by the `::group` marker
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PipelineStageStatus {
    /// Stage output was already cached from a previous run
    Cached,
    /// Stage needs to run (or was skipped in recover-logs mode)
    Run,
    /// Stage was explicitly skipped via --skip flag
    Skipped,
}

/// The state of a task
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskState {
    Running,
    Completed,
    Failed(String),
}

/// Contextual key for a task (used for deduplication / lookup)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskContext {
    /// The human-readable task kind
    pub kind: TaskKind,
    /// A string key that identifies the context (e.g., image tag, project name)
    pub context_key: String,
}

/// Information about a registered background task
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskInfo {
    pub id: TaskId,
    pub kind: TaskKind,
    pub state: TaskState,
    pub context_key: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Unified event type for all task kinds, used in subscribe_task_events
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskEvent {
    /// Progress during image preparation
    PrepareProgress(PrepareProgress),
    /// Progress during demo image download
    DemoProgress(DemoProgressEvent),
    /// Progress during batch image resize
    ResizeProgress(ResizeProgressEvent),
    /// Progress during ZIP download of project outputs
    ZipProgress(ZipProgressEvent),
    /// Progress during ZIP restore of project outputs
    RestoreProgress(RestoreProgressEvent),
    /// A generic log message from a task
    Log(String),
    /// A log line from the pipeline
    PipelineLog {
        /// Pipeline stage index (0-based)
        stage_index: u32,
        /// Stage name
        stage_name: String,
        /// The raw log line
        line: String,
    },
    /// The list of (some of the) remaining stage names, emitted at any point during execution.
    /// May arrive after some stages have already started/completed, and may be emitted
    /// multiple times as more pipeline stages are discovered (pipelines are now swappable).
    /// In the future, this will exclude past and in-progress groups.
    PipelineRemainingGroups(Vec<String>),
    /// A stage started in the pipeline
    PipelineStageStarted {
        /// Sequential index into the full group list (0-based, includes Config/Tool Discovery)
        stage_index: u32,
        stage_name: String,
        /// Number of pipeline stages (excludes Config/Tool Discovery); 0 for non-pipeline groups
        total_stages: u32,
        /// Whether the stage was cached/skipped/needs-run
        stage_status: PipelineStageStatus,
        /// 1-based pipeline stage number from count=X/Y; None for Config/Tool Discovery groups
        pipeline_stage_num: Option<u32>,
    },
    /// Pipeline sub-progress within the current stage (0.0..=1.0)
    PipelineStageProgress { stage_index: u32, progress: f32 },
    /// A stage completed
    PipelineStageCompleted {
        stage_index: u32,
        stage_name: String,
    },
    /// The task completed successfully
    Completed,
    /// The task failed
    Failed(String),
}

/// Result of polling for task events since a given cursor position.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskEventBatch {
    /// New events since the given cursor
    pub events: Vec<TaskEvent>,
    /// Updated cursor to use in the next poll (equals old cursor + events.len())
    pub cursor: usize,
    /// True if a terminal event (Completed or Failed) is in this batch
    pub is_terminal: bool,
    /// False if the task ID was not found in the registry (evicted or never existed)
    pub task_found: bool,
}

/// Status of a running or recently completed pipeline for a project
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectRunStatus {
    /// Whether a pipeline is currently running for this project
    pub is_running: bool,
    /// Whether the active pipeline is a recoveer-logs-run (true) or actual run (false)
    pub is_recover_logs: bool,
    /// Current progress as a percentage (0.0..=1.0), None if not started or not available
    pub progress: Option<f32>,
    /// The task ID of the active/last pipeline run, empty string if no active task
    pub task_id: String,
}
