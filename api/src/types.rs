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
