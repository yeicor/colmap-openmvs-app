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
        last_file: Option<String>, // None on 0% progress, Some(filename) on progress updates
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
