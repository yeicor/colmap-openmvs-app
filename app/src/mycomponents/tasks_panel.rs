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

const MAX_TASKS: usize = 20;

#[component]
pub fn TasksPanel() -> Element {
    let mut tasks_ctx = use_context::<TasksCtx>();
    let mut panel_open = use_signal(|| false);
    // Track which task IDs already have a background subscription started from
    // this component so we never start duplicates.
    let mut bg_subscribed: Signal<HashSet<String>> = use_signal(HashSet::new);

    // Subscribe to running tasks on mount only — don't re-run on every tasks_ctx change.
    use_memo(move || {
        let to_sub: Vec<String> = tasks_ctx
            .read()
            .tasks
            .iter()
            .filter(|t| t.is_running() && !bg_subscribed.read().contains(&t.id))
            .map(|t| t.id.clone())
            .collect();

        if !to_sub.is_empty() {
            for task_id in to_sub {
                bg_subscribed.write().insert(task_id.clone());
                drive_task(task_id, tasks_ctx, |_| {});
            }
        }
    });

    // Scroll drawer to top (showing newest) when a new task arrives while open.
    let mut prev_task_count = use_signal(|| 0usize);
    use_effect(move || {
        let current_count = tasks_ctx.read().tasks.len();
        if panel_open() && current_count > *prev_task_count.peek() {
            let _ = dioxus::document::eval(
                "setTimeout(() => { \
                    const body = document.querySelector('.tasks-drawer-body'); \
                    if (body) body.scrollTop = 0; \
                }, 0)",
            );
        }
        prev_task_count.set(current_count);
    });

    let state = tasks_ctx.read();
    let mut tasks_snap: Vec<TaskEntry> = state.tasks.clone();
    let running = state.running_count();
    let completed_count = tasks_snap.iter().filter(|t| t.is_terminal()).count();
    drop(state);

    if tasks_snap.is_empty() {
        return rsx! {};
    }

    // Reverse to show newest tasks first
    tasks_snap.reverse();

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
                // Header with action buttons
                div {
                    class: "tasks-drawer-header",
                    h3 { class: "tasks-drawer-title", "Background Tasks" }
                    div { class: "tasks-drawer-actions",
                        // Forget completed button
                        if completed_count > 0 {
                            button {
                                class: "tasks-drawer-action-btn tasks-drawer-action-forget-completed",
                                title: "Forget all completed tasks",
                                onclick: move |_| {
                                    tasks_ctx.write().forget_completed();
                                },
                                "🗑 Completed"
                            }
                        }
                        // Close button
                        button {
                            class: "tasks-drawer-close",
                            title: "Close",
                            onclick: move |_| panel_open.set(false),
                            "✕"
                        }
                    }
                }
                // Task list
                div {
                    class: "tasks-drawer-body",
                    if tasks_snap.is_empty() {
                        p { class: "tasks-empty", "No tasks yet." }
                    }
                    for entry in tasks_snap.iter() {
                        TaskCard { entry: entry.clone(), tasks_ctx }
                    }
                    // Overflow indicator
                    if tasks_snap.len() >= MAX_TASKS {
                        div { class: "tasks-overflow-notice",
                            "⚠ Oldest tasks will be removed when limit is reached"
                        }
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
fn TaskCard(entry: TaskEntry, tasks_ctx: TasksCtx) -> Element {
    // Auto-expand running tasks; collapsed by default once done.
    let mut logs_open = use_signal(|| entry.is_running());
    let task_id = entry.id.clone();
    let task_id_forget = entry.id.clone();

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

    // Logs in chronological order (oldest first), last 50 lines only.
    let log_lines: Vec<String> = entry
        .logs
        .iter()
        .skip(entry.logs.len().saturating_sub(50))
        .cloned()
        .collect();

    // Auto-scroll effect: when logs change and container is open, scroll to bottom
    // and ensure the task card is visible in the drawer.
    let log_lines_for_effect = log_lines.clone();
    use_effect(move || {
        if logs_open() && !log_lines_for_effect.is_empty() {
            let _ = dioxus::document::eval(
                r#"
                setTimeout(() => {
                    const logs = document.querySelector('.task-card-logs');
                    if (logs) {
                        logs.scrollTop = logs.scrollHeight;
                        const card = logs.closest('.task-card');
                        const drawer = document.querySelector('.tasks-drawer-body');
                        if (card && drawer) {
                            const cardBottom = card.offsetTop + card.offsetHeight;
                            const drawerScrollBottom = drawer.scrollTop + drawer.clientHeight;
                            if (cardBottom > drawerScrollBottom || card.offsetTop < drawer.scrollTop) {
                                card.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
                            }
                        }
                    }
                }, 0);
                "#,
            );
        }
    });

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

                // Action buttons
                div { class: "task-card-actions",
                    // Cancel button (running tasks only)
                    if entry.is_running() {
                        button {
                            class: "task-card-action-btn task-card-cancel",
                            title: "Cancel task",
                            onclick: move |evt| {
                                evt.stop_propagation();
                                let tid = task_id.clone();
                                spawn(async move { let _ = cancel_task(tid).await; });
                            },
                            "⏹"
                        }
                    }

                    // Forget button (completed/failed tasks only)
                    if entry.is_terminal() {
                        button {
                            class: "task-card-action-btn task-card-forget",
                            title: "Forget this task",
                            onclick: move |evt| {
                                evt.stop_propagation();
                                let tid = task_id_forget.clone();
                                tasks_ctx.write().forget_task(&tid);
                            },
                            "✕"
                        }
                    }

                    span {
                        class: "task-card-chevron",
                        if logs_open() { "▾" } else { "▸" }
                    }
                }
            }

            // ── Progress bar (running only) ──────────────────────────────
            if let Some(p) = entry.display_progress() {
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
