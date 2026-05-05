//! Docker image management using OCI Distribution specification.
//!
//! This module handles:
//! - Pulling container images from OCI registries (Docker Hub, etc.)
//! - Storing images by their content digest (immutable identifier)
//! - Extracting rootfs from image layers
//! - Managing image metadata and configuration
//!
//! Images are identified by their digest (SHA256 of manifest), not by tag.
//! Tags are mutable references that can be updated; digests are immutable.

use colmap_openmvs_api::PrepareProgress;
use anyhow::{anyhow, Result};
use oci_distribution::client::Client;
use oci_distribution::secrets::RegistryAuth;
use oci_distribution::Reference;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::str::FromStr;
use tokio::fs;
use tokio::sync::mpsc;

/// Docker image configuration extracted from image config blob
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageConfig {
    pub env: Vec<String>,
    pub entrypoint: Option<Vec<String>>,
    pub cmd: Option<Vec<String>>,
    pub working_dir: Option<String>,
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            env: vec![],
            entrypoint: None,
            cmd: None,
            working_dir: None,
        }
    }
}

/// Image digest info - stores both tag and computed digest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageDigestInfo {
    /// The image reference (tag) that was pulled
    pub reference: String,
    /// The digest of the pulled manifest (immutable content hash)
    pub digest: String,
}

/// Docker image manager using OCI Distribution client
pub struct ImageManager {
    client: Client,
    registry_auth: RegistryAuth,
}

impl ImageManager {
    /// Create a new image manager with anonymous registry authentication
    pub fn new() -> Self {
        Self {
            client: Client::default(),
            registry_auth: RegistryAuth::Anonymous,
        }
    }

    /// Pull a Docker image and extract to a rootfs directory
    ///
    /// # Arguments
    /// * `image_ref` - Image reference (e.g., "ubuntu:22.04", "library/alpine:latest")
    /// * `target_dir` - Directory where rootfs will be extracted
    /// * `progress_tx` - Channel to send progress events
    ///
    /// # Returns
    /// Tuple of (ImageDigestInfo, ImageConfig) - digest info and image configuration
    pub async fn pull_and_extract(
        &self,
        image_ref: &str,
        target_dir: &Path,
        progress_tx: &mpsc::Sender<PrepareProgress>,
    ) -> Result<(ImageDigestInfo, ImageConfig)> {
        // Send resolving event
        let _ = progress_tx.send(PrepareProgress::ResolvingImage).await;

        // Parse image reference
        let reference = Reference::from_str(image_ref)
            .map_err(|e| anyhow!("Invalid image reference '{}': {}", image_ref, e))?;

        // Pull manifest and get digest
        let (manifest, digest) = self
            .client
            .pull_manifest(&reference, &self.registry_auth)
            .await
            .map_err(|e| anyhow!("Failed to pull manifest for {}: {}", image_ref, e))?;

        // Get config descriptor and download config blob
        let config_descriptor = match &manifest {
            oci_distribution::manifest::OciManifest::Image(img) => &img.config,
            oci_distribution::manifest::OciManifest::ImageIndex(_) => {
                return Err(anyhow!(
                    "Multi-platform image index not supported, pull specific platform"
                ));
            }
        };

        let mut config_data = Vec::new();
        self.client
            .pull_blob(&reference, config_descriptor, &mut config_data)
            .await
            .map_err(|e| anyhow!("Failed to download image config: {}", e))?;

        let image_config = self.parse_image_config(&config_data)?;

        // Get layers for progress calculation
        let layers = match &manifest {
            oci_distribution::manifest::OciManifest::Image(img) => &img.layers,
            _ => return Err(anyhow!("Unexpected manifest type")),
        };

        let total_size: u64 = layers.iter().map(|l| l.size as u64).sum();

        // Send downloading event
        let _ = progress_tx
            .send(PrepareProgress::Downloading {
                downloaded_bytes: 0,
                total_bytes: Some(total_size),
            })
            .await;

        // Create target directory
        fs::create_dir_all(target_dir).await?;

        // Download and extract layers
        let mut downloaded_bytes: u64 = 0;
        for (index, layer_descriptor) in layers.iter().enumerate() {
            let _ = progress_tx
                .send(PrepareProgress::ExtractingLayer {
                    layer: format!("Layer {}/{}", index + 1, layers.len()),
                    progress: index as f32 / layers.len() as f32,
                })
                .await;

            let mut layer_data = Vec::new();
            self.client
                .pull_blob(&reference, layer_descriptor, &mut layer_data)
                .await
                .map_err(|e| anyhow!("Failed to download layer {}: {}", index, e))?;

            self.extract_layer(target_dir, &layer_data).await?;

            downloaded_bytes += layer_descriptor.size as u64;
            let _ = progress_tx
                .send(PrepareProgress::Downloading {
                    downloaded_bytes,
                    total_bytes: Some(total_size),
                })
                .await;
        }

        // Send writing rootfs event
        let _ = progress_tx.send(PrepareProgress::WritingRootFs).await;

        // Ensure standard directories exist
        self.ensure_rootfs_structure(target_dir).await?;

        // Send configuring event
        let _ = progress_tx.send(PrepareProgress::Configuring).await;

        let digest_info = ImageDigestInfo {
            reference: image_ref.to_string(),
            digest,
        };

        Ok((digest_info, image_config))
    }

    /// Parse image configuration from config blob
    fn parse_image_config(&self, data: &[u8]) -> Result<ImageConfig> {
        #[derive(Deserialize)]
        struct OciImageConfig {
            #[serde(default)]
            env: Vec<String>,
            #[serde(default)]
            entrypoint: Option<Vec<String>>,
            #[serde(default)]
            cmd: Option<Vec<String>>,
            #[serde(rename = "WorkingDir", default)]
            working_dir: Option<String>,
        }

        let oci_config: OciImageConfig = serde_json::from_slice(data)
            .map_err(|e| anyhow!("Failed to parse image config: {}", e))?;

        Ok(ImageConfig {
            env: oci_config.env,
            entrypoint: oci_config.entrypoint,
            cmd: oci_config.cmd,
            working_dir: oci_config.working_dir,
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

    /// Ensure standard rootfs directory structure exists
    async fn ensure_rootfs_structure(&self, rootfs_dir: &Path) -> Result<()> {
        let dirs = vec![
            "bin", "boot", "dev", "etc", "home", "lib", "lib64", "media", "mnt", "opt", "proc",
            "root", "run", "sbin", "srv", "sys", "tmp", "usr", "var",
        ];

        for dir in dirs {
            let path = rootfs_dir.join(dir);
            if !path.exists() {
                fs::create_dir_all(&path).await?;
            }
        }

        // Create essential files
        let etc_dir = rootfs_dir.join("etc");
        if !etc_dir.join("hostname").exists() {
            fs::write(etc_dir.join("hostname"), "container\n").await?;
        }

        Ok(())
    }
}

impl Default for ImageManager {
    fn default() -> Self {
        Self::new()
    }
}
