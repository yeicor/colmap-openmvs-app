//! Server-side implementations for runtime management.

use crate::runtimes::{prepare_progress_channel, Runtime, RuntimeFactory};
use colmap_openmvs_api::{ImageTagInfo, PrepareProgress, PreparedImageInfo, RuntimeInfo};
use dioxus::fullstack::ServerEvents;
use dioxus::Result;
use futures_util::StreamExt;
use reqwest::Client;

/// Return the current status of the PRoot runtime.
pub async fn get_runtime_info() -> Result<RuntimeInfo> {
    let rt = RuntimeFactory::proot();

    let (supported, unsupported_reason) = match rt.is_supported() {
        Ok(()) => (true, None),
        Err(e) => (false, Some(e.to_string())),
    };

    let (installed, version) = if supported {
        match rt.version().await {
            Ok(v) => (true, Some(v)),
            Err(_) => (false, None),
        }
    } else {
        (false, None)
    };

    Ok(RuntimeInfo {
        name: "PRoot".to_string(),
        supported,
        unsupported_reason,
        installed,
        version,
    })
}

/// List available PRoot versions that can be downloaded, most-recent first.
pub async fn get_available_runtime_versions() -> Result<Vec<String>> {
    let rt = RuntimeFactory::proot();
    rt.available_versions()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e).into())
}

/// Download and install a specific PRoot version.
pub async fn download_runtime_version(version: String) -> Result<()> {
    let rt = RuntimeFactory::proot();
    rt.download(&version)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e).into())
}

/// List all prepared container images stored on disk.
pub async fn list_runtime_images() -> Result<Vec<PreparedImageInfo>> {
    let rt = RuntimeFactory::proot();
    let images = rt
        .list_images()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    Ok(images
        .into_iter()
        .map(|img| PreparedImageInfo {
            tag: img.tag_str().to_string(),
            hash: img.hash.to_string(),
            size: img.size,
            size_readable: img.size_readable(),
            build_date: img.build_date,
        })
        .collect())
}

/// Prepare a container image for execution, streaming progress events to the caller.
pub async fn prepare_runtime_image(image: String) -> Result<ServerEvents<PrepareProgress>> {
    let rt = RuntimeFactory::proot();
    let (progress_tx, mut progress_rx) = prepare_progress_channel();
    let (stream_tx, stream_rx) = futures::channel::mpsc::unbounded::<PrepareProgress>();

    let stream_tx_err = stream_tx.clone();
    let image_for_task = image.clone();
    let rt_for_task = rt.clone();

    // Task A: run the prepare operation; on error, send an Error event.
    tokio::spawn(async move {
        if let Err(e) = rt_for_task.prepare(&image_for_task, progress_tx).await {
            let _ = stream_tx_err.unbounded_send(PrepareProgress::Error {
                message: e.to_string(),
            });
        }
    });

    // Task B: forward events directly.
    tokio::spawn(async move {
        while let Some(event) = progress_rx.recv().await {
            let is_terminal = matches!(
                event,
                PrepareProgress::Completed | PrepareProgress::Error { .. }
            );
            let _ = stream_tx.unbounded_send(event);
            if is_terminal {
                break;
            }
        }
    });

    Ok(ServerEvents::from_stream(stream_rx.map(Ok)))
}

/// Remove a previously prepared container image from disk.
pub async fn remove_runtime_image(image_hash: String) -> Result<()> {
    let rt = RuntimeFactory::proot();
    rt.remove(&image_hash)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e).into())
}

/// List available tags for the colmap-openmvs image from Docker Hub, sorted by date.
/// Filters out platform-specific tags (-latest, -arm64-, -amd64-) to show only multi-arch tags.
pub async fn list_available_image_tags() -> Result<Vec<ImageTagInfo>> {
    let client = Client::new();
    let url =
        "https://registry.hub.docker.com/v2/repositories/yeicor/colmap-openmvs/tags?page_size=100";

    let response = client
        .get(url)
        .header("User-Agent", "colmap-openmvs-app")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to fetch image tags: {}", e))?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "Docker Hub API returned status {}: {}",
            response.status(),
            response.text().await.unwrap_or_default()
        )
        .into());
    }

    #[derive(serde::Deserialize)]
    struct TagResult {
        name: String,
        #[serde(default)]
        last_updated: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct TagsResponse {
        results: Vec<TagResult>,
    }

    let body = response
        .text()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read response: {}", e))?;

    let tags_response: TagsResponse = serde_json::from_str(&body)
        .map_err(|e| anyhow::anyhow!("Failed to parse tags response: {}", e))?;

    // Filter tags to exclude platform-specific variants
    let mut tags: Vec<_> = tags_response
        .results
        .into_iter()
        .filter(|r| {
            // Skip -latest, -arm64-, and -amd64- variants
            !r.name.ends_with("-latest")
                && !r.name.contains("-arm64-")
                && !r.name.contains("-amd64-")
        })
        .map(|r| (r.name, r.last_updated))
        .collect();

    tags.sort_by(|a, b| {
        let date_cmp = b.1.cmp(&a.1);
        if date_cmp == std::cmp::Ordering::Equal {
            b.0.cmp(&a.0)
        } else {
            date_cmp
        }
    });

    let result: Vec<ImageTagInfo> = tags
        .into_iter()
        .map(|(name, build_date)| ImageTagInfo { name, build_date })
        .collect();
    Ok(result)
}
