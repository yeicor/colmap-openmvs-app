use colmap_openmvs_api::PrepareProgress;
mod image_manager;
mod proot;
mod registry;

pub use image_manager::{ImageConfig, ImageDigestInfo, ImageManager};
pub use proot::PRoot;
pub use registry::{ImageDigest, ImageTag, RegistryClient, RemoteImage, UpdateInfo, Version};

use async_trait::async_trait;
use std::path::PathBuf;
use tokio::sync::mpsc;

pub type RuntimeResult<T> = anyhow::Result<T>;

// ---------------------------------------------------------------------------
// Async process handle
// ---------------------------------------------------------------------------

/// An async handle to a running container process.
///
/// Backed by [`tokio::process::Child`]; all blocking wait/kill operations are
/// properly async and will not stall the executor.
pub struct ProcessHandle {
    pub child: tokio::process::Child,
}

impl ProcessHandle {
    /// Wait asynchronously for the process to exit.
    pub async fn wait(&mut self) -> RuntimeResult<std::process::ExitStatus> {
        self.child
            .wait()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to wait for process: {}", e))
    }

    /// Return the OS process ID, if still alive.
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Borrow stdin for writing.
    pub fn stdin(&mut self) -> Option<&mut tokio::process::ChildStdin> {
        self.child.stdin.as_mut()
    }

    /// Borrow stdout for reading.
    pub fn stdout(&mut self) -> Option<&mut tokio::process::ChildStdout> {
        self.child.stdout.as_mut()
    }

    /// Borrow stderr for reading.
    pub fn stderr(&mut self) -> Option<&mut tokio::process::ChildStderr> {
        self.child.stderr.as_mut()
    }

    /// Take ownership of stdin (useful for passing to another task).
    pub fn take_stdin(&mut self) -> Option<tokio::process::ChildStdin> {
        self.child.stdin.take()
    }

    /// Take ownership of stdout.
    pub fn take_stdout(&mut self) -> Option<tokio::process::ChildStdout> {
        self.child.stdout.take()
    }

    /// Take ownership of stderr.
    pub fn take_stderr(&mut self) -> Option<tokio::process::ChildStderr> {
        self.child.stderr.take()
    }

    /// Kill the process asynchronously.
    pub async fn kill(&mut self) -> RuntimeResult<()> {
        self.child
            .kill()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to kill process: {}", e))
    }
}

// ---------------------------------------------------------------------------
// Progress channel helpers
// ---------------------------------------------------------------------------

/// Sender half of a prepare-progress channel.
pub type PrepareProgressTx = mpsc::Sender<PrepareProgress>;

/// Receiver half of a prepare-progress channel.
pub type PrepareProgressRx = mpsc::Receiver<PrepareProgress>;

/// Create a bounded channel for streaming [`PrepareProgress`] events from
/// [`Runtime::prepare`].  A buffer of 16 is sufficient for typical UI refresh
/// rates; callers may create their own channel with a different capacity.
pub fn prepare_progress_channel() -> (PrepareProgressTx, PrepareProgressRx) {
    mpsc::channel(16)
}

// ---------------------------------------------------------------------------
// Image types
// ---------------------------------------------------------------------------

/// Opaque hash that uniquely identifies a prepared image on disk.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ImageHash(String);

impl ImageHash {
    pub fn new(hash: impl Into<String>) -> Self {
        ImageHash(hash.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ImageHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A container image that has been downloaded and extracted, ready to run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PreparedImage {
    /// Full image tag (e.g. `"library/alpine:3.18"`).
    pub tag: ImageTag,
    /// Storage hash for this image.
    pub hash: ImageHash,
    /// On-disk size in bytes.
    pub size: u64,
    /// Build date in RFC3339 format (optional).
    #[serde(default)]
    pub build_date: Option<String>,
}

impl PreparedImage {
    pub fn new(tag: ImageTag, hash: ImageHash, size: u64) -> Self {
        PreparedImage {
            tag,
            hash,
            size,
            build_date: None,
        }
    }

    pub fn with_build_date(
        tag: ImageTag,
        hash: ImageHash,
        size: u64,
        build_date: Option<String>,
    ) -> Self {
        PreparedImage {
            tag,
            hash,
            size,
            build_date,
        }
    }

    pub fn repository(&self) -> &str {
        self.tag.repository()
    }

    pub fn version(&self) -> Version {
        self.tag.version()
    }

    pub fn tag_str(&self) -> &str {
        self.tag.as_str()
    }

    /// Human-readable file size (e.g. `"123.45 MB"`).
    pub fn size_readable(&self) -> String {
        let units = ["B", "KB", "MB", "GB"];
        let mut size = self.size as f64;
        let mut unit_idx = 0;
        while size >= 1024.0 && unit_idx < units.len() - 1 {
            size /= 1024.0;
            unit_idx += 1;
        }
        format!("{:.2} {}", size, units[unit_idx])
    }
}

// ---------------------------------------------------------------------------
// Runtime trait
// ---------------------------------------------------------------------------

/// Core abstraction for a container runtime (e.g. PRoot).
///
/// Obtain a concrete implementation through [`RuntimeFactory`].  All I/O
/// operations are fully async and safe to call from a Tokio executor.
///
/// # Example
/// ```no_run
/// use colmap_openmvs_backend::runtimes::{RuntimeFactory, Runtime, prepare_progress_channel, PrepareProgress};
///
/// # async fn example() -> anyhow::Result<()> {
/// let rt = RuntimeFactory::proot();
/// rt.is_supported()?;
///
/// // Stream prepare progress while awaiting completion
/// let (tx, mut rx) = prepare_progress_channel();
/// tokio::spawn(async move {
///     while let Some(event) = rx.recv().await {
///         println!("{:?}", event);
///     }
/// });
/// rt.prepare("library/alpine:3.18", tx).await?;
///
/// let mut handle = rt.run("library/alpine:3.18", &["echo".into(), "hello".into()]).await?;
/// handle.wait().await?;
/// # Ok(())
/// # }
/// ```
#[async_trait]
pub trait Runtime: Send + Sync {
    /// Check whether this runtime can be installed/run on the current platform.
    /// Returns an error with a human-readable explanation when not supported.
    fn is_supported(&self) -> RuntimeResult<()>;

    /// Return the currently installed runtime binary version string.
    async fn version(&self) -> RuntimeResult<String>;

    /// List downloadable runtime versions, most-recent first.
    async fn available_versions(&self) -> RuntimeResult<Vec<String>>;

    /// Download and install a specific runtime binary version.
    async fn download(&self, version: &str) -> RuntimeResult<()>;

    /// Pull and prepare a container image for execution.
    ///
    /// Progress events are sent through `tx`; the last event is always
    /// [`PrepareProgress::Completed`].  Create the channel with
    /// [`prepare_progress_channel`] or `tokio::sync::mpsc::channel`.
    async fn prepare(&self, image: &str, tx: PrepareProgressTx) -> RuntimeResult<()>;

    /// Remove a previously prepared image from disk.
    async fn remove(&self, image: &str) -> RuntimeResult<()>;

    /// Spawn a process inside a prepared image and return its async handle.
    ///
    /// The returned [`ProcessHandle`] owns the child process; dropping it will
    /// kill the process (via `tokio::process::Child` drop semantics).
    async fn run(&self, image: &str, args: &[String]) -> RuntimeResult<ProcessHandle>;

    /// List all images that have been prepared and are ready to run.
    async fn list_images(&self) -> RuntimeResult<Vec<PreparedImage>>;
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Constructs concrete runtime instances.
pub struct RuntimeFactory;

impl RuntimeFactory {
    /// Create a [`PRoot`] runtime using the configured install directory from settings.
    pub fn proot() -> PRoot {
        let default_dir = crate::settings::default_proot_images_dir().into();
        PRoot::new(default_dir)
    }

    /// Create a [`PRoot`] runtime with a custom install directory.
    pub fn proot_with_dir(install_dir: PathBuf) -> PRoot {
        PRoot::new(install_dir)
    }
}
