use crate::server::{cancel_task, get_task_info, list_tasks, poll_task_events, run_pipeline};
use crate::task_manager::{apply_event_to_entry, TasksCtx};
use crate::views::project::{PipelineCommandCtx, PipelineIsRunningCtx, PipelineProgressCtx};
use colmap_openmvs_api::TaskKind;
use colmap_openmvs_api::{PipelineStageStatus, TaskEvent, TaskState};
use dioxus::document::eval;
use dioxus::prelude::*;

const POLL_INTERVAL_MS: u64 = 300;

async fn sleep_poll() {
    #[cfg(not(target_arch = "wasm32"))]
    tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
}

// ---------------------------------------------------------------------------
// Data model for UI state
// ---------------------------------------------------------------------------

/// A single pipeline stage with its log lines and latest sub-progress.
#[derive(Clone, Debug, PartialEq)]
struct StageData {
    index: u32,
    name: String,
    total_stages: u32,
    lines: Vec<String>,
    /// 0.0..=1.0 sub-progress; None = unknown
    progress: Option<f32>,
    /// Stage has finished (either by completing fresh or being loaded from cache)
    completed: bool,
    /// Stage output was served from cache (previous run)
    cached: bool,
    /// Stage was explicitly skipped (--skip flag)
    skipped: bool,
    /// Stage is currently being executed
    is_running: bool,
    /// 1-based pipeline stage number (X from count=X/Y); None for Config/Tool Discovery groups
    pipeline_stage_num: Option<u32>,
}

impl StageData {
    fn new(index: u32, name: String, total_stages: u32) -> Self {
        Self {
            index,
            name,
            total_stages,
            lines: Vec::new(),
            progress: None,
            completed: false,
            cached: false,
            skipped: false,
            is_running: false,
            pipeline_stage_num: None,
        }
    }

    /// Process a log line that may contain carriage returns (\r).
    /// Carriage returns indicate progress updates that should replace the previous line.
    /// If the line contains \r, the part after the last \r replaces the previous line.
    /// If there's no previous line and the line starts with \r, it's added as a new line.
    fn add_log_line(&mut self, line: String) {
        if line.contains('\r') {
            // Split by \r and process each part
            let parts: Vec<&str> = line.split('\r').collect();

            for (idx, part) in parts.iter().enumerate() {
                let len = self.lines.len();
                match idx {
                    0 => {
                        // First part: if there's a previous line, update it; otherwise add new line
                        if !part.is_empty() {
                            if len > 0 {
                                self.lines[len - 1] = part.to_string();
                            } else {
                                self.lines.push(part.to_string());
                            }
                        }
                    }
                    _ if idx == parts.len() - 1 => {
                        // Last part: replace previous line if non-empty, otherwise add new
                        if !part.is_empty() {
                            let len = self.lines.len();
                            if len > 0 {
                                self.lines[len - 1] = part.to_string();
                            } else {
                                self.lines.push(part.to_string());
                            }
                        }
                        // If part is empty, line ends with \r and we just replaced the previous line
                    }
                    _ => {
                        // Middle parts: always replace the previous line
                        if !part.is_empty() {
                            let len = self.lines.len();
                            if len > 0 {
                                self.lines[len - 1] = part.to_string();
                            } else {
                                self.lines.push(part.to_string());
                            }
                        }
                    }
                }
            }
        } else {
            // No carriage return, just add the line normally
            self.lines.push(line);
        }
    }
}

#[cfg(test)]
mod stage_data_tests {
    use super::StageData;

    #[test]
    fn test_add_log_line_without_carriage_return() {
        let mut stage = StageData::new(0, "test".to_string(), 1);
        stage.add_log_line("Line 1".to_string());
        stage.add_log_line("Line 2".to_string());
        assert_eq!(stage.lines, vec!["Line 1", "Line 2"]);
    }

    #[test]
    fn test_add_log_line_with_carriage_return_replaces_last_line() {
        let mut stage = StageData::new(0, "test".to_string(), 1);
        stage.add_log_line("Progress: 0%".to_string());
        stage.add_log_line("Progress: 50%\r".to_string()); // \r at end replaces last line
        stage.add_log_line("Progress: 100%\r".to_string()); // Again replaces

        // Should have one line from first add, then last progress update
        assert_eq!(stage.lines.len(), 1);
        assert_eq!(stage.lines[0], "Progress: 100%");
    }

    #[test]
    fn test_add_log_line_with_multiple_carriage_returns() {
        let mut stage = StageData::new(0, "test".to_string(), 1);
        stage.add_log_line("First line".to_string());
        // Three parts separated by \r: "Progress A" -> "Progress B" -> "Progress C"
        stage.add_log_line("Progress A\rProgress B\rProgress C".to_string());

        // Should have only the last progress update
        assert_eq!(stage.lines.len(), 1);
        assert_eq!(stage.lines[0], "Progress C");
    }

    #[test]
    fn test_add_log_line_carriage_return_at_start() {
        let mut stage = StageData::new(0, "test".to_string(), 1);
        stage.add_log_line("Initial line".to_string());
        stage.add_log_line("\rReplaced line".to_string()); // \r at start

        assert_eq!(stage.lines.len(), 1);
        assert_eq!(stage.lines[0], "Replaced line");
    }

    #[test]
    fn test_add_log_line_progress_bar_simulation() {
        // Simulate a typical progress bar: "[===>    ]" updates
        let mut stage = StageData::new(0, "test".to_string(), 1);
        stage.add_log_line("Starting task...".to_string());
        stage.add_log_line("Progress [=>       ]\r".to_string());
        stage.add_log_line("Progress [==>      ]\r".to_string());
        stage.add_log_line("Progress [===>     ]\r".to_string());
        stage.add_log_line("Progress [====>    ]\r".to_string());
        stage.add_log_line("Task completed!".to_string()); // New line without \r

        assert_eq!(stage.lines.len(), 2);
        assert_eq!(stage.lines[0], "Progress [====>    ]"); // Last progress bar state
        assert_eq!(stage.lines[1], "Task completed!");
    }

    #[test]
    fn test_add_log_line_empty_parts_between_carriage_returns() {
        let mut stage = StageData::new(0, "test".to_string(), 1);
        stage.add_log_line("First".to_string());
        stage.add_log_line("A\r\rC".to_string()); // Empty part between \r characters

        assert_eq!(stage.lines.len(), 1);
        assert_eq!(stage.lines[0], "C");
    }
}

/// High-level pipeline state driven by TaskEvents.
#[derive(Clone, Debug, PartialEq)]
pub enum PipelineStatus {
    Idle,
    Running,
    Completed,
    Failed(String),
}

// ---------------------------------------------------------------------------
// Helper: find a running task of a given kind for this project
// ---------------------------------------------------------------------------

async fn find_running_task(project_name: &str, recover_logs: bool) -> Option<String> {
    let kind_filter = if recover_logs {
        TaskKind::RecoverPipelineLogs
    } else {
        TaskKind::RunPipeline
    };

    let candidate_id = list_tasks(Some(kind_filter), Some(project_name.to_string()))
        .await
        .ok()
        .and_then(|tasks| {
            tasks
                .into_iter()
                .find(|t| t.state == TaskState::Running)
                .map(|t| t.id)
        });

    if let Some(id) = candidate_id {
        if let Ok(Some(info)) = get_task_info(id.clone()).await {
            if info.state == TaskState::Running {
                return Some(id);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Pipeline stream driver
// ---------------------------------------------------------------------------

/// Batched update result from processing one poll batch.
/// Holds all state locally so we can flush signals exactly once
/// instead of after every individual event.
struct BatchUpdate {
    stages: Vec<StageData>,
    pipeline_status: PipelineStatus,
    expanded_stage: Option<u32>,
    pipeline_progress: Option<f32>,
    active_task_id: String,
}

#[allow(clippy::too_many_arguments)]
fn spawn_pipeline_stream(
    task_id: String,
    mut stages: Signal<Vec<StageData>>,
    mut pipeline_status: Signal<PipelineStatus>,
    _auto_scroll: Signal<bool>,
    mut active_task_id: Signal<String>,
    mut expanded_stage: Signal<Option<u32>>,
    mut pipeline_progress_ctx: Signal<Option<f32>>,
    mut error_msg: Signal<String>,
    is_recover_logs: bool,
    mut tasks_ctx: TasksCtx,
    mut recovering: Signal<bool>,
) {
    spawn(async move {
        let mut cursor = 0usize;
        let mut is_first_batch = true;

        loop {
            // Use pagination so the initial recovery doesn't download
            // & process tens of thousands of events in one response.
            match poll_task_events(task_id.clone(), cursor, Some(500)).await {
                Ok(batch) => {
                    if !batch.task_found {
                        // Task evicted from registry; treat as completion.
                        pipeline_status.set(PipelineStatus::Idle);
                        active_task_id.set(String::new());
                        break;
                    }

                    let new_cursor = batch.cursor;
                    let is_terminal = batch.is_terminal;
                    let mut local_pos = cursor;

                    // ── Phase 1: process all events into local state ────
                    // We snapshot the current signal values, process events
                    // against the snapshots, then flush once at the end.
                    // This avoids O(n) signal writes & re-renders per event.
                    let mut update = BatchUpdate {
                        stages: stages.peek().clone(),
                        pipeline_status: pipeline_status.peek().clone(),
                        expanded_stage: expanded_stage.peek().clone(),
                        pipeline_progress: pipeline_progress_ctx.peek().clone(),
                        active_task_id: active_task_id.peek().clone(),
                    };
                    let mut should_terminate = false;

                    for event in batch.events {
                        local_pos += 1;

                        // Update global TasksCtx (batched in one write).
                        {
                            let mut state = tasks_ctx.write();
                            if let Some(entry) = state.tasks.iter_mut().find(|t| t.id == task_id) {
                                apply_event_to_entry(entry, &event, local_pos);
                            }
                        }

                        match event {
                            TaskEvent::PipelineRemainingGroups(names) => {
                                // Filter out empty names (safety against leading comma in pipeline output)
                                let names: Vec<String> =
                                    names.into_iter().filter(|n| !n.is_empty()).collect();
                                let total = names.len() as u32;
                                if update.stages.is_empty() {
                                    for (i, name) in names.into_iter().enumerate() {
                                        update.stages.push(StageData {
                                            index: i as u32,
                                            name,
                                            total_stages: total,
                                            lines: vec![],
                                            progress: None,
                                            completed: false,
                                            cached: false,
                                            skipped: false,
                                            is_running: false,
                                            pipeline_stage_num: None,
                                        });
                                    }
                                } else {
                                    // PipelineRemainingGroups can arrive *after* some stages
                                    // have already started. Add any not-yet-seen stage names
                                    // as placeholder entries so the user can see upcoming stages.
                                    // Matching by name is safe because names are unique and
                                    // the list is always in sequential group order.
                                    for name in names {
                                        if !update.stages.iter().any(|s| s.name == name) {
                                            let idx = update.stages.len() as u32;
                                            update.stages.push(StageData {
                                                index: idx,
                                                name,
                                                total_stages: 0,
                                                lines: vec![],
                                                progress: None,
                                                completed: false,
                                                cached: false,
                                                skipped: false,
                                                is_running: false,
                                                pipeline_stage_num: None,
                                            });
                                        }
                                    }
                                }
                            }
                            TaskEvent::PipelineStageStarted {
                                stage_index,
                                stage_name,
                                total_stages,
                                stage_status,
                                pipeline_stage_num,
                            } => {
                                while update.stages.len() <= stage_index as usize {
                                    let idx = update.stages.len() as u32;
                                    update.stages.push(StageData::new(
                                        idx,
                                        String::new(),
                                        total_stages,
                                    ));
                                }
                                let cached = matches!(stage_status, PipelineStageStatus::Cached);
                                let skipped = matches!(stage_status, PipelineStageStatus::Skipped);
                                let actually_running = !cached && !skipped && !is_recover_logs;
                                {
                                    let stage = &mut update.stages[stage_index as usize];
                                    stage.name = stage_name.clone();
                                    if total_stages > 0 {
                                        stage.total_stages = total_stages;
                                    }
                                    stage.cached = cached;
                                    stage.skipped = skipped;
                                    stage.completed = cached || skipped;
                                    stage.is_running = actually_running;
                                    stage.pipeline_stage_num = pipeline_stage_num;
                                }
                                // Only auto-expand stages during live updates
                                // (after initial recovery), not during recovery.
                                if actually_running
                                    && pipeline_stage_num.is_some()
                                    && !is_first_batch
                                {
                                    update.expanded_stage = Some(stage_index);
                                }
                                if cached || skipped {
                                    let total_p = update
                                        .stages
                                        .iter()
                                        .filter(|x| x.pipeline_stage_num.is_some())
                                        .count()
                                        as f32;
                                    let done_p = update
                                        .stages
                                        .iter()
                                        .filter(|x| x.pipeline_stage_num.is_some() && x.completed)
                                        .count()
                                        as f32;
                                    if total_p > 0.0 {
                                        update.pipeline_progress =
                                            Some((done_p / total_p).clamp(0.0, 1.0));
                                    }
                                }
                            }
                            TaskEvent::PipelineLog {
                                stage_index,
                                stage_name,
                                line,
                            } => {
                                while update.stages.len() <= stage_index as usize {
                                    let idx = update.stages.len() as u32;
                                    let total =
                                        update.stages.last().map(|x| x.total_stages).unwrap_or(1);
                                    update.stages.push(StageData::new(
                                        idx,
                                        stage_name.clone(),
                                        total,
                                    ));
                                }
                                update.stages[stage_index as usize].add_log_line(line);
                                // Auto-scroll is handled by a persistent watcher effect below.
                            }
                            TaskEvent::PipelineStageProgress {
                                stage_index,
                                progress,
                            } => {
                                if let Some(stage) = update.stages.get_mut(stage_index as usize) {
                                    if stage.pipeline_stage_num.is_some() {
                                        stage.progress = Some(progress);
                                    }
                                }
                                let total_p = update
                                    .stages
                                    .iter()
                                    .filter(|x| x.pipeline_stage_num.is_some())
                                    .count() as f32;
                                if total_p > 0.0 {
                                    let done_p = update
                                        .stages
                                        .iter()
                                        .filter(|x| x.pipeline_stage_num.is_some() && x.completed)
                                        .count()
                                        as f32;
                                    let sub = progress / total_p;
                                    update.pipeline_progress =
                                        Some((done_p / total_p + sub).clamp(0.0, 1.0));
                                }
                            }
                            TaskEvent::PipelineStageCompleted { stage_index, .. } => {
                                if let Some(stage) = update.stages.get_mut(stage_index as usize) {
                                    if !is_recover_logs || stage.cached || stage.skipped {
                                        stage.completed = true;
                                        if stage.pipeline_stage_num.is_some() {
                                            stage.progress = Some(1.0);
                                        }
                                    }
                                    stage.is_running = false;
                                }
                                let total_p = update
                                    .stages
                                    .iter()
                                    .filter(|x| x.pipeline_stage_num.is_some())
                                    .count() as f32;
                                if total_p > 0.0 {
                                    let done_p = update
                                        .stages
                                        .iter()
                                        .filter(|x| x.pipeline_stage_num.is_some() && x.completed)
                                        .count()
                                        as f32;
                                    update.pipeline_progress =
                                        Some((done_p / total_p).clamp(0.0, 1.0));
                                }
                            }
                            TaskEvent::Completed => {
                                if is_recover_logs {
                                    update.pipeline_status = PipelineStatus::Idle;
                                } else {
                                    update.pipeline_status = PipelineStatus::Completed;
                                    update.pipeline_progress = Some(1.0);
                                }
                                update.active_task_id = String::new();
                                should_terminate = true;
                            }
                            TaskEvent::Failed(msg) => {
                                if is_recover_logs {
                                    update.pipeline_status = PipelineStatus::Idle;
                                } else {
                                    update.pipeline_status = PipelineStatus::Failed(msg);
                                }
                                update.active_task_id = String::new();
                                update.pipeline_progress = None;
                                should_terminate = true;
                            }
                            _ => {}
                        }
                    }

                    // ── Phase 2: flush all signals exactly once ──────────
                    stages.set(update.stages);
                    pipeline_status.set(update.pipeline_status);
                    expanded_stage.set(update.expanded_stage);
                    pipeline_progress_ctx.set(update.pipeline_progress);
                    // Always flush active_task_id so Completed/Failed can
                    // clear it.  Dioxus's PartialEq check skips unnecessary
                    // re-renders when the value hasn't changed.
                    active_task_id.set(update.active_task_id);

                    // ── Clear recovery indicator after first batch ───────
                    if is_first_batch {
                        is_first_batch = false;
                        recovering.set(false);
                    }

                    cursor = new_cursor;

                    if should_terminate || is_terminal {
                        break;
                    }
                }
                Err(e) => {
                    error_msg.set(format!("Failed to poll pipeline events: {e}"));
                    sleep_poll().await;
                }
            }

            sleep_poll().await;
        }
    });
}

// ---------------------------------------------------------------------------
// Main component
// ---------------------------------------------------------------------------

#[component]
pub fn LogsTab(project_name: String) -> Element {
    let mut stages = use_signal(Vec::<StageData>::new);
    let mut pipeline_status = use_signal(|| PipelineStatus::Idle);
    let mut error_msg = use_signal(String::new);
    let mut active_task_id = use_signal(String::new);
    let mut expanded_stage = use_signal(|| Option::<u32>::None);
    let mut auto_scroll = use_signal(|| true);
    let mut is_current_recover_logs = use_signal(|| false);
    // `true` while the tab is reconnecting to a running pipeline and
    // replaying its event log.  UI shows a spinner + message instead
    // of an empty placeholder during this phase.
    let mut recovering = use_signal(|| false);

    let mut pipeline_progress_ctx = use_context::<PipelineProgressCtx>();
    let mut pipeline_is_running = use_context::<PipelineIsRunningCtx>();
    let mut pipeline_command = use_context::<PipelineCommandCtx>();
    let mut tasks_ctx = use_context::<TasksCtx>();

    // ── Keep PipelineIsRunningCtx in sync with local status ──────────────
    use_effect(move || {
        let running = matches!(pipeline_status(), PipelineStatus::Running);
        pipeline_is_running.set(running);
    });

    // ── Persistent auto-scroll watcher ───────────────────────────────────
    // A single interval continuously scrolls the log view while auto-scroll
    // is enabled AND the pipeline is running.  Uses use_drop to clean up
    // on unmount, and toggles the interval on/off via a sentinel value.
    let mut auto_scroll_running = use_signal(|| false);
    use_effect(move || {
        let should_run = auto_scroll() && matches!(pipeline_status(), PipelineStatus::Running);
        if should_run == auto_scroll_running() {
            return; // no change
        }
        auto_scroll_running.set(should_run);
        tracing::debug!(should_run = should_run, "Auto-scroll watcher state change");
        if should_run {
            let _ = eval(
                r#"
                if (!window.__logsAutoScrollId) {
                    console.log('[auto-scroll] Starting interval');
                    window.__logsAutoScrollId = setInterval(function () {
                        var el = document.getElementById('logs-output');
                        if (!el) return;
                        var stage = el.closest('.logs-stage');
                        if (!stage) return;
                        // Smooth-scroll inner log output to bottom
                        el.scroll({ top: el.scrollHeight, behavior: 'smooth' });
                        // Smooth-scroll to bring the stage into view in ancestor containers
                        stage.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
                        // Show/hide floating button based on deepest scroll depth
                        var maxScroll = 0;
                        for (var p = stage.parentElement; p; p = p.parentElement) {
                            if (p.scrollHeight > p.clientHeight) {
                                maxScroll = Math.max(maxScroll, p.scrollTop);
                            }
                        }
                        var floatBtn = document.querySelector('.logs-autoscroll-float');
                        if (floatBtn) {
                            floatBtn.classList.toggle('logs-autoscroll-float--hidden', maxScroll < 100);
                        }
                    }, 100);
                }
                "#,
            );
        } else {
            // One last scroll to the bottom before stopping.
            let _ = eval(concat!(
                "console.log('[auto-scroll] One last scroll then stop'); ",
                "var el = document.getElementById('logs-output'); ",
                "if (el) { el.scroll({ top: el.scrollHeight, behavior: 'smooth' }); } ",
                "var stage = document.querySelector('.logs-stage'); ",
                "if (stage) { stage.scrollIntoView({ block: 'nearest', behavior: 'smooth' }); } ",
                "clearInterval(window.__logsAutoScrollId); ",
                "window.__logsAutoScrollId = null;",
            ));
        }
    });
    // Always clean up on unmount (last scroll + clear).
    use_drop(move || {
        let _ = eval(concat!(
            "var el = document.getElementById('logs-output'); ",
            "if (el) { el.scrollTop = el.scrollHeight; } ",
            "var container = document.querySelector('.logs-stages'); ",
            "if (container) { container.scrollTop = container.scrollHeight; } ",
            "clearInterval(window.__logsAutoScrollId); ",
            "window.__logsAutoScrollId = null;",
        ));
    });

    // ── On mount: reconnect or auto-start recover-logs ─────────────────────────
    //
    // We intentionally read NO signals in the synchronous closure body so
    // this effect runs exactly once (on mount).  All signal reads happen
    // inside the spawned async task where they do not create subscriptions.
    let project_name_mount = project_name.clone();
    use_effect(move || {
        let project_name = project_name_mount.clone();
        spawn(async move {
            // If the command watcher already set Running (e.g. user clicked
            // Run before this tab was mounted), skip the auto recover-logs so we
            // don't race with the full run that the command watcher starts.
            if matches!(pipeline_status.peek().clone(), PipelineStatus::Running) {
                return;
            }

            // 1. Reconnect to a running full pipeline.
            if let Some(task_id) = find_running_task(&project_name, false).await {
                active_task_id.set(task_id.clone());
                pipeline_status.set(PipelineStatus::Running);
                is_current_recover_logs.set(false);
                tasks_ctx.write().register(
                    task_id.clone(),
                    format!("Pipeline: {}", project_name),
                    TaskKind::RunPipeline,
                );
                recovering.set(true);
                spawn_pipeline_stream(
                    task_id,
                    stages,
                    pipeline_status,
                    auto_scroll,
                    active_task_id,
                    expanded_stage,
                    pipeline_progress_ctx,
                    error_msg,
                    false,
                    tasks_ctx,
                    recovering,
                );
                return;
            }

            // 2. Reconnect to a running recover-logs.
            if let Some(task_id) = find_running_task(&project_name, true).await {
                active_task_id.set(task_id.clone());
                pipeline_status.set(PipelineStatus::Running);
                is_current_recover_logs.set(true);
                tasks_ctx.write().register(
                    task_id.clone(),
                    format!("Recover-logs: {}", project_name),
                    TaskKind::RecoverPipelineLogs,
                );
                recovering.set(true);
                spawn_pipeline_stream(
                    task_id,
                    stages,
                    pipeline_status,
                    auto_scroll,
                    active_task_id,
                    expanded_stage,
                    pipeline_progress_ctx,
                    error_msg,
                    true,
                    tasks_ctx,
                    recovering,
                );
                return;
            }

            // 3. Auto-start a recover-logs to show cached/pending stage status.
            match run_pipeline(project_name.clone(), true).await {
                Ok(task_id) => {
                    active_task_id.set(task_id.clone());
                    pipeline_status.set(PipelineStatus::Running);
                    is_current_recover_logs.set(true);
                    tasks_ctx.write().register(
                        task_id.clone(),
                        format!("Recover-logs: {}", project_name),
                        TaskKind::RecoverPipelineLogs,
                    );
                    recovering.set(true);
                    spawn_pipeline_stream(
                        task_id,
                        stages,
                        pipeline_status,
                        auto_scroll,
                        active_task_id,
                        expanded_stage,
                        pipeline_progress_ctx,
                        error_msg,
                        true,
                        tasks_ctx,
                        recovering,
                    );
                }
                Err(_) => {
                    // Silently ignore — project may have no images yet.
                }
            }
        });
    });

    // ── Watch PipelineCommandCtx for start/cancel commands ────────────────
    //
    // We set pipeline_status = Running synchronously (before the async spawn)
    // so that the mount effect's async task sees Running and bails out if it
    // somehow runs concurrently on first mount with a pending command.
    let project_name_cmd = project_name.clone();
    use_effect(move || {
        let cmd = pipeline_command();
        let is_running = matches!(pipeline_status(), PipelineStatus::Running);

        if cmd == Some(true) && !is_running {
            // Consume the command immediately.
            pipeline_command.set(None);
            // Mark as running synchronously so the mount-effect task won't
            // race and also start a recover-logs.
            pipeline_status.set(PipelineStatus::Running);
            // Reset runtime state but preserve cached/skipped flags from recover-logs.
            // This allows us to recover from a recover-logs and start the real pipeline
            // while keeping the cached stage information, so cached stages will be
            // marked as done immediately when their PipelineStageStarted event arrives
            // with status=cached.
            let mut s = stages.write();
            for stage in s.iter_mut() {
                stage.completed = false; // Will be re-set by real pipeline events
                stage.is_running = false; // No stage is running yet
                stage.progress = None; // Clear old progress
                stage.lines.clear(); // Clear old logs
                                     // NOTE: cached and skipped flags are preserved!
            }
            drop(s);
            error_msg.set(String::new());
            pipeline_progress_ctx.set(Some(0.0));
            is_current_recover_logs.set(false);

            let project_name = project_name_cmd.clone();
            spawn(async move {
                match run_pipeline(project_name.clone(), false).await {
                    Ok(task_id) => {
                        active_task_id.set(task_id.clone());
                        tasks_ctx.write().register(
                            task_id.clone(),
                            format!("Pipeline: {}", project_name),
                            TaskKind::RunPipeline,
                        );
                        spawn_pipeline_stream(
                            task_id,
                            stages,
                            pipeline_status,
                            auto_scroll,
                            active_task_id,
                            expanded_stage,
                            pipeline_progress_ctx,
                            error_msg,
                            false,
                            tasks_ctx,
                            recovering,
                        );
                    }
                    Err(e) => {
                        error_msg.set(format!("Failed to start pipeline: {e}"));
                        pipeline_status.set(PipelineStatus::Idle);
                        pipeline_progress_ctx.set(None);
                    }
                }
            });
        } else if cmd == Some(false) && is_running {
            pipeline_command.set(None);
            let task_id = active_task_id();
            if !task_id.is_empty() {
                spawn(async move {
                    let _ = cancel_task(task_id).await;
                });
            }
        }
    });

    // ── Snapshot for render ───────────────────────────────────────────────
    let stages_snapshot = stages();
    let is_recovering = recovering();

    rsx! {
        div {
            class: "tab-content logs-tab",

            // ── Toolbar ──────────────────────────────────────────────────
            div {
                class: "logs-toolbar",

                // Status badge
                match pipeline_status() {
                    PipelineStatus::Idle => rsx! {},
                    PipelineStatus::Running => rsx! {
                        span {
                            class: "logs-status logs-status-running",
                            if is_current_recover_logs() { "● Checking…" } else { "● Running" }
                        }
                    },
                    PipelineStatus::Completed => rsx! {
                        span { class: "logs-status logs-status-done", "✓ Completed" }
                    },
                    PipelineStatus::Failed(msg) => rsx! {
                        span {
                            class: "logs-status logs-status-failed",
                            title: "{msg}",
                            "✗ Failed"
                        }
                    },
                }

                // Auto-scroll toggle (always visible in toolbar)
                label {
                    class: "logs-autoscroll-label",
                    title: "Automatically scroll to the latest log line",
                    input {
                        r#type: "checkbox",
                        checked: auto_scroll(),
                        onchange: move |e| auto_scroll.set(e.checked()),
                    }
                    " Auto-scroll"
                }
            }

            // ── Error banner ─────────────────────────────────────────────
            if !error_msg().is_empty() {
                div {
                    class: "logs-error-banner",
                    span { "{error_msg}" }
                    button {
                        class: "logs-error-dismiss",
                        onclick: move |_| error_msg.set(String::new()),
                        "✕"
                    }
                }
            }

            // ── Recovery / placeholder / stages ────────────────────────────
            if is_recovering {
                div {
                    class: "logs-recovering",
                    div { class: "logs-recovering-spinner", "⟳" }
                    span { "Reconnecting to pipeline…" }
                }
            } else if stages_snapshot.is_empty() {
                div {
                    class: "logs-placeholder",
                    div { class: "spinner" }
                    span { "Loading..." }
                }
            } else {
                div {
                    class: "logs-stages",
                    for stage in stages_snapshot.iter() {
                        {
                            let is_expanded = expanded_stage() == Some(stage.index);
                            let stage_idx = stage.index;
                            let stage_pct = stage.progress.map(|p| p * 100.0).unwrap_or(0.0);
                            let line_count = stage.lines.len();

                            // Determine display icon and CSS class.
                            let (status_icon, stage_status_class) = if stage.completed && stage.cached {
                                ("✓", "stage-status-cached")
                            } else if stage.completed && stage.skipped {
                                ("⊘", "stage-status-skipped")
                            } else if stage.completed {
                                ("✓", "stage-status-done")
                            } else if stage.is_running {
                                ("⟳", "stage-status-running")
                            } else {
                                ("○", "stage-status-pending")
                            };

                            // Suffix shown next to stage name.
                            let name_suffix = if stage.cached { " (cached)" } else if stage.skipped { " (skipped)" } else { "" };

                            rsx! {
                                div {
                                    key: "{stage_idx}",
                                    class: "logs-stage",

                                    // Stage header (click to expand/collapse)
                                    div {
                                        class: "logs-stage-header",
                                        onclick: move |_| {
                                            if expanded_stage() == Some(stage_idx) {
                                                expanded_stage.set(None);
                                            } else {
                                                expanded_stage.set(Some(stage_idx));
                                            }
                                        },
                                        span {
                                            class: "stage-chevron",
                                            if is_expanded { "▾" } else { "▸" }
                                        }
                                        if let Some(num) = stage.pipeline_stage_num {
                                            span {
                                                class: "stage-index",
                                                "{num}/{stage.total_stages}"
                                            }
                                        }
                                        span {
                                            class: "stage-name",
                                            "{stage.name}{name_suffix}"
                                        }
                                        span {
                                            class: "stage-line-count",
                                            "({line_count} lines)"
                                        }
                                        // Per-stage sub-progress bar
                                        if stage.progress.is_some() && !stage.completed {
                                            div {
                                                class: "stage-progress-track",
                                                div {
                                                    class: "stage-progress-fill",
                                                    style: "width: {stage_pct:.1}%",
                                                }
                                            }
                                            span {
                                                class: "stage-progress-label",
                                                "{stage_pct as u32}%"
                                            }
                                        }
                                        span {
                                            class: "stage-status-dot {stage_status_class}",
                                            "{status_icon}"
                                        }
                                    }

                                    // Log lines (collapsible)
                                    if is_expanded && !stage.lines.is_empty() {
                                        div {
                                            class: "logs-output",
                                            id: if is_expanded { "logs-output" } else { "" },
                                            pre {
                                                class: "logs-pre",
                                                for line in stage.lines.iter() {
                                                    span {
                                                        class: "log-line",
                                                        "{line}\n"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── Floating indicator: only while auto-scroll is ON AND pipeline is running ──
            if auto_scroll() && matches!(pipeline_status(), PipelineStatus::Running) {
                label {
                    class: "logs-autoscroll-float",
                    title: "Auto-scroll is ON — click to disable",
                    input {
                        r#type: "checkbox",
                        checked: true,
                        onchange: move |e| auto_scroll.set(e.checked()),
                    }
                    span { class: "logs-autoscroll-float-icon", "▼" }
                    span { class: "logs-autoscroll-float-text", "Scrolling… (click to disable auto-scroll)" }
                }
            }
        }
    }
}
