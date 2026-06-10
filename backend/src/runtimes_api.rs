//! Server-side implementations for runtime management.

use crate::runtimes::{prepare_progress_channel, Runtime, RuntimeFactory};
use crate::task_registry::TASK_REGISTRY;
use crate::AndroidSettingsValidation;
use colmap_openmvs_api::{
    ImageTagInfo, PrepareProgress, PreparedImageInfo, ProjectRunStatus, RuntimeInfo, TaskEvent,
    TaskEventBatch, TaskInfo, TaskKind, TaskState,
};
use dioxus::Result;
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

    let task_id = TASK_REGISTRY.create_task(TaskKind::PrepareImage, image.clone());

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
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent("colmap-openmvs-app")
        .build()
        .into();
    let url =
        "https://registry.hub.docker.com/v2/repositories/yeicor/colmap-openmvs/tags?page_size=100";

    let response = agent
        .get(url)
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| anyhow::anyhow!("Failed to fetch image tags: {}", e))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.into_body().read_to_string().unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Docker Hub API returned status {}: {}",
            status,
            body
        ));
    }

    let body = response
        .into_body()
        .read_to_string()
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
/// On Android, this function returns an empty list with an explanatory message.
/// Due to platform security features, no downloadable images are available. You must update
/// the entire application to change the rootfs. Ready images always show the embedded image.
///
/// On non-Android platforms, the embedded image tag (if available) is prepended to the list
/// so it always appears first, even when Docker Hub is unreachable. If the Docker Hub fetch
/// fails entirely but an embedded tag is available, only the embedded tag is returned instead
/// of propagating the error.
#[cfg_attr(target_os = "android", allow(unreachable_code))]
pub async fn list_available_image_tags() -> Result<Vec<ImageTagInfo>> {
    // On Android, ensure the embedded runtime environment is set up first.
    // This is idempotent and safe to call multiple times.
    #[cfg(target_os = "android")]
    if let Err(e) = crate::android_startup::setup_android_runtime().await {
        tracing::warn!(error = %e, "list_available_image_tags: Android setup failed, continuing anyway");
    }

    // On Android, no downloadable images are available due to platform security features.
    // You must update the entire application to change the rootfs.
    #[cfg(target_os = "android")]
    {
        tracing::info!("list_available_image_tags: returning empty list on Android (no downloadable images available)");
        return Err(anyhow::anyhow!("No downloadable images available on Android (due to platform security features); update the entire application to change the rootfs.").into());
    }

    // On non-Android targets, probe for the embedded tag first.
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
    let mut tasks = TASK_REGISTRY.list_tasks();
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
    Ok(TASK_REGISTRY.get_task_info(&task_id))
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
    TASK_REGISTRY.cancel_task(&task_id);
    Ok(())
}

/// Poll for new task events since `cursor`. Returns batched events.
/// This is the universal replacement for the SSE-based subscribe_task_events.
///
/// `limit` controls the maximum number of events returned per poll.  Use
/// `Some(500)` to paginate through large event logs gradually.  Pass `None`
/// to get all remaining events in a single response.
pub async fn poll_task_events(
    task_id: String,
    cursor: usize,
    limit: Option<usize>,
) -> Result<TaskEventBatch> {
    match TASK_REGISTRY.poll_events(&task_id, cursor, limit) {
        Some(batch) => Ok(batch),
        None => Ok(TaskEventBatch {
            events: vec![],
            cursor,
            is_terminal: false,
            task_found: false,
        }),
    }
}

// ---------------------------------------------------------------------------
// Android settings repair
// ---------------------------------------------------------------------------

/// Repair invalid Android settings paths and re-trigger runtime setup.
/// Returns the task ID immediately; subscribe with `subscribe_task_events`.
/// On non-Android platforms, this completes immediately without doing anything.
pub async fn repair_android_settings() -> Result<String> {
    let task_id = TASK_REGISTRY.create_task(TaskKind::AndroidSettingsRepair, "system".to_string());

    let task_id_clone = task_id.clone();
    tokio::spawn(async move {
        crate::task_registry::publish_event(
            &task_id_clone,
            TaskEvent::Log("Starting Android settings validation and repair...".to_string()),
        );

        match repair_android_settings_impl().await {
            Ok(_string) => {
                crate::task_registry::publish_event(
                    &task_id_clone,
                    TaskEvent::Log("Android settings repair completed successfully.".to_string()),
                );
                crate::task_registry::publish_event(&task_id_clone, TaskEvent::Completed);
            }
            Err(e) => {
                let error_msg = format!("Android settings repair failed: {}", e);
                crate::task_registry::publish_event(
                    &task_id_clone,
                    TaskEvent::Log(error_msg.clone()),
                );
                crate::task_registry::publish_event(&task_id_clone, TaskEvent::Failed(error_msg));
            }
        }
    });

    Ok(task_id)
}

/// Repair invalid Android settings paths and trigger runtime setup.
/// This is a no-op on non-Android platforms.
pub async fn repair_android_settings_impl() -> anyhow::Result<()> {
    if AndroidSettingsValidation::repair_paths().await? {
        tracing::info!("Android settings repair completed");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Docker runtime API
// ---------------------------------------------------------------------------

/// Return the current status of the Docker runtime.
pub async fn get_docker_runtime_info() -> Result<RuntimeInfo> {
    let rt = RuntimeFactory::docker();
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
        name: "Docker".to_string(),
        supported,
        unsupported_reason,
        installed,
        version,
    })
}

/// List all Docker images available locally on the system.
pub async fn list_docker_images() -> Result<Vec<PreparedImageInfo>> {
    let rt = RuntimeFactory::docker();
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

/// Pull a Docker image, streaming progress via the task registry.
/// Returns the task ID immediately; subscribe with `subscribe_task_events`.
pub async fn prepare_docker_image(image: String) -> Result<String> {
    let rt = RuntimeFactory::docker();

    let task_id = TASK_REGISTRY.create_task(TaskKind::PrepareImage, image.clone());

    let task_id_clone = task_id.clone();
    tokio::spawn(async move {
        let (tx, mut rx) = prepare_progress_channel();

        // Relay PrepareProgress events into the task registry
        let task_id_relay = task_id_clone.clone();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                TASK_REGISTRY.publish_event(&task_id_relay, TaskEvent::PrepareProgress(event));
            }
        });

        match rt.prepare(&image, tx).await {
            Ok(()) => {
                TASK_REGISTRY.publish_event(&task_id_clone, TaskEvent::Completed);
            }
            Err(e) => {
                TASK_REGISTRY.publish_event(&task_id_clone, TaskEvent::Failed(e.to_string()));
            }
        }
    });

    Ok(task_id)
}

/// Remove a Docker image by its tag (or ID).
pub async fn remove_docker_image(image_tag: String) -> Result<()> {
    let rt = RuntimeFactory::docker();
    rt.remove(&image_tag)
        .await
        .map_err(|e| anyhow::anyhow!("{}", e).into())
}

// ---------------------------------------------------------------------------
// Pipeline status querying
// ---------------------------------------------------------------------------

/// Query the current run status for a project.
/// Returns information about any active or recently completed RunPipeline/RecoverPipelineLogs task.
pub async fn get_project_run_status(project_name: String) -> Result<ProjectRunStatus> {
    let tasks = TASK_REGISTRY.list_tasks();

    // Look for the most recent RunPipeline or RecoverPipelineLogs task for this project
    for task_info in &tasks {
        if task_info.context_key == project_name
            && (task_info.kind == TaskKind::RunPipeline
                || task_info.kind == TaskKind::RecoverPipelineLogs)
        {
            let is_running = matches!(task_info.state, TaskState::Running);
            let is_recover_logs = task_info.kind == TaskKind::RecoverPipelineLogs;

            // Try to extract progress from the task's event log
            let progress = is_running
                .then(|| TASK_REGISTRY.latest_pipeline_progress(&task_info.id))
                .flatten();

            return Ok(ProjectRunStatus {
                is_running,
                is_recover_logs,
                progress,
                task_id: task_info.id.clone(),
            });
        }
    }

    // No active or recent task found
    Ok(ProjectRunStatus {
        is_running: false,
        is_recover_logs: false,
        progress: None,
        task_id: String::new(),
    })
}
