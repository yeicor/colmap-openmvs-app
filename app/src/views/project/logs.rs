use crate::server::{cancel_task, get_task_info, list_tasks, run_pipeline, subscribe_task_events};
use crate::task_manager::{apply_event_to_entry, TasksCtx};
use crate::views::project::{PipelineCommandCtx, PipelineIsRunningCtx, PipelineProgressCtx};
use colmap_openmvs_api::TaskKind;
use colmap_openmvs_api::{PipelineStageStatus, TaskEvent, TaskState};
use dioxus::document::eval;
use dioxus::prelude::*;

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

async fn find_running_task(project_name: &str, dry_run: bool) -> Option<String> {
    let kind_str = if dry_run {
        "DryRunPipeline"
    } else {
        "RunPipeline"
    };

    let candidate_id = list_tasks(Some(kind_str.to_string()), Some(project_name.to_string()))
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

#[allow(clippy::too_many_arguments)]
fn spawn_pipeline_stream(
    task_id: String,
    mut stages: Signal<Vec<StageData>>,
    mut pipeline_status: Signal<PipelineStatus>,
    auto_scroll: Signal<bool>,
    mut active_task_id: Signal<String>,
    mut expanded_stage: Signal<Option<u32>>,
    mut pipeline_progress_ctx: Signal<Option<f32>>,
    mut error_msg: Signal<String>,
    is_dry_run: bool,
    mut tasks_ctx: TasksCtx,
) {
    spawn(async move {
        let mut on_event_cursor = 0usize;

        'reconnect: loop {
            let mut stream_len = 0usize;

            // On reconnect, clear stale log lines so replayed events
            // repopulate them cleanly without duplicates.
            if on_event_cursor > 0 {
                let mut s = stages.write();
                for stage in s.iter_mut() {
                    stage.lines.clear();
                }
            }

            match subscribe_task_events(task_id.clone()).await {
                Ok(mut stream) => {
                    'stream: loop {
                        match stream.recv().await {
                            Some(Ok(event)) => {
                                stream_len += 1;
                                let is_new = stream_len > on_event_cursor;
                                let is_terminal =
                                    matches!(event, TaskEvent::Completed | TaskEvent::Failed(_));

                                // Update global TasksCtx.
                                {
                                    let mut state = tasks_ctx.write();
                                    if let Some(entry) =
                                        state.tasks.iter_mut().find(|t| t.id == task_id)
                                    {
                                        apply_event_to_entry(entry, &event, stream_len);
                                    }
                                }

                                // Skip keep-alive heartbeats.
                                let is_heartbeat = matches!(
                                    &event,
                                    TaskEvent::Log(msg) if msg.contains("Keep-alive")
                                );

                                if is_new && !is_heartbeat {
                                    on_event_cursor = stream_len;
                                    match event {
                                        TaskEvent::PipelineRemainingGroups(names) => {
                                            let total = names.len() as u32;
                                            let mut s = stages.write();
                                            if s.is_empty() {
                                                for (i, name) in names.into_iter().enumerate() {
                                                    s.push(StageData {
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
                                            }
                                        }
                                        TaskEvent::PipelineStageStarted {
                                            stage_index,
                                            stage_name,
                                            total_stages,
                                            stage_status,
                                            pipeline_stage_num,
                                        } => {
                                            let mut s = stages.write();
                                            while s.len() <= stage_index as usize {
                                                let idx = s.len() as u32;
                                                s.push(StageData::new(
                                                    idx,
                                                    String::new(),
                                                    total_stages,
                                                ));
                                            }
                                            let cached =
                                                matches!(stage_status, PipelineStageStatus::Cached);
                                            let skipped = matches!(
                                                stage_status,
                                                PipelineStageStatus::Skipped
                                            );
                                            let actually_running =
                                                !cached && !skipped && !is_dry_run;
                                            let stage = &mut s[stage_index as usize];
                                            stage.name = stage_name.clone();
                                            if total_stages > 0 {
                                                stage.total_stages = total_stages;
                                            }
                                            stage.cached = cached;
                                            stage.skipped = skipped;
                                            stage.completed = cached || skipped;
                                            stage.is_running = actually_running;
                                            stage.pipeline_stage_num = pipeline_stage_num;
                                            if actually_running && pipeline_stage_num.is_some() {
                                                expanded_stage.set(Some(stage_index));
                                            }
                                            if cached || skipped {
                                                let total_p = s
                                                    .iter()
                                                    .filter(|x| x.pipeline_stage_num.is_some())
                                                    .count()
                                                    as f32;
                                                let done_p = s
                                                    .iter()
                                                    .filter(|x| {
                                                        x.pipeline_stage_num.is_some()
                                                            && x.completed
                                                    })
                                                    .count()
                                                    as f32;
                                                if total_p > 0.0 {
                                                    pipeline_progress_ctx.set(Some(
                                                        (done_p / total_p).clamp(0.0, 1.0),
                                                    ));
                                                }
                                            }
                                        }
                                        TaskEvent::PipelineLog {
                                            stage_index,
                                            stage_name,
                                            line,
                                        } => {
                                            let mut s = stages.write();
                                            while s.len() <= stage_index as usize {
                                                let idx = s.len() as u32;
                                                let total =
                                                    s.last().map(|x| x.total_stages).unwrap_or(1);
                                                s.push(StageData::new(
                                                    idx,
                                                    stage_name.clone(),
                                                    total,
                                                ));
                                            }
                                            s[stage_index as usize].add_log_line(line);
                                            drop(s);
                                            if auto_scroll() {
                                                let _ = eval(
                                                    "var el = document.getElementById('logs-output'); \
                                                     if (el) el.scrollTop = el.scrollHeight;",
                                                );
                                            }
                                        }
                                        TaskEvent::PipelineStageProgress {
                                            stage_index,
                                            progress,
                                        } => {
                                            let mut s = stages.write();
                                            if let Some(stage) = s.get_mut(stage_index as usize) {
                                                if stage.pipeline_stage_num.is_some() {
                                                    stage.progress = Some(progress);
                                                }
                                            }
                                            let total_p = s
                                                .iter()
                                                .filter(|x| x.pipeline_stage_num.is_some())
                                                .count()
                                                as f32;
                                            if total_p > 0.0 {
                                                let done_p = s
                                                    .iter()
                                                    .filter(|x| {
                                                        x.pipeline_stage_num.is_some()
                                                            && x.completed
                                                    })
                                                    .count()
                                                    as f32;
                                                let sub = progress / total_p;
                                                pipeline_progress_ctx.set(Some(
                                                    (done_p / total_p + sub).clamp(0.0, 1.0),
                                                ));
                                            }
                                        }
                                        TaskEvent::PipelineStageCompleted {
                                            stage_index, ..
                                        } => {
                                            let mut s = stages.write();
                                            if let Some(stage) = s.get_mut(stage_index as usize) {
                                                if !is_dry_run || stage.cached || stage.skipped {
                                                    stage.completed = true;
                                                    if stage.pipeline_stage_num.is_some() {
                                                        stage.progress = Some(1.0);
                                                    }
                                                }
                                                stage.is_running = false;
                                            }
                                            let total_p = s
                                                .iter()
                                                .filter(|x| x.pipeline_stage_num.is_some())
                                                .count()
                                                as f32;
                                            if total_p > 0.0 {
                                                let done_p = s
                                                    .iter()
                                                    .filter(|x| {
                                                        x.pipeline_stage_num.is_some()
                                                            && x.completed
                                                    })
                                                    .count()
                                                    as f32;
                                                pipeline_progress_ctx
                                                    .set(Some((done_p / total_p).clamp(0.0, 1.0)));
                                            }
                                        }
                                        TaskEvent::Completed => {
                                            if is_dry_run {
                                                pipeline_status.set(PipelineStatus::Idle);
                                            } else {
                                                pipeline_status.set(PipelineStatus::Completed);
                                                pipeline_progress_ctx.set(Some(1.0));
                                            }
                                            active_task_id.set(String::new());
                                            break 'reconnect;
                                        }
                                        TaskEvent::Failed(msg) => {
                                            if is_dry_run {
                                                pipeline_status.set(PipelineStatus::Idle);
                                            } else {
                                                pipeline_status.set(PipelineStatus::Failed(msg));
                                            }
                                            active_task_id.set(String::new());
                                            pipeline_progress_ctx.set(None);
                                            break 'reconnect;
                                        }
                                        _ => {}
                                    } // end match event
                                } // end if is_new && !is_heartbeat

                                if is_terminal {
                                    break 'reconnect;
                                }
                            }
                            Some(Err(_)) | None => break 'stream,
                        }
                    } // end 'stream loop
                }
                Err(e) => {
                    error_msg.set(format!("Failed to subscribe to pipeline events: {e}"));
                }
            }

            // Stream ended without a terminal event – check actual state.
            match get_task_info(task_id.clone()).await {
                Ok(Some(info)) if info.state == TaskState::Running => {
                    tracing::debug!(
                        task_id = %task_id,
                        "pipeline still running after stream drop; reconnecting"
                    );
                    continue 'reconnect;
                }
                Ok(Some(info)) => {
                    match info.state {
                        TaskState::Completed => {
                            if is_dry_run {
                                pipeline_status.set(PipelineStatus::Idle);
                            } else {
                                pipeline_status.set(PipelineStatus::Completed);
                                pipeline_progress_ctx.set(Some(1.0));
                            }
                        }
                        TaskState::Failed(msg) => {
                            if is_dry_run {
                                pipeline_status.set(PipelineStatus::Idle);
                            } else {
                                pipeline_status.set(PipelineStatus::Failed(msg));
                            }
                            pipeline_progress_ctx.set(None);
                        }
                        TaskState::Running => continue 'reconnect,
                    }
                    active_task_id.set(String::new());
                    break 'reconnect;
                }
                _ => {
                    if !is_dry_run {
                        error_msg.set("Pipeline disconnected; state unknown.".to_string());
                    }
                    pipeline_status.set(PipelineStatus::Idle);
                    active_task_id.set(String::new());
                    break 'reconnect;
                }
            }
        } // end 'reconnect loop
    }); // end spawn
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
    let mut is_current_dry_run = use_signal(|| false);

    let mut pipeline_progress_ctx = use_context::<PipelineProgressCtx>();
    let mut pipeline_is_running = use_context::<PipelineIsRunningCtx>();
    let mut pipeline_command = use_context::<PipelineCommandCtx>();
    let mut tasks_ctx = use_context::<TasksCtx>();

    // ── Keep PipelineIsRunningCtx in sync with local status ──────────────
    use_effect(move || {
        let running = matches!(pipeline_status(), PipelineStatus::Running);
        pipeline_is_running.set(running);
    });

    // ── On mount: reconnect or auto-start dry-run ─────────────────────────
    //
    // We intentionally read NO signals in the synchronous closure body so
    // this effect runs exactly once (on mount).  All signal reads happen
    // inside the spawned async task where they do not create subscriptions.
    let project_name_mount = project_name.clone();
    use_effect(move || {
        let project_name = project_name_mount.clone();
        spawn(async move {
            // If the command watcher already set Running (e.g. user clicked
            // Run before this tab was mounted), skip the auto dry-run so we
            // don't race with the full run that the command watcher starts.
            if matches!(pipeline_status.peek().clone(), PipelineStatus::Running) {
                return;
            }

            // 1. Reconnect to a running full pipeline.
            if let Some(task_id) = find_running_task(&project_name, false).await {
                active_task_id.set(task_id.clone());
                pipeline_status.set(PipelineStatus::Running);
                is_current_dry_run.set(false);
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
                );
                return;
            }

            // 2. Reconnect to a running dry-run.
            if let Some(task_id) = find_running_task(&project_name, true).await {
                active_task_id.set(task_id.clone());
                pipeline_status.set(PipelineStatus::Running);
                is_current_dry_run.set(true);
                tasks_ctx.write().register(
                    task_id.clone(),
                    format!("Dry-run: {}", project_name),
                    TaskKind::DryRunPipeline,
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
                    true,
                    tasks_ctx,
                );
                return;
            }

            // 3. Auto-start a dry-run to show cached/pending stage status.
            match run_pipeline(project_name.clone(), true).await {
                Ok(task_id) => {
                    active_task_id.set(task_id.clone());
                    pipeline_status.set(PipelineStatus::Running);
                    is_current_dry_run.set(true);
                    tasks_ctx.write().register(
                        task_id.clone(),
                        format!("Dry-run: {}", project_name),
                        TaskKind::DryRunPipeline,
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
                        true,
                        tasks_ctx,
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
            // race and also start a dry-run.
            pipeline_status.set(PipelineStatus::Running);
            // Reset runtime state but preserve cached/skipped flags from dry-run.
            // This allows us to recover from a dry-run and start the real pipeline
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
            is_current_dry_run.set(false);

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
    let is_running = matches!(pipeline_status(), PipelineStatus::Running);

    rsx! {
        document::Link { rel: "stylesheet", href: asset!("/assets/views/project/logs.css") }

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
                            if is_current_dry_run() { "● Checking…" } else { "● Running" }
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

                // Auto-scroll toggle
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

            // ── Stage groups or placeholder ───────────────────────────────
            if stages_snapshot.is_empty() {
                div {
                    class: "logs-placeholder",
                    if is_running {
                        "Checking pipeline state…"
                    } else {
                        "Press ▶️ Run in the header to start the reconstruction."
                    }
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
        }
    }
}
