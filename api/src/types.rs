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
    ResolvingImage,
    Downloading {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    ExtractingLayer {
        layer: String,
        progress: f32,
    },
    WritingRootFs,
    Configuring,
    Completed,
    Error {
        message: String,
    },
}
