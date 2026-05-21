//! OCI image pulling and extraction using oras-project/oci-client.
//!
//! This module handles:
//! - Pulling container images from OCI registries (Docker Hub, etc.)
//! - Extracting image layers to a rootfs directory
//! - Parsing and returning complete image configuration (entrypoint, env, cmd, etc.)
//! - Progress reporting during download and extraction

use anyhow::{anyhow, Result};
use colmap_openmvs_api::PrepareProgress;
use oci_client::config::ConfigFile;
use oci_client::manifest::OciManifest;
use oci_client::secrets::RegistryAuth;
use oci_client::{Client, Reference};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::sync::mpsc;

/// Docker image configuration extracted from image config blob
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageConfig {
    pub env: Vec<String>,
    pub entrypoint: Option<Vec<String>>,
    pub cmd: Option<Vec<String>>,
    pub working_dir: Option<String>,
}

/// Docker image manager using OCI Distribution client
pub struct ImageManager {
    client: Client,
}

impl ImageManager {
    /// Create a new image manager with anonymous registry authentication
    pub fn new() -> Self {
        Self {
            client: Client::default(),
        }
    }

    /// Pull a Docker image and extract to a rootfs directory
    ///
    /// # Arguments
    /// * `image_ref` - Image reference (e.g., "docker.io/library/alpine:latest")
    /// * `target_dir` - Directory where rootfs will be extracted
    /// * `progress_tx` - Channel to send progress events
    ///
    /// # Returns
    /// ImageConfig - image configuration with entrypoint, env, cmd, working_dir
    pub async fn pull_and_extract(
        &self,
        image_ref: &str,
        target_dir: &Path,
        progress_tx: &mpsc::Sender<PrepareProgress>,
    ) -> Result<ImageConfig> {
        // Parse image reference
        let reference: Reference = image_ref
            .parse()
            .map_err(|e| anyhow!("Invalid image reference '{}': {}", image_ref, e))?;

        // Pull manifest to get image config and layers
        let (manifest, _digest) = self
            .client
            .pull_manifest(&reference, &RegistryAuth::Anonymous)
            .await
            .map_err(|e| anyhow!("Failed to pull manifest for {}: {}", image_ref, e))?;

        // Handle both single-platform and multi-platform images
        let (config_descriptor, layers) = match manifest {
            OciManifest::Image(img) => (img.config.clone(), img.layers.clone()),
            OciManifest::ImageIndex(index) => {
                // Auto-select platform matching the host
                let (host_os, host_arch) = get_host_platform();
                let selected_manifest = self
                    .select_platform_manifest(&reference, &index, host_os, host_arch)
                    .await?;
                (
                    selected_manifest.config.clone(),
                    selected_manifest.layers.clone(),
                )
            }
        };

        // Calculate total download size
        let total_size: u64 =
            layers.iter().map(|l| l.size as u64).sum::<u64>() + config_descriptor.size as u64;

        // Send downloading event
        let _ = progress_tx
            .send(PrepareProgress::Downloading {
                downloaded_bytes: 0,
                total_bytes: Some(total_size),
            })
            .await;

        // Create target directory
        tokio::fs::create_dir_all(target_dir).await?;

        // Download and extract layers
        let mut downloaded_bytes: u64 = 0;

        for (index, layer_descriptor) in layers.iter().enumerate() {
            let _ = progress_tx
                .send(PrepareProgress::ExtractingLayer {
                    layer: format!("Layer {}/{}", index + 1, layers.len()),
                    progress: index as f32 / layers.len() as f32,
                })
                .await;

            // Download layer blob
            let mut layer_data = Vec::new();
            self.client
                .pull_blob(
                    &reference,
                    layer_descriptor.digest.as_str(),
                    &mut layer_data,
                )
                .await
                .map_err(|e| anyhow!("Failed to download layer {}: {}", index, e))?;

            // Extract layer (gzip tar archive)
            self.extract_layer(target_dir, &layer_data).await?;

            downloaded_bytes += layer_descriptor.size as u64;
            let _ = progress_tx
                .send(PrepareProgress::Downloading {
                    downloaded_bytes,
                    total_bytes: Some(total_size),
                })
                .await;
        }

        let mut config_bytes: Vec<u8> = Vec::new();
        self.client
            .pull_blob(
                &reference,
                config_descriptor.digest.as_str(),
                &mut config_bytes,
            )
            .await
            .map_err(|e| anyhow!("Failed to download image config blob: {}", e))?;
        let config: ConfigFile = serde_json::from_slice(config_bytes.as_slice())
            .map_err(|e| anyhow!("Failed to parse image config JSON: {}", e))?;
        let config_ref = config.config.as_ref();
        Ok(ImageConfig {
            env: config_ref
                .and_then(|c| c.env.as_ref())
                .cloned()
                .unwrap_or_default(),
            entrypoint: config_ref.and_then(|c| c.entrypoint.as_ref()).cloned(),
            cmd: config_ref.and_then(|c| c.cmd.as_ref()).cloned(),
            working_dir: config_ref.and_then(|c| c.working_dir.as_ref()).cloned(),
        })
    }

    /// Extract a layer (gzip tar archive) to rootfs
    async fn extract_layer(&self, rootfs_dir: &Path, layer_data: &[u8]) -> Result<()> {
        // Layer is gzip compressed tar, decompress and extract
        let decompressed = tokio::task::spawn_blocking({
            let data = layer_data.to_vec();
            move || {
                let mut decoder = flate2::read::GzDecoder::new(&data[..]);
                let mut decompressed = Vec::new();
                std::io::Read::read_to_end(&mut decoder, &mut decompressed)?;
                Ok::<_, std::io::Error>(decompressed)
            }
        })
        .await??;

        // Extract tar archive
        let cursor = std::io::Cursor::new(decompressed);
        let mut archive = tar::Archive::new(cursor);

        let rootfs_dir = rootfs_dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            archive
                .unpack(&rootfs_dir)
                .map_err(|e| anyhow!("Failed to extract layer: {}", e))
        })
        .await??;

        Ok(())
    }
}

impl ImageManager {
    /// Select the appropriate manifest from an image index based on host platform
    async fn select_platform_manifest(
        &self,
        reference: &Reference,
        index: &oci_client::manifest::OciImageIndex,
        host_os: &str,
        host_arch: &str,
    ) -> Result<oci_client::manifest::OciImageManifest> {
        // Find a matching platform in the index
        let _matching_entry = index
            .manifests
            .iter()
            .find(|entry| {
                if let Some(platform) = &entry.platform {
                    let os_str = platform.os.to_string().to_lowercase();
                    let arch_str = platform.architecture.to_string().to_lowercase();
                    os_str == host_os && arch_str == host_arch
                } else {
                    false
                }
            })
            .ok_or_else(|| {
                anyhow!(
                    "No image available for platform {}/{} in multi-arch image",
                    host_os,
                    host_arch
                )
            })?;

        // Pull the specific manifest for this platform
        let (manifest, _, _) = self
            .client
            .pull_manifest_and_config(reference, &RegistryAuth::Anonymous)
            .await
            .map_err(|e| anyhow!("Failed to pull platform-specific manifest: {}", e))?;

        Ok(manifest)
    }
}

impl Default for ImageManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Get the current host's OS and architecture
fn get_host_platform() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "windows",
        "android" => "android",
        "ios" => "ios",
        other => other,
    };

    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "arm",
        "i686" => "386",
        "powerpc64" => "ppc64le",
        "s390x" => "s390x",
        other => other,
    };

    (os, arch)
}
