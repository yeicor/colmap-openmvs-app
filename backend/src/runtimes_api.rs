//! Server-side implementations for runtime management.

use crate::runtimes::{prepare_progress_channel, Runtime, RuntimeFactory};
use crate::task_registry::TASK_REGISTRY;
use colmap_openmvs_api::{
    ImageTagInfo, PrepareProgress, PreparedImageInfo, RuntimeInfo, TaskEvent, TaskInfo, TaskKind,
};
use dioxus::fullstack::ServerEvents;
use dioxus::Result;
use reqwest::Client;
use tracing::warn;

/// Return the current status of the PRoot runtime.
pub async fn get_runtime_info() -> Result<RuntimeInfo> {
    // On Android, ensure the embedded runtime environment is set up first.
    // This is idempotent and safe to call multiple times.
    #[cfg(target_os = "android")]
    if let Err(e) = crate::android_startup::setup_android_runtime().await {
        tracing::warn!(error = %e, "get_runtime_info: Android setup failed, continuing anyway");
    }

    let rt = RuntimeFactory::proot().await;

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
    let rt = RuntimeFactory::proot().await;
    rt.available_versions()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e).into())
}

/// Download and install a specific PRoot version.
pub async fn download_runtime_version(version: String) -> Result<()> {
    let rt = RuntimeFactory::proot().await;
    rt.download(&version)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e).into())
}

/// List all prepared container images stored on disk.
pub async fn list_runtime_images() -> Result<Vec<PreparedImageInfo>> {
    // On Android, ensure the embedded runtime environment is set up first.
    // This is idempotent and safe to call multiple times.
    #[cfg(target_os = "android")]
    if let Err(e) = crate::android_startup::setup_android_runtime().await {
        tracing::warn!(error = %e, "list_runtime_images: Android setup failed, continuing anyway");
    }

    let rt = RuntimeFactory::proot().await;
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

/// Prepare a container image, streaming progress via the task registry.
/// Returns the task ID immediately; subscribe with `create_event_stream`.
pub async fn prepare_runtime_image(image: String) -> Result<String> {
    let rt = RuntimeFactory::proot().await;

    let task_id = {
        let mut registry = TASK_REGISTRY.lock().unwrap();
        registry.create_task(TaskKind::PrepareImage, image.clone())
    };

    let task_id_clone = task_id.clone();
    tokio::spawn(async move {
        let (progress_tx, mut progress_rx) = prepare_progress_channel();

        let rt_clone = rt.clone();
        let image_clone = image.clone();
        let prepare_handle =
            tokio::spawn(async move { rt_clone.prepare(&image_clone, progress_tx).await });

        while let Some(event) = progress_rx.recv().await {
            let is_error = matches!(event, PrepareProgress::Error { .. });
            crate::task_registry::publish_event(&task_id_clone, TaskEvent::PrepareProgress(event));
            if is_error {
                crate::task_registry::publish_event(
                    &task_id_clone,
                    TaskEvent::Failed("Image preparation failed.".to_string()),
                );
                return;
            }
        }

        match prepare_handle.await {
            Ok(Ok(())) => {
                crate::task_registry::publish_event(&task_id_clone, TaskEvent::Completed);
            }
            Ok(Err(e)) => {
                crate::task_registry::publish_event(
                    &task_id_clone,
                    TaskEvent::Failed(e.to_string()),
                );
            }
            Err(e) => {
                crate::task_registry::publish_event(
                    &task_id_clone,
                    TaskEvent::Failed(format!("Task panicked: {}", e)),
                );
            }
        }
    });

    Ok(task_id)
}

/// Remove a previously prepared container image from disk.
pub async fn remove_runtime_image(image_hash: String) -> Result<()> {
    let rt = RuntimeFactory::proot().await;
    rt.remove(&image_hash)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e).into())
}

// ---------------------------------------------------------------------------
// Image tag listing
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct HubTagResult {
    name: String,
    #[serde(default)]
    last_updated: Option<String>,
}

#[derive(serde::Deserialize)]
struct HubTagsResponse {
    results: Vec<HubTagResult>,
}

/// Private helper: fetch and sort image tags from Docker Hub.
async fn fetch_hub_image_tags() -> anyhow::Result<Vec<ImageTagInfo>> {
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
        ));
    }

    let body = response
        .text()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read response: {}", e))?;

    let tags_response: HubTagsResponse = serde_json::from_str(&body)
        .map_err(|e| anyhow::anyhow!("Failed to parse tags response: {}", e))?;

    // Filter out platform-specific variant tags
    let mut tags: Vec<_> = tags_response
        .results
        .into_iter()
        .filter(|r| {
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

    Ok(tags
        .into_iter()
        .map(|(name, build_date)| ImageTagInfo { name, build_date })
        .collect())
}

/// List available tags for the colmap-openmvs image from Docker Hub, sorted by date.
/// Filters out platform-specific tags (-latest, -arm64-, -amd64-) to show only multi-arch tags.
///
/// On Android the embedded image tag (from `libembedded_metadata.so`) is prepended to the list
/// so it always appears first, even when Docker Hub is unreachable. If the Docker Hub fetch fails
/// entirely but an embedded tag is available, only the embedded tag is returned instead of
/// propagating the error.
pub async fn list_available_image_tags() -> Result<Vec<ImageTagInfo>> {
    // On Android, ensure the embedded runtime environment is set up first.
    // This is idempotent and safe to call multiple times.
    #[cfg(target_os = "android")]
    if let Err(e) = crate::android_startup::setup_android_runtime().await {
        tracing::warn!(error = %e, "list_available_image_tags: Android setup failed, continuing anyway");
    }

    // Probe for the embedded tag first (no-op on non-Android targets).
    let embedded_tag = crate::settings::read_embedded_image_tag_public().map(|name| ImageTagInfo {
        name,
        build_date: None,
    });

    match fetch_hub_image_tags().await {
        Ok(mut hub_tags) => {
            // Prepend the embedded tag if it isn't already in the hub list.
            if let Some(embedded) = embedded_tag {
                if !hub_tags.iter().any(|t| t.name == embedded.name) {
                    hub_tags.insert(0, embedded);
                }
            }
            Ok(hub_tags)
        }
        Err(e) => {
            // Docker Hub unreachable – fall back to the embedded tag when available.
            if let Some(embedded) = embedded_tag {
                warn!(error = %e, "Docker Hub fetch failed; returning embedded image tag only");
                Ok(vec![embedded])
            } else {
                Err(e.into())
            }
        }
    }
}

/// Get the embedded image tag if running on Android with embedded assets.
/// Returns `None` on non-Android targets or when the metadata file is absent.
pub async fn get_embedded_image_tag() -> Result<Option<String>> {
    Ok(crate::settings::read_embedded_image_tag_public())
}

// ---------------------------------------------------------------------------
// Task management
// ---------------------------------------------------------------------------

/// List tasks, optionally filtered by kind and/or context_key.
pub async fn list_tasks(
    kind_filter: Option<TaskKind>,
    context_key_filter: Option<String>,
) -> Result<Vec<TaskInfo>> {
    let registry = TASK_REGISTRY.lock().unwrap();
    let mut tasks = registry.list_tasks();
    if let Some(kind) = kind_filter {
        tasks.retain(|t| t.kind == kind);
    }
    if let Some(ctx) = context_key_filter {
        tasks.retain(|t| t.context_key == ctx);
    }
    Ok(tasks)
}

/// Get info for a single task by ID.
pub async fn get_task_info(task_id: String) -> Result<Option<TaskInfo>> {
    let registry = TASK_REGISTRY.lock().unwrap();
    Ok(registry.get_task_info(&task_id))
}

/// Delete the PRoot binary if it's installed in the custom location.
/// Returns an error if the binary is from the system PATH (not deletable).
pub async fn delete_runtime_binary() -> Result<()> {
    let rt = RuntimeFactory::proot().await;
    rt.delete_binary()
        .await
        .map_err(|e| anyhow::anyhow!("{}", e).into())
}

/// Cancel a running task.
pub async fn cancel_task(task_id: String) -> Result<()> {
    TASK_REGISTRY.lock().unwrap().cancel_task(&task_id);
    Ok(())
}

/// Subscribe to a task's event stream (replays history + live).
/// Returns a ServerEvents stream of TaskEvents.
pub async fn subscribe_task_events(task_id: String) -> Result<ServerEvents<TaskEvent>> {
    use futures_util::TryStreamExt;
    let stream = crate::task_registry::create_event_stream(&task_id)
        .ok_or_else(|| anyhow::anyhow!("Task not found: {}", task_id))?;
    let stream = stream.map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
        Box::new(std::io::Error::other(e.to_string()))
    });
    Ok(ServerEvents::from_stream(stream))
}
