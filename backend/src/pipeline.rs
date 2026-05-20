//! Pipeline execution: runs the colmap-openmvs container pipeline and streams events.

use anyhow::Result;
use colmap_openmvs_api::{PipelineStageStatus, TaskEvent, TaskKind};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::{
    runtimes::{Mount, Runtime, RuntimeFactory},
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
pub async fn run_pipeline(project_name: String, dry_run: bool) -> Result<String> {
    let settings = get_settings().await.map_err(|e| anyhow::anyhow!("{}", e))?;
    let image_tag = settings
        .default_image_tag
        .ok_or_else(|| anyhow::anyhow!("No default image configured"))?;

    let project_path = {
        let projects = crate::projects::get_projects()
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        projects
            .into_iter()
            .find(|p| p.name == project_name)
            .map(|p| p.path)
            .ok_or_else(|| anyhow::anyhow!("Project not found: {}", project_name))?
    };

    let task_kind = if dry_run {
        TaskKind::DryRunPipeline
    } else {
        TaskKind::RunPipeline
    };

    let task_id = {
        let mut registry = TASK_REGISTRY.lock().unwrap();
        registry.create_task(task_kind, project_name.clone())
    };

    let task_id_clone = task_id.clone();
    tokio::spawn(async move {
        if let Err(e) =
            run_pipeline_task(task_id_clone.clone(), image_tag, project_path, dry_run).await
        {
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
    image_tag: String,
    project_path: String,
    dry_run: bool,
) -> Result<()> {
    let rt = RuntimeFactory::proot();

    // Mount the project directory at /work inside the container.
    let mounts = vec![Mount {
        host_path: std::path::PathBuf::from(&project_path),
        container_path: "/work".to_string(),
    }];

    // Build args: /work -v [--dry-run]
    let mut args = vec!["/work".to_string(), "-v".to_string()];
    if dry_run {
        args.push("--dry-run".to_string());
    }

    let mut handle = rt
        .run(&image_tag, &args, &mounts)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start pipeline container: {}", e))?;

    // Take stdout/stderr before spawning the log reader task.
    let stdout = handle.take_stdout().map(BufReader::new);
    let stderr = handle.take_stderr().map(BufReader::new);

    let task_id_log = task_id.clone();

    // Spawn a task that reads both streams and publishes task events.
    // We use the two-arm select pattern: when one stream ends it is replaced
    // with `pending()` so the other stream continues to be drained normally.
    let log_handle = tokio::spawn(async move {
        let mut stdout_lines = stdout.map(|r| r.lines());
        let mut stderr_lines = stderr.map(|r| r.lines());

        // Mutable group-tracking state shared across both stream arms.
        // group_seq: next sequential index to assign on the next ::group marker.
        // current_group_idx: the index assigned to the currently open ::group.
        let mut group_seq: u32 = 0;
        let mut current_group_idx: u32 = 0;
        let mut current_stage_name: String = String::new();

        loop {
            tokio::select! {
                // ── stdout arm ───────────────────────────────────────────
                line = async {
                    if let Some(lines) = stdout_lines.as_mut() {
                        lines.next_line().await
                    } else {
                        std::future::pending::<std::io::Result<Option<String>>>().await
                    }
                } => {
                    match line {
                        Ok(Some(l)) => {
                            process_line(
                                &task_id_log,
                                &l,
                                &mut group_seq,
                                &mut current_group_idx,
                                &mut current_stage_name,
                            );
                        }
                        _ => {
                            stdout_lines = None;
                        }
                    }
                }

                // ── stderr arm ───────────────────────────────────────────
                line = async {
                    if let Some(lines) = stderr_lines.as_mut() {
                        lines.next_line().await
                    } else {
                        std::future::pending::<std::io::Result<Option<String>>>().await
                    }
                } => {
                    match line {
                        Ok(Some(l)) => {
                            process_line(
                                &task_id_log,
                                &l,
                                &mut group_seq,
                                &mut current_group_idx,
                                &mut current_stage_name,
                            );
                        }
                        _ => {
                            stderr_lines = None;
                        }
                    }
                }
            }

            if stdout_lines.is_none() && stderr_lines.is_none() {
                break;
            }
        }
    });

    // Wait for the container process to exit, then drain any remaining log output.
    let exit_status = handle.wait().await?;
    log_handle.await.ok();

    if !dry_run && !exit_status.success() {
        let msg = format!("Pipeline failed with exit code {:?}", exit_status.code());
        crate::task_registry::publish_event(&task_id, TaskEvent::Failed(msg.clone()));
        return Err(anyhow::anyhow!("{}", msg));
    }

    let _ = make_project_outputs_readable(&project_path).await;
    crate::task_registry::publish_event(&task_id, TaskEvent::Completed);
    Ok(())
}

// ---------------------------------------------------------------------------
// Line processor
// ---------------------------------------------------------------------------

/// Parse one output line and publish the appropriate task event(s).
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
