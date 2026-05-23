//! Server-side implementations for runtime management.

use crate::runtimes::{prepare_progress_channel, Runtime, RuntimeFactory};
use crate::task_registry::TASK_REGISTRY;
use colmap_openmvs_api::{
    ImageTagInfo, PrepareProgress, PreparedImageInfo, RuntimeInfo, TaskEvent, TaskInfo, TaskKind,
};
use dioxus::fullstack::ServerEvents;
use dioxus::Result;

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

/// Prepare a container image, streaming progress via the task registry.
/// Returns the task ID immediately; subscribe with `create_event_stream`.
pub async fn prepare_runtime_image(image: String) -> Result<String> {
    let rt = RuntimeFactory::proot();

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
    let rt = RuntimeFactory::proot();
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
