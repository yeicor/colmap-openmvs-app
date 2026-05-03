mod proot;
#[cfg(test)]
mod tests;

pub use proot::{PRoot, PrepareProgress};

use std::path::PathBuf;
use thiserror::Error;

/// Runtime system errors
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Runtime not supported: {0}")]
    NotSupported(String),

    #[error("Runtime not found: {0}")]
    NotFound(String),

    #[error("Failed to determine version: {0}")]
    VersionError(String),

    #[error("Download error: {0}")]
    DownloadError(String),

    #[error("Image preparation failed: {0}")]
    PrepareError(String),

    #[error("Execution error: {0}")]
    ExecutionError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::error::Error),

    #[error("Command execution failed: {0}")]
    CommandError(String),

    #[error("Platform error: {0}")]
    PlatformError(String),

    #[error("Invalid version: {0}")]
    InvalidVersion(String),
}

pub type RuntimeResult<T> = Result<T, RuntimeError>;

/// Process handle for lifecycle management
pub struct ProcessHandle {
    pub child: std::process::Child,
}

impl ProcessHandle {
    /// Wait for process to complete
    pub fn wait(mut self) -> RuntimeResult<std::process::ExitStatus> {
        self.child
            .wait()
            .map_err(|e| RuntimeError::ExecutionError(format!("Failed to wait for process: {}", e)))
    }

    /// Get mutable references to stdin/stdout/stderr
    pub fn stdin(&mut self) -> Option<&mut std::process::ChildStdin> {
        self.child.stdin.as_mut()
    }

    pub fn stdout(&mut self) -> Option<&mut std::process::ChildStdout> {
        self.child.stdout.as_mut()
    }

    pub fn stderr(&mut self) -> Option<&mut std::process::ChildStderr> {
        self.child.stderr.as_mut()
    }

    /// Kill the process
    pub fn kill(mut self) -> RuntimeResult<()> {
        self.child
            .kill()
            .map_err(|e| RuntimeError::ExecutionError(format!("Failed to kill process: {}", e)))
    }
}

/// Main Runtime enum for different containerization approaches
#[derive(Debug, Clone)]
pub enum Runtime {
    PRoot(PRoot),
}

impl Runtime {
    /// Create a new PRoot runtime with default install directory
    pub fn proot() -> Self {
        #[cfg(target_os = "android")]
        let default_dir =
            PathBuf::from("/data/data/com.github.yeicor.colmap_openmvs_app/files/runtimes/proot");
        #[cfg(not(target_os = "android"))]
        let default_dir = PathBuf::from("./runtimes/proot");
        Runtime::PRoot(PRoot::new(default_dir))
    }

    /// Create a new PRoot runtime with custom install directory
    pub fn proot_with_dir(install_dir: PathBuf) -> Self {
        Runtime::PRoot(PRoot::new(install_dir))
    }

    /// Check if the runtime is supported on this system
    pub fn is_supported(&self) -> RuntimeResult<()> {
        match self {
            Runtime::PRoot(proot) => proot.is_supported(),
        }
    }

    /// Get the version of the installed runtime
    pub async fn version(&self) -> RuntimeResult<String> {
        match self {
            Runtime::PRoot(proot) => proot.version().await,
        }
    }

    /// Get available versions for download
    pub async fn available_versions(&self) -> RuntimeResult<Vec<String>> {
        match self {
            Runtime::PRoot(proot) => proot.available_versions().await,
        }
    }

    /// Download and install a specific version
    pub async fn download(&self, version: &str) -> RuntimeResult<()> {
        match self {
            Runtime::PRoot(proot) => proot.download(version).await,
        }
    }

    /// Prepare a Docker image for execution
    pub async fn prepare(
        &self,
        image: &str,
        progress: impl Fn(PrepareProgress) + Send + Sync,
    ) -> RuntimeResult<()> {
        match self {
            Runtime::PRoot(proot) => proot.prepare(image, progress).await,
        }
    }

    /// Remove a prepared image
    pub async fn remove(&self, image: &str) -> RuntimeResult<()> {
        match self {
            Runtime::PRoot(proot) => proot.remove(image).await,
        }
    }

    /// Execute a prepared image with given arguments
    pub async fn run(&self, image: &str, args: &[String]) -> RuntimeResult<ProcessHandle> {
        match self {
            Runtime::PRoot(proot) => proot.run(image, args).await,
        }
    }

    /// Get the install directory
    pub fn install_dir(&self) -> &PathBuf {
        match self {
            Runtime::PRoot(proot) => &proot.install_dir,
        }
    }
}
