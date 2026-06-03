//! Pipeline execution: runs the colmap-openmvs container pipeline and streams events.

use anyhow::Result;
use colmap_openmvs_api::{PipelineStageStatus, TaskEvent, TaskKind};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use tokio::io::AsyncRead;
use tracing::{debug, error, info, span, Level};

use crate::{
    line_reader::LineReader,
    process::kill_process_tree,
    runtimes::{Mount, RuntimeFactory},
    settings::get_settings,
    task_registry::TASK_REGISTRY,
};

// ---------------------------------------------------------------------------
// Regex patterns
// ---------------------------------------------------------------------------

static GROUP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^::group (.+)::(.+)$").unwrap());
static REMAINING_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^::remaining_groups::(.+)$").unwrap());
static ENDGROUP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^::endgroup::").unwrap());

static PERCENT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d+(?:\.\d+)?)\s*%").unwrap());
static FRACTION_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d+)\s*/\s*(\d+)").unwrap());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Start the reconstruction pipeline for a project. Returns the task ID.
pub async fn run_pipeline(project_name: String, dry_run: bool) -> dioxus::Result<String> {
    let span = span!(Level::DEBUG, "run_pipeline", project = %project_name, dry_run = %dry_run);
    let _enter = span.enter();

    debug!("Starting pipeline execution");
    let settings = get_settings().await.map_err(|e| anyhow::anyhow!("{}", e))?;
    let (runtime, image_tag) = settings.parse_default_image();
    let runtime = runtime
        .ok_or_else(|| {
            error!("No runtime in default image configured");
            anyhow::anyhow!("No runtime in default image configured")
        })?
        .to_string();
    let image_tag = image_tag
        .ok_or_else(|| {
            error!("No default image configured");
            anyhow::anyhow!("No default image configured")
        })?
        .to_string();
    debug!(runtime = %runtime, image_tag = %image_tag, "Using runtime and container image");

    let project_path = crate::project::resolve_project_path(&project_name)
        .await
        .map_err(|e| {
            error!(error = %e, "Project path resolution failed");
            anyhow::anyhow!("{}", e)
        })?;
    debug!(project_path = %project_path.display(), "Resolved project path");

    let task_kind = if dry_run {
        TaskKind::DryRunPipeline
    } else {
        TaskKind::RunPipeline
    };

    let task_id = TASK_REGISTRY.create_task(task_kind, project_name.clone());
    info!(task_id = %task_id, "Pipeline task created");

    let task_id_clone = task_id.clone();
    tokio::spawn(async move {
        if let Err(e) = run_pipeline_task(
            task_id_clone.clone(),
            runtime,
            image_tag,
            project_path.to_string_lossy().into_owned(),
            dry_run,
        )
        .await
        {
            error!(error = %e, "Pipeline task failed");
            crate::task_registry::publish_event(&task_id_clone, TaskEvent::Failed(e.to_string()));
        }
    });

    Ok(task_id)
}

// ---------------------------------------------------------------------------
// Internal task runner
// ---------------------------------------------------------------------------

async fn run_pipeline_task(
    task_id: String,
    runtime: String,
    image_tag: String,
    project_path: String,
    dry_run: bool,
) -> Result<()> {
    let span = span!(Level::DEBUG, "run_pipeline_task", task_id = %task_id, runtime = %runtime, project_path = %project_path, dry_run = %dry_run);
    let _enter = span.enter();

    info!("Starting pipeline container");
    let rt: Box<dyn crate::runtimes::Runtime> = match runtime.as_str() {
        "docker" => {
            debug!("Using Docker runtime");
            Box::new(RuntimeFactory::docker())
        }
        _ => {
            debug!("Using PRoot runtime");
            Box::new(RuntimeFactory::proot().await)
        }
    };

    // Mount the project directory at /work inside the container.
    let mounts = vec![Mount {
        host_path: std::path::PathBuf::from(&project_path),
        container_path: "/work".to_string(),
    }];
    debug!(count = mounts.len(), "Configured container mounts");

    // Build args: /work -v [--dry-run]
    let mut args = vec!["/work".to_string(), "-v".to_string()];
    if dry_run {
        args.push("--dry-run".to_string());
        debug!("Added --dry-run flag to container arguments");
    }
    debug!(args = ?args, "Container arguments prepared");

    let mut handle = rt.run(&image_tag, &args, &mounts, &[]).await.map_err(|e| {
        error!(error = %e, "Failed to start pipeline container");
        anyhow::anyhow!("Failed to start pipeline container: {}", e)
    })?;

    // Store a kill function in the task registry so the process can be killed on cancellation
    {
        let task_id_kill = task_id.clone();
        // We need to capture the PID to kill the process and its children
        if let Some(pid) = handle.id() {
            debug!(pid = pid, "Registered process termination handler");
            TASK_REGISTRY.set_kill_fn(&task_id_kill, move || {
                info!(pid = pid, "Terminating pipeline process tree");
                kill_process_tree(pid as i32);
            });
        }
    }

    // Take stdout/stderr before spawning the log reader task.
    let mut stdout = handle.take_stdout();
    let mut stderr = handle.take_stderr();

    let task_id_log = task_id.clone();

    // Spawn a task that reads both streams and publishes task events.
    // We use the two-arm select pattern: when one stream ends it is replaced
    // with `pending()` so the other stream continues to be drained normally.
    let log_handle = tokio::spawn(async move {
        debug!("Log reader task started");
        let mut stdout_reader = stdout.is_some().then(LineReader::new);
        let mut stderr_reader = stderr.is_some().then(LineReader::new);

        // Mutable group-tracking state shared across both stream arms.
        // group_seq: next sequential index to assign on the next ::group marker.
        // current_group_idx: the index assigned to the currently open ::group.
        let mut group_seq: u32 = 0;
        let mut current_group_idx: u32 = 0;
        let mut current_stage_name: String = String::new();

        loop {
            tokio::select! {
                // ── stdout arm ───────────────────────────────────────────
                line = read_optional_line(&mut stdout_reader, &mut stdout) => {
                    match line {
                        Ok((Some(l), has_more)) => {
                            process_line(
                                &task_id_log,
                                &l,
                                &mut group_seq,
                                &mut current_group_idx,
                                &mut current_stage_name,
                            );
                            if !has_more {
                                stdout_reader = None;
                            }
                        }
                        Ok((None, _)) => {
                            stdout_reader = None;
                        }
                        Err(_) => {
                            stdout_reader = None;
                        }
                    }
                }

                // ── stderr arm ───────────────────────────────────────────
                line = read_optional_line(&mut stderr_reader, &mut stderr) => {
                    match line {
                        Ok((Some(l), has_more)) => {
                            process_line(
                                &task_id_log,
                                &l,
                                &mut group_seq,
                                &mut current_group_idx,
                                &mut current_stage_name,
                            );
                            if !has_more {
                                stderr_reader = None;
                            }
                        }
                        Ok((None, _)) => {
                            stderr_reader = None;
                        }
                        Err(_) => {
                            stderr_reader = None;
                        }
                    }
                }
            }

            if stdout_reader.is_none() && stderr_reader.is_none() {
                break;
            }
        }
        debug!("Log reader task completed");
    });

    // Wait for the container process to exit, then drain any remaining log output.
    debug!("Waiting for pipeline process to exit");
    let exit_status = handle.wait().await?;
    info!(exit_status = ?exit_status, "Pipeline process exited");

    debug!("Waiting for log reading to complete");
    log_handle.await.ok();
    info!("Log reading completed");

    if !dry_run && !exit_status.success() {
        let msg = format!("Pipeline failed with exit code {:?}", exit_status.code());
        error!("Pipeline execution failed");
        crate::task_registry::publish_event(&task_id, TaskEvent::Failed(msg.clone()));
        return Err(anyhow::anyhow!("{}", msg));
    }

    debug!("Fixing output file permissions");
    let _ = make_project_outputs_readable(&project_path).await;

    info!("Publishing pipeline completion event");
    crate::task_registry::publish_event(&task_id, TaskEvent::Completed);
    info!("Pipeline task completed successfully");
    Ok(())
}

async fn read_optional_line<R: AsyncRead + Unpin>(
    reader: &mut Option<LineReader>,
    stream: &mut Option<R>,
) -> std::io::Result<(Option<String>, bool)> {
    match (reader.as_mut(), stream.as_mut()) {
        (Some(reader), Some(stream)) => reader.read_line(stream).await,
        _ => std::future::pending().await,
    }
}

// ---------------------------------------------------------------------------
// Process group utilities
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Line processor
// ---------------------------------------------------------------------------

/// Parse one output line and publish the appropriate task event(s).
/// Get the process group ID for a given PID using ps command
///
/// `group_seq` is the next sequential index to assign to the next `::group` marker;
/// `current_group_idx` is the currently-open group's assigned index (for log lines).
fn process_line(
    task_id: &str,
    line: &str,
    group_seq: &mut u32,
    current_group_idx: &mut u32,
    current_stage_name: &mut String,
) {
    // ::remaining_groups::A,B,C,...
    if let Some(caps) = REMAINING_RE.captures(line) {
        let groups: Vec<String> = caps[1].split(',').map(|s| s.trim().to_string()).collect();
        crate::task_registry::publish_event(task_id, TaskEvent::PipelineRemainingGroups(groups));
        return;
    }

    // ::group <attrs>::<Stage Name>
    if let Some(caps) = GROUP_RE.captures(line) {
        let attr_str = &caps[1];
        let stage_name = caps[2].trim().to_string();

        let attrs = parse_attrs(attr_str);

        // Parse count=X/Y → pipeline_stage_num = X (1-based), total_stages = Y.
        // The stage_index is always the sequential group counter (not X-1), so that
        // Config and Tool Discovery groups get their own unique slots.
        let (pipeline_stage_num, total_stages) = if let Some(count) = attrs.get("count") {
            let parts: Vec<&str> = count.splitn(2, '/').collect();
            if parts.len() == 2 {
                let x = parts[0].trim().parse::<u32>().unwrap_or(1);
                let y = parts[1].trim().parse::<u32>().unwrap_or(1);
                (Some(x), y)
            } else {
                (None, 0)
            }
        } else {
            (None, 0)
        };

        let stage_status = match attrs.get("status").map(|s| s.as_str()) {
            Some("cached") => PipelineStageStatus::Cached,
            Some("skipped") => PipelineStageStatus::Skipped,
            _ => PipelineStageStatus::Run,
        };

        // Assign the current sequential index then advance the counter.
        let stage_index = *group_seq;
        *current_group_idx = stage_index;
        *group_seq += 1;
        *current_stage_name = stage_name.clone();

        crate::task_registry::publish_event(
            task_id,
            TaskEvent::PipelineStageStarted {
                stage_index,
                stage_name,
                total_stages,
                stage_status,
                pipeline_stage_num,
            },
        );
        return;
    }

    // ::endgroup::
    if ENDGROUP_RE.is_match(line) {
        crate::task_registry::publish_event(
            task_id,
            TaskEvent::PipelineStageCompleted {
                stage_index: *current_group_idx,
                stage_name: current_stage_name.clone(),
            },
        );
        return;
    }

    // Regular log line — always emit the raw text.
    crate::task_registry::publish_event(
        task_id,
        TaskEvent::PipelineLog {
            stage_index: *current_group_idx,
            stage_name: current_stage_name.clone(),
            line: line.to_string(),
        },
    );

    // Try to parse sub-progress from the line (fraction first, then percentage).
    if let Some(caps) = FRACTION_RE.captures(line) {
        if let (Ok(num), Ok(denom)) = (caps[1].parse::<f32>(), caps[2].parse::<f32>()) {
            if denom > 0.0 {
                crate::task_registry::publish_event(
                    task_id,
                    TaskEvent::PipelineStageProgress {
                        stage_index: *current_group_idx,
                        progress: (num / denom).clamp(0.0, 1.0),
                    },
                );
                return;
            }
        }
    }

    if let Some(caps) = PERCENT_RE.captures(line) {
        if let Ok(pct) = caps[1].parse::<f32>() {
            crate::task_registry::publish_event(
                task_id,
                TaskEvent::PipelineStageProgress {
                    stage_index: *current_group_idx,
                    progress: (pct / 100.0).clamp(0.0, 1.0),
                },
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Attribute parser
// ---------------------------------------------------------------------------

/// Parse a comma-separated key=value attribute string (as used in `::group`).
///
/// Example input: `file=/foo/bar.sh,type=stage,status=cached,count=3/12`
fn parse_attrs(attr_str: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for part in attr_str.split(',') {
        let mut it = part.splitn(2, '=');
        if let (Some(k), Some(v)) = (it.next(), it.next()) {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

// ---------------------------------------------------------------------------
// Permission fixing
// ---------------------------------------------------------------------------

/// Recursively make output files readable by ensuring they have user read permissions.
/// This helps fix permission issues when files are created in containers.
async fn make_project_outputs_readable(project_path: &str) -> anyhow::Result<()> {
    let path = std::path::PathBuf::from(project_path);
    if path.exists() {
        tokio::task::spawn_blocking({
            let path = path.clone();
            move || make_dir_readable_sync(&path)
        })
        .await
        .ok();
    }
    Ok(())
}

#[cfg(unix)]
fn make_dir_readable_sync(dir: &std::path::Path) {
    use std::collections::VecDeque;
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;

    let mut queue = VecDeque::new();
    queue.push_back(dir.to_path_buf());

    while let Some(current_dir) = queue.pop_front() {
        if let Ok(entries) = std::fs::read_dir(&current_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(meta) = std::fs::metadata(&path) {
                    let mode = meta.permissions().mode();
                    let new_mode = mode | 0o400;
                    let _ = std::fs::set_permissions(&path, Permissions::from_mode(new_mode));
                }
                if path.is_dir() {
                    queue.push_back(path);
                }
            }
        }
    }
}

#[cfg(not(unix))]
fn make_dir_readable_sync(_dir: &std::path::Path) {
    // No-op on non-Unix systems
}
