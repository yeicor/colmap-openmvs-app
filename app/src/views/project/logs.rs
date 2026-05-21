use crate::server::{cancel_task, get_task_info, list_tasks, run_pipeline, subscribe_task_events};
use crate::views::project::{PipelineCommandCtx, PipelineIsRunningCtx, PipelineProgressCtx};
use colmap_openmvs_api::{PipelineStageStatus, TaskEvent, TaskState};
use dioxus::document::eval;
use dioxus::prelude::*;

// ---------------------------------------------------------------------------
// localStorage helpers
// ---------------------------------------------------------------------------

fn ls_set_pipeline_task(project_name: &str, task_id: &str) {
    let safe_key = project_name.replace(['\'', '\\'], "_");
    let safe_val = task_id.replace(['\'', '\\'], "_");
    let _ = eval(&format!(
        "try {{ localStorage.setItem('colmap_task_pipeline_{safe_key}', '{safe_val}'); }} catch(e) {{}}"
    ));
}

async fn ls_get_pipeline_task(project_name: &str) -> Option<String> {
    let safe_key = project_name.replace(['\'', '\\'], "_");
    let js = eval(&format!(
        "return (localStorage.getItem('colmap_task_pipeline_{safe_key}') || '');"
    ));
    js.await
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .filter(|s| !s.is_empty())
}

fn ls_clear_pipeline_task(project_name: &str) {
    let safe_key = project_name.replace(['\'', '\\'], "_");
    let _ = eval(&format!(
        "try {{ localStorage.removeItem('colmap_task_pipeline_{safe_key}'); }} catch(e) {{}}"
    ));
}

fn ls_set_dryrun_task(project_name: &str, task_id: &str) {
    let safe_key = project_name.replace(['\'', '\\'], "_");
    let safe_val = task_id.replace(['\'', '\\'], "_");
    let _ = eval(&format!(
        "try {{ localStorage.setItem('colmap_task_dryrun_{safe_key}', '{safe_val}'); }} catch(e) {{}}"
    ));
}

async fn ls_get_dryrun_task(project_name: &str) -> Option<String> {
    let safe_key = project_name.replace(['\'', '\\'], "_");
    let js = eval(&format!(
        "return (localStorage.getItem('colmap_task_dryrun_{safe_key}') || '');"
    ));
    js.await
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .filter(|s| !s.is_empty())
}

fn ls_clear_dryrun_task(project_name: &str) {
    let safe_key = project_name.replace(['\'', '\\'], "_");
    let _ = eval(&format!(
        "try {{ localStorage.removeItem('colmap_task_dryrun_{safe_key}'); }} catch(e) {{}}"
    ));
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

    // 1. Check localStorage
    let stored_id = if dry_run {
        ls_get_dryrun_task(project_name).await
    } else {
        ls_get_pipeline_task(project_name).await
    };

    let candidate_id = if let Some(id) = stored_id {
        Some(id)
    } else {
        // 2. Fall back to server-side task list
        list_tasks(Some(kind_str.to_string()), Some(project_name.to_string()))
            .await
            .ok()
            .and_then(|tasks| {
                tasks
                    .into_iter()
                    .find(|t| t.state == TaskState::Running)
                    .map(|t| t.id)
            })
    };

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
    project_name: String,
    mut stages: Signal<Vec<StageData>>,
    mut pipeline_status: Signal<PipelineStatus>,
    auto_scroll: Signal<bool>,
    mut active_task_id: Signal<String>,
    mut expanded_stage: Signal<Option<u32>>,
    mut pipeline_progress_ctx: Signal<Option<f32>>,
    mut error_msg: Signal<String>,
    is_dry_run: bool,
) {
    spawn(async move {
        match subscribe_task_events(task_id.clone()).await {
            Ok(mut stream) => {
                while let Some(Ok(event)) = stream.recv().await {
                    match event {
                        // --------------------------------------------------
                        // Pre-populate stage list from the leading groups list
                        // --------------------------------------------------
                        // NOTE: In recovery from a dry-run (full pipeline after dry-run),
                        // stages may already be populated with cached/skipped flags.
                        // We skip re-population to preserve that cached state information.
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

                        // --------------------------------------------------
                        // Stage started
                        // --------------------------------------------------
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
                                s.push(StageData::new(idx, String::new(), total_stages));
                            }
                            let cached = matches!(stage_status, PipelineStageStatus::Cached);
                            let skipped = matches!(stage_status, PipelineStageStatus::Skipped);
                            // In a dry-run, a "Run" status means "needs to execute" (pending),
                            // not that it is actually executing right now.
                            let actually_running = !cached && !skipped && !is_dry_run;

                            let stage = &mut s[stage_index as usize];
                            stage.name = stage_name.clone();
                            // Only update total_stages for pipeline stages (those with a count)
                            if total_stages > 0 {
                                stage.total_stages = total_stages;
                            }
                            stage.cached = cached;
                            stage.skipped = skipped;
                            stage.completed = cached || skipped;
                            stage.is_running = actually_running;
                            stage.pipeline_stage_num = pipeline_stage_num;

                            // Auto-expand only actual running pipeline stages
                            if actually_running && pipeline_stage_num.is_some() {
                                expanded_stage.set(Some(stage_index));
                            }

                            // Update global progress counter for cached/skipped stages
                            if cached || skipped {
                                let total_pipeline =
                                    s.iter().filter(|x| x.pipeline_stage_num.is_some()).count()
                                        as f32;
                                let completed_pipeline =
                                    s.iter()
                                        .filter(|x| x.pipeline_stage_num.is_some() && x.completed)
                                        .count() as f32;
                                if total_pipeline > 0.0 {
                                    pipeline_progress_ctx.set(Some(
                                        (completed_pipeline / total_pipeline).clamp(0.0, 1.0),
                                    ));
                                }
                            }
                        }

                        // --------------------------------------------------
                        // Log line
                        // --------------------------------------------------
                        TaskEvent::PipelineLog {
                            stage_index,
                            stage_name,
                            line,
                        } => {
                            let mut s = stages.write();
                            while s.len() <= stage_index as usize {
                                let idx = s.len() as u32;
                                let total = s.last().map(|x| x.total_stages).unwrap_or(1);
                                s.push(StageData::new(idx, stage_name.clone(), total));
                            }
                            s[stage_index as usize].lines.push(line);
                            drop(s);
                            if auto_scroll() {
                                let _ = eval(
                                    "var el = document.getElementById('logs-output'); \
                                     if (el) el.scrollTop = el.scrollHeight;",
                                );
                            }
                        }

                        // --------------------------------------------------
                        // Sub-progress within a stage
                        // --------------------------------------------------
                        TaskEvent::PipelineStageProgress {
                            stage_index,
                            progress,
                        } => {
                            let mut s = stages.write();
                            if let Some(stage) = s.get_mut(stage_index as usize) {
                                // Only update progress for pipeline stages (not Config/Tool Discovery)
                                if stage.pipeline_stage_num.is_some() {
                                    stage.progress = Some(progress);
                                }
                            }
                            let total_pipeline =
                                s.iter().filter(|x| x.pipeline_stage_num.is_some()).count() as f32;
                            if total_pipeline > 0.0 {
                                let completed_pipeline =
                                    s.iter()
                                        .filter(|x| x.pipeline_stage_num.is_some() && x.completed)
                                        .count() as f32;
                                let sub = progress / total_pipeline;
                                pipeline_progress_ctx.set(Some(
                                    (completed_pipeline / total_pipeline + sub).clamp(0.0, 1.0),
                                ));
                            }
                        }

                        // --------------------------------------------------
                        // Stage completed
                        // --------------------------------------------------
                        TaskEvent::PipelineStageCompleted { stage_index, .. } => {
                            let mut s = stages.write();
                            if let Some(stage) = s.get_mut(stage_index as usize) {
                                // During dry-run: only mark cached/skipped as completed
                                // During full run: mark any completed stage as completed
                                if !is_dry_run || stage.cached || stage.skipped {
                                    stage.completed = true;
                                    // Only set progress to 1.0 for stages that are actually completed
                                    if stage.pipeline_stage_num.is_some() {
                                        stage.progress = Some(1.0);
                                    }
                                }
                                stage.is_running = false;
                            }
                            let total_pipeline =
                                s.iter().filter(|x| x.pipeline_stage_num.is_some()).count() as f32;
                            if total_pipeline > 0.0 {
                                let completed_pipeline =
                                    s.iter()
                                        .filter(|x| x.pipeline_stage_num.is_some() && x.completed)
                                        .count() as f32;
                                pipeline_progress_ctx.set(Some(
                                    (completed_pipeline / total_pipeline).clamp(0.0, 1.0),
                                ));
                            }
                        }

                        // --------------------------------------------------
                        // Terminal events
                        // --------------------------------------------------
                        TaskEvent::Completed => {
                            if is_dry_run {
                                // Dry-run done: go back to Idle so the header
                                // shows "Run" (not "Completed").
                                pipeline_status.set(PipelineStatus::Idle);
                                ls_clear_dryrun_task(&project_name);
                                // Leave pipeline_progress_ctx at current value
                                // so the header bar shows the cached-stage fraction.
                            } else {
                                pipeline_status.set(PipelineStatus::Completed);
                                pipeline_progress_ctx.set(Some(1.0));
                                ls_clear_pipeline_task(&project_name);
                            }
                            active_task_id.set(String::new());
                            return;
                        }

                        TaskEvent::Failed(msg) => {
                            if is_dry_run {
                                // Silently absorb dry-run failures (e.g. no images yet).
                                pipeline_status.set(PipelineStatus::Idle);
                                ls_clear_dryrun_task(&project_name);
                            } else {
                                pipeline_status.set(PipelineStatus::Failed(msg));
                                ls_clear_pipeline_task(&project_name);
                            }
                            active_task_id.set(String::new());
                            pipeline_progress_ctx.set(None);
                            return;
                        }

                        _ => {}
                    }
                }

                // Stream ended without an explicit terminal event
                if is_dry_run {
                    pipeline_status.set(PipelineStatus::Idle);
                    ls_clear_dryrun_task(&project_name);
                } else {
                    pipeline_status.set(PipelineStatus::Completed);
                    pipeline_progress_ctx.set(Some(1.0));
                    ls_clear_pipeline_task(&project_name);
                }
                active_task_id.set(String::new());
            }

            Err(e) => {
                error_msg.set(format!("Failed to subscribe to pipeline events: {e}"));
                pipeline_status.set(PipelineStatus::Idle);
                active_task_id.set(String::new());
                pipeline_progress_ctx.set(None);
            }
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
    // Track whether the currently active task is a dry-run.
    let mut is_current_dry_run = use_signal(|| false);

    // Shared contexts provided by the parent Project component.
    let mut pipeline_progress_ctx = use_context::<PipelineProgressCtx>();
    let mut pipeline_is_running = use_context::<PipelineIsRunningCtx>();
    let mut pipeline_command = use_context::<PipelineCommandCtx>();

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
                spawn_pipeline_stream(
                    task_id,
                    project_name.clone(),
                    stages,
                    pipeline_status,
                    auto_scroll,
                    active_task_id,
                    expanded_stage,
                    pipeline_progress_ctx,
                    error_msg,
                    false,
                );
                return;
            }

            // 2. Reconnect to a running dry-run.
            if let Some(task_id) = find_running_task(&project_name, true).await {
                active_task_id.set(task_id.clone());
                pipeline_status.set(PipelineStatus::Running);
                is_current_dry_run.set(true);
                spawn_pipeline_stream(
                    task_id,
                    project_name.clone(),
                    stages,
                    pipeline_status,
                    auto_scroll,
                    active_task_id,
                    expanded_stage,
                    pipeline_progress_ctx,
                    error_msg,
                    true,
                );
                return;
            }

            // 3. Auto-start a dry-run to show cached/pending stage status.
            match run_pipeline(project_name.clone(), true).await {
                Ok(task_id) => {
                    ls_set_dryrun_task(&project_name, &task_id);
                    active_task_id.set(task_id.clone());
                    pipeline_status.set(PipelineStatus::Running);
                    is_current_dry_run.set(true);
                    spawn_pipeline_stream(
                        task_id,
                        project_name.clone(),
                        stages,
                        pipeline_status,
                        auto_scroll,
                        active_task_id,
                        expanded_stage,
                        pipeline_progress_ctx,
                        error_msg,
                        true,
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
                        ls_set_pipeline_task(&project_name, &task_id);
                        active_task_id.set(task_id.clone());
                        spawn_pipeline_stream(
                            task_id,
                            project_name.clone(),
                            stages,
                            pipeline_status,
                            auto_scroll,
                            active_task_id,
                            expanded_stage,
                            pipeline_progress_ctx,
                            error_msg,
                            false,
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
