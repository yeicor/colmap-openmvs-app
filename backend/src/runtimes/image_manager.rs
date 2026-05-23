//! OCI image pulling and extraction using reqwest for HTTP operations.
//!
//! This module handles:
//! - Pulling container images from OCI registries (Docker Hub, etc.)
//! - Extracting image layers to a rootfs directory
//! - Parsing and returning complete image configuration (entrypoint, env, cmd, etc.)
//! - Progress reporting during download and extraction

use anyhow::{anyhow, Result};
use colmap_openmvs_api::PrepareProgress;
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
    pub created: Option<String>,
}

/// OCI image manifest v2 schema
#[derive(Debug, Deserialize)]
pub struct OciImageManifest {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    #[serde(rename = "mediaType")]
    pub media_type: Option<String>,
    pub config: Descriptor,
    pub layers: Vec<Descriptor>,
}

/// OCI image index for multi-platform images
#[derive(Debug, Deserialize)]
pub struct OciImageIndex {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    #[serde(rename = "mediaType")]
    pub media_type: Option<String>,
    pub manifests: Vec<IndexEntry>,
}

/// Entry in an OCI image index
#[derive(Debug, Deserialize)]
pub struct IndexEntry {
    pub digest: String,
    #[serde(rename = "mediaType")]
    pub media_type: Option<String>,
    pub size: Option<i64>,
    pub platform: Option<Platform>,
}

/// Platform specification
#[derive(Debug, Deserialize)]
pub struct Platform {
    pub os: String,
    pub architecture: String,
    #[serde(default)]
    pub variant: Option<String>,
}

/// Descriptor for an OCI object (layer, config, etc.)
#[derive(Debug, Deserialize, Clone)]
pub struct Descriptor {
    #[serde(rename = "mediaType")]
    pub media_type: String,
    pub size: i64,
    pub digest: String,
}

/// Docker image configuration file
#[derive(Debug, Deserialize, Default)]
pub struct ConfigFile {
    #[serde(default)]
    pub config: Option<Config>,
    #[serde(default)]
    pub created: Option<String>,
}

/// Docker container config
#[derive(Debug, Deserialize, Default)]
pub struct Config {
    #[serde(rename = "Env", default)]
    pub env: Option<Vec<String>>,
    #[serde(rename = "Entrypoint", default)]
    pub entrypoint: Option<Vec<String>>,
    #[serde(rename = "Cmd", default)]
    pub cmd: Option<Vec<String>>,
    #[serde(rename = "WorkingDir", default)]
    pub working_dir: Option<String>,
}

/// Docker image manager using reqwest for HTTP operations
pub struct ImageManager;

impl ImageManager {
    /// Create a new image manager
    pub fn new() -> Self {
        Self
    }

    /// Pull a Docker image and extract to a rootfs directory
    ///
    /// # Arguments
    /// * `image_ref` - Image reference (e.g., "docker.io/library/alpine:latest" or "yeicor/colmap-openmvs:latest")
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
        let (registry, image_name, tag) = parse_image_ref(image_ref)?;

        // Construct registry API URL
        let registry_url = format!("https://{}", registry);
        let api_path = format!("/v2/{}/manifests/{}", image_name, tag);

        // Pull manifest to get image config and layers
        let client = reqwest::Client::new();
        let manifest_url = format!("{}{}", registry_url, api_path);

        let manifest_response = client
            .get(&manifest_url)
            .header("Accept", "application/vnd.docker.distribution.manifest.v2+json,application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.list.v2+json")
            .header("User-Agent", "colmap-openmvs-app")
            .send()
            .await
            .map_err(|e| anyhow!("Failed to fetch manifest for {}: {}", image_ref, e))?;

        if !manifest_response.status().is_success() {
            return Err(anyhow!(
                "Failed to fetch manifest: {} - {}",
                manifest_response.status(),
                manifest_response.text().await.unwrap_or_default()
            ));
        }

        let manifest_text = manifest_response
            .text()
            .await
            .map_err(|e| anyhow!("Failed to read manifest: {}", e))?;

        // Try to parse as image manifest first
        let (config_descriptor, layers) = if let Ok(manifest) =
            serde_json::from_str::<OciImageManifest>(&manifest_text)
        {
            (manifest.config, manifest.layers)
        } else if let Ok(index) = serde_json::from_str::<OciImageIndex>(&manifest_text) {
            // Multi-platform image - select for this platform
            let (host_os, host_arch) = get_host_platform();
            let selected_entry = index
                .manifests
                .iter()
                .find(|entry| {
                    if let Some(platform) = &entry.platform {
                        platform.os.to_lowercase() == host_os
                            && platform.architecture.to_lowercase() == host_arch
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

            // Fetch the specific manifest for this platform
            let platform_manifest_url = format!(
                "{}{}",
                registry_url,
                api_path.replace(&tag, &selected_entry.digest)
            );
            let platform_response = client
                .get(&platform_manifest_url)
                .header("Accept", "application/vnd.docker.distribution.manifest.v2+json,application/vnd.oci.image.manifest.v1+json")
                .header("User-Agent", "colmap-openmvs-app")
                .send()
                .await
                .map_err(|e| anyhow!("Failed to fetch platform-specific manifest: {}", e))?;

            let platform_text = platform_response
                .text()
                .await
                .map_err(|e| anyhow!("Failed to read platform manifest: {}", e))?;

            let manifest: OciImageManifest = serde_json::from_str(&platform_text)
                .map_err(|e| anyhow!("Failed to parse platform manifest: {}", e))?;

            (manifest.config, manifest.layers)
        } else {
            return Err(anyhow!("Failed to parse manifest - unknown format"));
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
            let blob_url = format!(
                "{}/v2/{}/blobs/{}",
                registry_url, image_name, layer_descriptor.digest
            );

            let layer_data = client
                .get(&blob_url)
                .header("User-Agent", "colmap-openmvs-app")
                .send()
                .await
                .map_err(|e| anyhow!("Failed to download layer {}: {}", index, e))?
                .bytes()
                .await
                .map_err(|e| anyhow!("Failed to read layer {}: {}", index, e))?;

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

        // Download and parse config blob
        let config_url = format!(
            "{}/v2/{}/blobs/{}",
            registry_url, image_name, config_descriptor.digest
        );

        let config_bytes = client
            .get(&config_url)
            .header("User-Agent", "colmap-openmvs-app")
            .send()
            .await
            .map_err(|e| anyhow!("Failed to download image config blob: {}", e))?
            .bytes()
            .await
            .map_err(|e| anyhow!("Failed to read config blob: {}", e))?;

        let config: ConfigFile = serde_json::from_slice(&config_bytes)
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
            created: config.created,
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

impl Default for ImageManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse image reference in the form: [registry/]image[:tag]
/// Returns (registry, image_name, tag)
fn parse_image_ref(image_ref: &str) -> Result<(String, String, String)> {
    let mut parts = image_ref.split('/');

    let first = parts
        .next()
        .ok_or_else(|| anyhow!("Invalid image reference"))?;

    // Check if first part is registry (contains . or :)
    let (registry, remaining) = if first.contains('.') || first.contains(':') {
        // First part is registry
        (
            first.to_string(),
            parts
                .next()
                .ok_or_else(|| anyhow!("Invalid image reference: no image name after registry"))?,
        )
    } else {
        // Use Docker Hub as default registry
        ("docker.io".to_string(), first)
    };

    // Collect remaining parts as image name
    let mut image_parts = vec![remaining];
    image_parts.extend(parts);
    let image_and_tag = image_parts.join("/");

    // Split image name and tag
    let (image_name, tag) = if let Some(tag_pos) = image_and_tag.rfind(':') {
        let (name, tag_part) = image_and_tag.split_at(tag_pos);
        (name.to_string(), tag_part[1..].to_string())
    } else {
        (image_and_tag.to_string(), "latest".to_string())
    };

    // Ensure full image name for Docker Hub (library prefix)
    let full_image_name = if registry == "docker.io" && !image_name.contains('/') {
        format!("library/{}", image_name)
    } else {
        image_name
    };

    Ok((registry.to_string(), full_image_name, tag))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_image_config_with_metadata() {
        let json_str = r#"{
            "architecture": "amd64",
            "config": {
                "Env": ["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"],
                "Entrypoint": ["/bin/sh"],
                "Cmd": ["-c", "echo hello"],
                "WorkingDir": "/home/user"
            },
            "created": "2026-05-23T18:31:41.577243600+00:00"
        }"#;

        let config: ConfigFile = serde_json::from_str(json_str).expect("Failed to parse JSON");

        assert_eq!(
            config.created,
            Some("2026-05-23T18:31:41.577243600+00:00".to_string())
        );
        assert!(config.config.is_some());

        let cfg = config.config.unwrap();
        assert_eq!(
            cfg.env,
            Some(vec![
                "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string()
            ])
        );
        assert_eq!(cfg.entrypoint, Some(vec!["/bin/sh".to_string()]));
        assert_eq!(
            cfg.cmd,
            Some(vec!["-c".to_string(), "echo hello".to_string()])
        );
        assert_eq!(cfg.working_dir, Some("/home/user".to_string()));
    }

    #[test]
    fn test_parse_image_config_empty_values() {
        let json_str = r#"{
            "architecture": "amd64",
            "config": {
                "Env": [],
                "Entrypoint": null,
                "Cmd": null,
                "WorkingDir": "/home/user"
            },
            "created": "2026-05-23T18:31:41.577243600+00:00"
        }"#;

        let config: ConfigFile = serde_json::from_str(json_str).expect("Failed to parse JSON");

        assert_eq!(
            config.created,
            Some("2026-05-23T18:31:41.577243600+00:00".to_string())
        );
        assert!(config.config.is_some());

        let cfg = config.config.unwrap();
        assert_eq!(cfg.env, Some(vec![]));
        assert_eq!(cfg.entrypoint, None);
        assert_eq!(cfg.cmd, None);
        assert_eq!(cfg.working_dir, Some("/home/user".to_string()));
    }

    #[test]
    fn test_parse_image_config_missing_fields() {
        let json_str = r#"{
            "architecture": "amd64",
            "config": {
                "WorkingDir": "/"
            },
            "created": "2026-05-23T18:31:41.577243600+00:00"
        }"#;

        let config: ConfigFile = serde_json::from_str(json_str).expect("Failed to parse JSON");

        assert_eq!(
            config.created,
            Some("2026-05-23T18:31:41.577243600+00:00".to_string())
        );
        assert!(config.config.is_some());

        let cfg = config.config.unwrap();
        assert_eq!(cfg.env, None);
        assert_eq!(cfg.entrypoint, None);
        assert_eq!(cfg.cmd, None);
        assert_eq!(cfg.working_dir, Some("/".to_string()));
    }
}
