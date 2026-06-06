//! Global task management.
//!
//! All long-running background tasks (image preparation, demo download, batch
//! resize, pipeline runs) register themselves here.  The [`TasksCtx`] signal
//! lives at the `App` level so it outlives any individual component.
//!
//! # Why this exists
//! Dioxus `spawn` futures are scoped to the component that calls them.  When a
//! user navigates away, the component unmounts and the subscription is dropped –
//! even though the server task is still running.  This module solves that by:
//!
//! 1. Keeping a persistent task list in [`TasksCtx`] (provided at `App` level).
//! 2. [`drive_task`] auto-reconnects when the SSE stream closes unexpectedly
//!    instead of treating stream-end as success.
//! 3. [`TasksPanel`](crate::mycomponents::tasks_panel) in `ProjectsSidebar`
//!    (always mounted) re-subscribes to any task that lost its component
//!    subscription, so the panel stays live.

use crate::server::poll_task_events;
use colmap_openmvs_api::{
    DemoProgressEvent, PrepareProgress, ResizeProgressEvent, TaskEvent, TaskKind, TaskState,
};
use dioxus::prelude::*;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// How often the client polls the server for new task events.
const POLL_INTERVAL_MS: u64 = 300;

/// Cross-platform async sleep.
pub async fn sleep_poll() {
    #[cfg(not(target_arch = "wasm32"))]
    tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
}

pub const MAX_LOG_LINES: usize = 500;

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct TaskEntry {
    pub id: String,
    pub label: String,
    pub kind: TaskKind,
    pub state: TaskState,
    /// Recent log lines (capped at [`MAX_LOG_LINES`]).
    pub logs: Vec<String>,
    /// Overall 0.0..=1.0 progress; `None` if unknown.
    pub progress: Option<f32>,
    /// How many stream events have been written into `logs` so far.
    /// Used by [`drive_task`] to deduplicate log lines when multiple
    /// subscribers replay the same event history concurrently.
    pub logs_cursor: usize,
    /// For pipeline tasks: number of actual pipeline stages (excludes Config/Tool Discovery).
    /// Used to calculate global progress correctly.
    pub total_pipeline_stages: Option<u32>,
    /// For pipeline tasks: which stage is currently running (1-based, or 0 for Config/Tool Discovery).
    pub current_stage_num: Option<u32>,
    /// For pipeline tasks: progress within the current stage (0.0..=1.0).
    pub stage_progress: Option<f32>,
}

impl TaskEntry {
    pub fn new(id: String, label: String, kind: TaskKind) -> Self {
        Self {
            id,
            label,
            kind,
            state: TaskState::Running,
            logs: Vec::new(),
            progress: None,
            logs_cursor: 0,
            total_pipeline_stages: None,
            current_stage_num: None,
            stage_progress: None,
        }
    }

    /// Calculate global pipeline progress (0.0..=1.0) based on current stage and stage progress.
    /// Config and Tool Discovery stages (stage_num=0) are not counted toward progress.
    pub fn compute_pipeline_progress(&self) -> Option<f32> {
        if let (Some(total), Some(current), Some(stage_prog)) = (
            self.total_pipeline_stages,
            self.current_stage_num,
            self.stage_progress,
        ) {
            if total == 0 {
                return None;
            }
            // Config/Tool Discovery is stage_num=0, doesn't count
            if current == 0 {
                return Some(0.0);
            }
            // Calculate: previous stages are complete (100%), current stage is at stage_prog%
            let previous_stages = (current - 1) as f32; // Stages before current (1-indexed)
            let current_stage_contribution = stage_prog;
            let global_progress = (previous_stages + current_stage_contribution) / total as f32;
            Some(global_progress.clamp(0.0, 1.0))
        } else {
            None
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, TaskState::Running)
    }

    pub fn is_terminal(&self) -> bool {
        !self.is_running()
    }

    /// Get the progress to display. For pipeline tasks, use computed progress.
    /// For other tasks, use the stored progress.
    pub fn display_progress(&self) -> Option<f32> {
        self.compute_pipeline_progress().or(self.progress)
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct TasksState {
    pub tasks: Vec<TaskEntry>,
}

impl TasksState {
    pub fn running_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.is_running()).count()
    }

    /// Register a new Running task.  No-op if the `id` is already present.
    pub fn register(&mut self, id: String, label: String, kind: TaskKind) {
        if self.tasks.iter().any(|t| t.id == id) {
            return;
        }
        self.tasks.insert(0, TaskEntry::new(id, label, kind));
        self.enforce_max_tasks(20);
    }

    /// Remove old terminal tasks, keeping at most `MAX_TERMINAL` recent ones.
    pub fn gc(&mut self) {
        const MAX_TERMINAL: usize = 10;
        let terminal_count = self.tasks.iter().filter(|t| t.is_terminal()).count();
        if terminal_count > MAX_TERMINAL {
            let to_drop = terminal_count - MAX_TERMINAL;
            let mut dropped = 0usize;
            self.tasks.retain(|t| {
                if t.is_terminal() && dropped < to_drop {
                    dropped += 1;
                    false
                } else {
                    true
                }
            });
        }
    }

    /// Forget a single task by ID.
    pub fn forget_task(&mut self, id: &str) {
        self.tasks.retain(|t| t.id != id);
    }

    /// Forget all completed and failed tasks.
    pub fn forget_completed(&mut self) {
        self.tasks.retain(|t| t.is_running());
    }

    /// Enforce maximum task limit (keep newest tasks).
    /// Tasks are sorted with newest at the beginning.
    pub fn enforce_max_tasks(&mut self, max_tasks: usize) {
        if self.tasks.len() > max_tasks {
            self.tasks.truncate(max_tasks);
        }
    }
}

/// Global context type.  Provide this once at `App` level with
/// `use_context_provider(|| Signal::new(TasksState::default()))`.
pub type TasksCtx = Signal<TasksState>;

// ---------------------------------------------------------------------------
// Event → log line
// ---------------------------------------------------------------------------

/// Convert a [`TaskEvent`] to a human-readable log line, or `None` for events
/// that don't produce visible output (keep-alives, progress-only events, etc.).
pub fn event_to_log_line(event: &TaskEvent) -> Option<String> {
    match event {
        TaskEvent::Log(msg) if !msg.contains("Keep-alive") => Some(msg.clone()),

        TaskEvent::PrepareProgress(PrepareProgress::Downloading {
            downloaded_bytes,
            total_bytes,
        }) => {
            let mb = *downloaded_bytes as f64 / 1_048_576.0;
            Some(if let Some(total) = total_bytes {
                format!("↓ {:.1}/{:.1} MB", mb, *total as f64 / 1_048_576.0)
            } else {
                format!("↓ {:.1} MB", mb)
            })
        }
        TaskEvent::PrepareProgress(PrepareProgress::ExtractingLayer { layer, progress }) => {
            Some(format!("⚙ {} {:.0}%", layer, progress * 100.0))
        }
        TaskEvent::PrepareProgress(PrepareProgress::Error { message }) => {
            Some(format!("✗ {}", message))
        }

        TaskEvent::DemoProgress(DemoProgressEvent::FetchingFileList) => {
            Some("↓ fetching file list…".to_string())
        }
        TaskEvent::DemoProgress(DemoProgressEvent::DownloadProgress {
            filename,
            downloaded,
            total,
        }) => {
            if filename.is_empty() {
                Some(format!("↓ demo {}/{} files", downloaded, total))
            } else {
                Some(format!("↓ demo {}/{}: {}", downloaded + 1, total, filename))
            }
        }
        TaskEvent::DemoProgress(DemoProgressEvent::Error { message }) => {
            Some(format!("✗ {}", message))
        }

        TaskEvent::ResizeProgress(ResizeProgressEvent::ResizeProgress {
            name,
            completed,
            total_files,
        }) => Some(format!("⚙ {}/{} {}", completed, total_files, name)),
        TaskEvent::ResizeProgress(ResizeProgressEvent::Error { message }) => {
            Some(format!("✗ {}", message))
        }

        TaskEvent::PipelineLog {
            stage_name, line, ..
        } => Some(format!("[{}] {}", stage_name, line)),
        TaskEvent::PipelineStageStarted {
            stage_name,
            stage_status,
            ..
        } => {
            use colmap_openmvs_api::PipelineStageStatus;
            let icon = match stage_status {
                PipelineStageStatus::Cached => "↩",
                PipelineStageStatus::Skipped => "⊘",
                PipelineStageStatus::Run => "▶",
            };
            Some(format!("{} {}", icon, stage_name))
        }
        TaskEvent::PipelineStageCompleted { stage_name, .. } => Some(format!("✓ {}", stage_name)),

        TaskEvent::Completed => Some("✓ Done".to_string()),
        TaskEvent::Failed(msg) => Some(format!("✗ {}", msg)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Internal: apply an event to a TaskEntry
// ---------------------------------------------------------------------------

/// Apply a single event to a [`TaskEntry`].
///
/// `stream_pos` is the 1-based index of this event in the full replayed stream.
/// Log lines are only appended when `stream_pos > entry.logs_cursor` so that
/// concurrent subscribers (component + sidebar background) never produce
/// duplicate entries.
pub fn apply_event_to_entry(entry: &mut TaskEntry, event: &TaskEvent, stream_pos: usize) {
    // State / progress updates are always idempotent.
    match event {
        TaskEvent::Completed => {
            entry.state = TaskState::Completed;
            entry.progress = Some(1.0);
        }
        TaskEvent::Failed(msg) => {
            entry.state = TaskState::Failed(msg.clone());
        }
        TaskEvent::PrepareProgress(PrepareProgress::Downloading {
            downloaded_bytes,
            total_bytes: Some(total),
        }) if *total > 0 => {
            entry.progress = Some(*downloaded_bytes as f32 / *total as f32);
        }
        TaskEvent::PrepareProgress(PrepareProgress::ExtractingLayer { progress, .. }) => {
            entry.progress = Some(*progress);
        }
        TaskEvent::DemoProgress(DemoProgressEvent::DownloadProgress {
            downloaded, total, ..
        }) if *total > 0 => {
            entry.progress = Some(*downloaded as f32 / *total as f32);
        }
        TaskEvent::ResizeProgress(ResizeProgressEvent::ResizeProgress {
            completed,
            total_files,
            ..
        }) if *total_files > 0 => {
            entry.progress = Some(*completed as f32 / *total_files as f32);
        }
        TaskEvent::PipelineRemainingGroups(groups) => {
            // Count non-Config/Tool-Discovery stages (those with pipeline_stage_num > 0)
            // This will be refined when we see actual stage starts.
            entry.total_pipeline_stages = Some(groups.len() as u32);
        }
        TaskEvent::PipelineStageStarted {
            stage_index: _,
            pipeline_stage_num,
            total_stages,
            stage_status,
            ..
        } => {
            // Store total stages count
            if *total_stages > 0 {
                entry.total_pipeline_stages = Some(*total_stages);
            }

            // Update current stage and initialize stage progress
            if let Some(stage_num) = pipeline_stage_num {
                entry.current_stage_num = Some(*stage_num);
                // For cached/skipped stages, progress is 1.0. For running stages, start at 0.0
                entry.stage_progress = Some(match stage_status {
                    colmap_openmvs_api::PipelineStageStatus::Cached
                    | colmap_openmvs_api::PipelineStageStatus::Skipped => 1.0,
                    colmap_openmvs_api::PipelineStageStatus::Run => 0.0,
                });
            } else {
                // Config/Tool Discovery
                entry.current_stage_num = Some(0);
                entry.stage_progress = Some(1.0);
            }
        }
        TaskEvent::PipelineStageProgress { progress, .. } => {
            // Update progress within current stage
            entry.stage_progress = Some(*progress);
        }
        _ => {}
    }

    // Log update – deduplicated.
    if stream_pos > entry.logs_cursor {
        entry.logs_cursor = stream_pos;
        if let Some(line) = event_to_log_line(event) {
            entry.logs.push(line);
            if entry.logs.len() > MAX_LOG_LINES {
                // Drop oldest quarter to amortise the cost.
                entry.logs.drain(0..MAX_LOG_LINES / 4);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new task in the global context and start streaming its events.
///
/// * Updates [`TasksCtx`] with progress, state, and log lines.
/// * Calls `on_event` for each *new* event so the calling component can update
///   its local UI state (banners, progress text, etc.).
///
/// The subscription is scoped to the spawning component.  [`TasksPanel`] in
/// `ProjectsSidebar` provides a persistent background subscription that kicks
/// in when the component unmounts.
///
/// [`TasksPanel`]: crate::mycomponents::TasksPanel
pub fn start_task<F: FnMut(TaskEvent) + 'static>(
    task_id: String,
    label: String,
    kind: TaskKind,
    mut tasks: TasksCtx,
    on_event: F,
) {
    tasks.write().register(task_id.clone(), label, kind);
    drive_task(task_id, tasks, on_event);
}

/// Subscribe to an already-registered task and drive its event stream via polling.
///
/// Polls the server every [`POLL_INTERVAL_MS`] ms for new events.
/// Updates [`TasksCtx`] and calls `on_event` for each new event.
/// Stops automatically when a terminal event (Completed/Failed) is received
/// or when the task is no longer found in the registry.
pub fn drive_task<F: FnMut(TaskEvent) + 'static>(
    task_id: String,
    mut tasks: TasksCtx,
    mut on_event: F,
) {
    spawn(async move {
        let mut cursor = 0usize;

        loop {
            match poll_task_events(task_id.clone(), cursor).await {
                Ok(batch) => {
                    if !batch.task_found {
                        // Task evicted or never existed — stop silently.
                        break;
                    }

                    let new_cursor = batch.cursor;
                    let is_terminal = batch.is_terminal;
                    let mut local_pos = cursor;

                    for event in batch.events {
                        local_pos += 1;

                        // Update global context (log dedup via entry.logs_cursor).
                        {
                            let mut state = tasks.write();
                            if let Some(entry) = state.tasks.iter_mut().find(|t| t.id == task_id) {
                                apply_event_to_entry(entry, &event, local_pos);
                            }
                        }

                        on_event(event);
                    }

                    cursor = new_cursor;

                    if is_terminal {
                        tasks.write().gc();
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        task_id = %task_id,
                        error = %e,
                        "poll_task_events failed; will retry"
                    );
                }
            }

            sleep_poll().await;
        }
    });
}

// ---------------------------------------------------------------------------
// Startup task context (shared across components during boot)
// ---------------------------------------------------------------------------

/// Shared state for the one-time server startup task.
///
/// Provided at `App` level so the task can be kicked off immediately without
/// waiting for `StartupTasks` to mount.  `StartupTasks` picks up the running
/// task from here when it renders.
#[derive(Clone, Copy, Default)]
pub struct StartupCtx {
    /// The URL the user originally navigated to (stored before redirect).
    pub origin: Signal<String>,
    /// The server task ID, once started.
    pub task_id: Signal<Option<String>>,
    /// Whether the startup cycle has reached a terminal state.
    pub is_completed: Signal<bool>,
    /// Terminal state of the startup task (None while running).
    pub task_state: Signal<Option<TaskState>>,
}

impl StartupCtx {
    pub fn new() -> Self {
        Self {
            origin: use_signal(String::new),
            task_id: use_signal(|| None),
            is_completed: use_signal(|| false),
            task_state: use_signal(|| None),
        }
    }

    // ── Convenience accessors ────────────────────────────────────────────

    pub fn get_origin(&self) -> String {
        self.origin.read().clone()
    }

    pub fn is_origin_empty(&self) -> bool {
        self.origin.read().is_empty()
    }

    pub fn is_startup_completed(&self) -> bool {
        *self.is_completed.read()
    }

    pub fn get_startup_task_state(&self) -> Option<TaskState> {
        self.task_state.read().clone()
    }
}
