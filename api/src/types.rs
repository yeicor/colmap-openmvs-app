//! Re-export types for backend - these are defined in the server package
//! but used by both server (with macros) and backend (implementations)

use serde::{Deserialize, Serialize};

/// Project information
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub name: String,
    pub path: String,
}

/// Application settings
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    pub projects_folder: String,
    /// The default container image tag to use for all runtime commands (e.g. running --help in Config tab).
    /// Example: "mirror.gcr.io/yeicor/colmap-openmvs:latest"
    pub default_image_tag: Option<String>,
}

/// Events emitted during demo image download
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum DemoProgressEvent {
    DownloadProgress {
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    ExtractionProgress {
        last_file: Option<String>,
        total_files: usize,
        total_bytes: u64,
    },
    Error {
        message: String,
    },
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

/// A configuration parameter parsed from tool help output
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigParameter {
    /// Full parameter name including prefix (e.g., "--focal-length" or "--Mapper.min_num_matches")
    pub name: String,
    /// Parameter description/help text
    pub description: String,
    /// Default value if specified in help
    pub default_value: Option<String>,
    /// Possible enum values if the parameter is an enum type
    pub enum_values: Vec<String>,
}

/// Configuration for a specific tool extracted from its help output
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolConfig {
    /// Tool name (e.g., "colmap", "openmvs")
    pub tool: String,
    /// Sub-command name (e.g., "mapper", "feature_extractor")
    pub command: String,
    /// All configuration parameters for this command
    pub parameters: Vec<ConfigParameter>,
    /// Environment variables that affect this command
    pub environment_variables: Vec<String>,
}

/// Complete configuration schema for all tools in an image
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageConfigSchema {
    /// Image tag this schema is from
    pub image_tag: String,
    /// Build date of the image
    pub build_date: Option<String>,
    /// All tools and their configurations
    pub tools: Vec<ToolConfig>,
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
    /// All tools and their configurations
    pub tools: Vec<ToolConfig>,
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
    DryRunPipeline,
}

/// Status of a pipeline stage as reported by the `::group` marker
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PipelineStageStatus {
    /// Stage output was already cached from a previous run
    Cached,
    /// Stage needs to run (or was skipped in dry-run mode)
    Run,
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
    /// A log line from the pipeline
    PipelineLog {
        /// Pipeline stage index (0-based)
        stage_index: u32,
        /// Stage name
        stage_name: String,
        /// The raw log line
        line: String,
    },
    /// The list of all expected stage names, emitted once before any ::group markers
    PipelineRemainingGroups(Vec<String>),
    /// A stage started in the pipeline
    PipelineStageStarted {
        stage_index: u32,
        stage_name: String,
        total_stages: u32,
        /// Whether the stage was already cached (completed in a previous run)
        stage_status: PipelineStageStatus,
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
