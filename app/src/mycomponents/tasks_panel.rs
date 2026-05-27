//! Persistent background-task side panel.
//!
//! Mounted inside `ProjectsSidebar` (always visible across all main routes).
//! Provides two things:
//!
//! 1. **Background subscriptions** – re-subscribes to any [`TasksCtx`] task
//!    whose component-level subscription has been dropped (e.g. the user
//!    navigated away mid-task).  This keeps the global state up-to-date and
//!    ensures the "stream closed = success" bug never surfaces after
//!    navigation.
//!
//! 2. **Drawer UI** – a slide-in panel showing all known tasks with their
//!    state, progress bar, and the last ~50 log lines.  A floating badge
//!    button shows the count of currently running tasks.

use crate::server::cancel_task;
use crate::task_manager::{drive_task, TaskEntry, TasksCtx};
use colmap_openmvs_api::TaskState;
use dioxus::prelude::*;
use std::collections::HashSet;

#[component]
pub fn TasksPanel() -> Element {
    let tasks_ctx = use_context::<TasksCtx>();
    let mut panel_open = use_signal(|| false);
    // Track which task IDs already have a background subscription started from
    // this component so we never start duplicates.
    let mut bg_subscribed: Signal<HashSet<String>> = use_signal(HashSet::new);

    // Re-run whenever the task list changes.  Start a background drive_task
    // for every Running task not yet subscribed from here.
    use_effect(move || {
        let to_sub: Vec<String> = tasks_ctx
            .read()
            .tasks
            .iter()
            .filter(|t| t.is_running() && !bg_subscribed.read().contains(&t.id))
            .map(|t| t.id.clone())
            .collect();

        for task_id in to_sub {
            bg_subscribed.write().insert(task_id.clone());
            // No component callback – just keep TasksCtx alive.
            drive_task(task_id, tasks_ctx, |_| {});
        }
    });

    let state = tasks_ctx.read();
    let tasks_snap: Vec<TaskEntry> = state.tasks.clone();
    let running = state.running_count();
    drop(state);

    if tasks_snap.is_empty() {
        return rsx! {};
    }

    rsx! {
        // ── Floating badge button ─────────────────────────────────────────
        button {
            class: if running > 0 { "tasks-fab tasks-fab--active" } else { "tasks-fab" },
            title: "Background tasks",
            onclick: move |_| panel_open.set(!panel_open()),
            span { class: "tasks-fab-icon", "⟳" }
            if running > 0 {
                span { class: "tasks-fab-badge", "{running}" }
            }
        }

        // ── Drawer overlay ────────────────────────────────────────────────
        if panel_open() {
            div {
                class: "tasks-overlay",
                onclick: move |_| panel_open.set(false),
            }
            div {
                class: "tasks-drawer",
                // Header
                div {
                    class: "tasks-drawer-header",
                    h3 { class: "tasks-drawer-title", "Background Tasks" }
                    button {
                        class: "tasks-drawer-close",
                        title: "Close",
                        onclick: move |_| panel_open.set(false),
                        "✕"
                    }
                }
                // Task list
                div {
                    class: "tasks-drawer-body",
                    if tasks_snap.is_empty() {
                        p { class: "tasks-empty", "No tasks yet." }
                    }
                    for entry in tasks_snap.iter() {
                        TaskCard { entry: entry.clone() }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TaskCard
// ---------------------------------------------------------------------------

#[component]
fn TaskCard(entry: TaskEntry) -> Element {
    // Auto-expand running tasks; collapsed by default once done.
    let mut logs_open = use_signal(|| entry.is_running());
    let task_id = entry.id.clone();

    let (status_cls, status_icon, status_label) = match &entry.state {
        TaskState::Running => ("task-card--running", "⟳", "Running"),
        TaskState::Completed => ("task-card--done", "✓", "Done"),
        TaskState::Failed(_) => ("task-card--failed", "✗", "Failed"),
    };

    let error_msg = if let TaskState::Failed(msg) = &entry.state {
        Some(msg.clone())
    } else {
        None
    };

    // Last 50 log lines, in chronological order.
    let log_lines: Vec<String> = entry
        .logs
        .iter()
        .rev()
        .take(50)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    rsx! {
        div {
            class: "task-card {status_cls}",

            // ── Header row ──────────────────────────────────────────────
            div {
                class: "task-card-header",
                onclick: move |_| logs_open.set(!logs_open()),

                span { class: "task-card-icon", title: "{status_label}", "{status_icon}" }

                div { class: "task-card-meta",
                    span { class: "task-card-label", "{entry.label}" }
                    if let Some(ref msg) = error_msg {
                        span { class: "task-card-err", title: "{msg}", "✗ {msg}" }
                    }
                }

                // Cancel button (running tasks only)
                if entry.is_running() {
                    button {
                        class: "task-card-cancel",
                        title: "Cancel task",
                        onclick: move |evt| {
                            evt.stop_propagation();
                            let tid = task_id.clone();
                            spawn(async move { let _ = cancel_task(tid).await; });
                        },
                        "⏹"
                    }
                }

                span {
                    class: "task-card-chevron",
                    if logs_open() { "▾" } else { "▸" }
                }
            }

            // ── Progress bar (running only) ──────────────────────────────
            if let Some(p) = entry.progress {
                if entry.is_running() {
                    div { class: "task-card-progress-track",
                        div {
                            class: "task-card-progress-fill",
                            style: "width: {p * 100.0:.1}%",
                        }
                    }
                }
            }

            // ── Log lines (collapsible) ──────────────────────────────────
            if logs_open() {
                div { class: "task-card-logs",
                    if log_lines.is_empty() {
                        span { class: "task-card-logs-empty", "No output yet…" }
                    }
                    for line in log_lines.iter() {
                        div { class: "task-card-log-line", "{line}" }
                    }
                }
            }
        }
    }
}
