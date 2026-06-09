//! System startup tasks view.
//!
//! Shows progress of background startup steps (platform-specific setup such
//! as Android runtime environment preparation).  The view is completely
//! generic — startup steps are defined on the server and communicated via
//! [`TaskEvent::Log`] / [`TaskEvent::Completed`] / [`TaskEvent::Failed`].

use colmap_openmvs_api::TaskEvent;
use colmap_openmvs_api::TaskState;
use dioxus::prelude::*;
use std::collections::VecDeque;

use crate::server::poll_task_events;

const POLL_INTERVAL_MS: u64 = 300;

async fn sleep_poll() {
    #[cfg(not(target_arch = "wasm32"))]
    tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
}

#[derive(Clone)]
struct LogEntry {
    message: String,
}

#[component]
pub fn StartupTasks() -> Element {
    let mut logs = use_signal(VecDeque::<LogEntry>::new);

    // Pick up the already-running startup task from the shared context.
    // The task was kicked off by `App` immediately on boot.
    let mut startup = use_context::<crate::task_manager::StartupCtx>();
    let mut local_task_state = use_signal(|| None::<TaskState>);

    // Stream server events into the local log buffer.
    // We only mount this component when a redirect to /startup happened,
    // so the task should already have a task_id.
    let task_id = startup.task_id;
    use_effect(move || {
        let id = match task_id() {
            Some(id) => id,
            None => return, // not yet started – unlikely, but be safe
        };
        spawn(async move {
            // If the task already finished before we mounted, skip streaming.
            if startup.is_startup_completed() {
                local_task_state.set(Some(
                    startup
                        .get_startup_task_state()
                        .unwrap_or(TaskState::Completed),
                ));
                return;
            }
            let mut cursor = 0usize;
            loop {
                match poll_task_events(id.clone(), cursor, None).await {
                    Ok(batch) => {
                        if !batch.task_found {
                            break;
                        }
                        cursor = batch.cursor;
                        for event in batch.events {
                            match event {
                                TaskEvent::Log(msg) => {
                                    logs.with_mut(|l| l.push_back(LogEntry { message: msg }));
                                }
                                TaskEvent::Completed => {
                                    logs.with_mut(|l| {
                                        l.push_back(LogEntry {
                                            message: "Startup completed successfully.".to_string(),
                                        })
                                    });
                                    local_task_state.set(Some(TaskState::Completed));
                                    return;
                                }
                                TaskEvent::Failed(msg) => {
                                    logs.with_mut(|l| {
                                        l.push_back(LogEntry {
                                            message: format!("Error: {}", msg),
                                        })
                                    });
                                    local_task_state.set(Some(TaskState::Failed(msg)));
                                    return;
                                }
                                _ => {}
                            }
                        }
                        if batch.is_terminal {
                            // Sync local state from the shared context.
                            if startup.is_startup_completed() {
                                let st = startup
                                    .get_startup_task_state()
                                    .unwrap_or(TaskState::Completed);
                                local_task_state.set(Some(st));
                            } else {
                                // Terminal but no explicit event — assume completed.
                                local_task_state.set(Some(TaskState::Completed));
                            }
                            return;
                        }
                    }
                    Err(e) => {
                        logs.with_mut(|l| {
                            l.push_back(LogEntry {
                                message: format!("Failed to poll events: {}", e),
                            })
                        });
                        local_task_state
                            .set(Some(TaskState::Failed("Event polling failed".to_string())));
                        return;
                    }
                }
                sleep_poll().await;
            }
        });
    });

    // React to the shared completion flags so we redirect to origin.
    let is_running = match (local_task_state(), startup.is_startup_completed()) {
        (Some(TaskState::Running), _) | (None, false) => true,
        _ => false,
    };

    let on_retry = move |_: Event<MouseData>| {
        // Reset shared state and restart the startup task.
        startup.task_id.set(None);
        startup.is_completed.set(false);
        startup.task_state.set(None);
        local_task_state.set(None);
        logs.set(VecDeque::new());
        spawn(async move {
            match crate::server::startup().await {
                Ok(id) => {
                    startup.task_id.set(Some(id.clone()));
                    let mut cursor = 0usize;
                    loop {
                        match crate::server::poll_task_events(id.clone(), cursor, None).await {
                            Ok(batch) => {
                                if !batch.task_found {
                                    startup.is_completed.set(true);
                                    startup.task_state.set(Some(TaskState::Completed));
                                    return;
                                }
                                cursor = batch.cursor;
                                for event in batch.events {
                                    match event {
                                        TaskEvent::Log(msg) => {
                                            logs.with_mut(|l| {
                                                l.push_back(LogEntry { message: msg })
                                            });
                                        }
                                        TaskEvent::Completed => {
                                            logs.with_mut(|l| {
                                                l.push_back(LogEntry {
                                                    message: "Startup completed.".to_string(),
                                                })
                                            });
                                            startup.is_completed.set(true);
                                            startup.task_state.set(Some(TaskState::Completed));
                                            return;
                                        }
                                        TaskEvent::Failed(msg) => {
                                            logs.with_mut(|l| {
                                                l.push_back(LogEntry {
                                                    message: format!("Error: {}", msg),
                                                })
                                            });
                                            startup.is_completed.set(true);
                                            startup.task_state.set(Some(TaskState::Failed(msg)));
                                            return;
                                        }
                                        _ => {}
                                    }
                                }
                                if batch.is_terminal {
                                    // Sync local state from the shared context
                                    // (retry spawn shares the same StartupCtx).
                                    if startup.is_startup_completed() {
                                        let st = startup
                                            .get_startup_task_state()
                                            .unwrap_or(TaskState::Completed);
                                        local_task_state.set(Some(st));
                                    } else {
                                        local_task_state.set(Some(TaskState::Completed));
                                    }
                                    return;
                                }
                            }
                            Err(e) => {
                                logs.with_mut(|l| {
                                    l.push_back(LogEntry {
                                        message: format!("Failed to poll events: {}", e),
                                    })
                                });
                                startup.is_completed.set(true);
                                startup.task_state.set(Some(TaskState::Failed(
                                    "Event polling failed".to_string(),
                                )));
                                local_task_state.set(Some(TaskState::Failed(
                                    "Event polling failed".to_string(),
                                )));
                                return;
                            }
                        }
                        sleep_poll().await;
                    }
                }
                Err(e) => {
                    logs.with_mut(|l| {
                        l.push_back(LogEntry {
                            message: format!("Failed to start: {}", e),
                        })
                    });
                    startup.is_completed.set(true);
                    startup
                        .task_state
                        .set(Some(TaskState::Failed("Failed to start".to_string())));
                }
            }
        });
    };

    // Auto-navigate back to the original URL when the task completes (or fails).
    // Capture the navigator at the component level so it's available
    // inside the spawned future.
    let navigator = use_navigator();
    use_effect(move || {
        // Read signals inside the closure so the effect re-runs when they change.
        // Only auto-redirect on successful completion (shared state).
        // On failure the user must click Ignore or Retry.
        if startup.is_startup_completed()
            && matches!(startup.get_startup_task_state(), Some(TaskState::Completed))
        {
            // Small delay so the user can briefly see the outcome in the logs.
            spawn(async move {
                for _ in 0..10 {
                    sleep_poll().await;
                }
                let target = if startup.is_origin_empty() {
                    "/".to_string()
                } else {
                    startup.get_origin()
                };
                navigator.push(target);
            });
        }
    });

    let completed = startup.is_startup_completed()
        && matches!(startup.get_startup_task_state(), Some(TaskState::Completed));
    let is_failed = startup.is_startup_completed()
        && matches!(startup.get_startup_task_state(), Some(TaskState::Failed(_)));

    let card_mod = if completed {
        "startup-card--done"
    } else if is_failed {
        "startup-card--failed"
    } else {
        "startup-card--running"
    };

    let icon_char = if completed {
        "\u{2713}" // ✓
    } else if is_failed {
        "\u{2717}" // ✗
    } else {
        "\u{2699}" // ⚙
    };

    let logs_empty = logs.read().is_empty();

    rsx! {
        div { class: "startup-wrapper",
            div { class: "startup-card {card_mod}",
                // ── Header ────────────────────────────────────────────
                div { class: "startup-header",
                    span { class: "startup-icon", "{icon_char}" }
                    div { class: "startup-title-group",
                        h1 { class: "startup-title", "System Startup" }
                        p { class: "startup-subtitle",
                            if completed {
                                "All checks passed"
                            } else if is_failed {
                                "Startup failed"
                            } else {
                                "Running platform startup tasks…"
                            }
                        }
                    }
                }
                // ── Logs ─────────────────────────────────────────────
                div { class: "startup-logs",
                    if logs_empty {
                        div { class: "startup-logs-empty", "Waiting for logs…" }
                    } else {
                        for entry in logs.read().iter() {
                            div { class: "startup-log-line", "{entry.message}" }
                        }
                    }
                }
                // ── Footer ───────────────────────────────────────────
                div { class: "startup-footer",
                    if completed {
                        span { class: "startup-status-text startup-status-text--done",
                            "\u{2713} Setup complete \u{2014} redirecting…"
                        }
                    } else if is_failed {
                        span { class: "startup-status-text startup-status-text--failed",
                            "\u{2717} Startup failed"
                        }
                        button {
                            class: "startup-retry-btn",
                            onclick: on_retry,
                            "\u{21BB} Retry"
                        }
                        button {
                            class: "startup-retry-btn",
                            onclick: move |_| {
                                // Assume success and carry on.
                                startup.is_completed.set(true);
                                startup.task_state.set(Some(TaskState::Completed));
                                local_task_state.set(Some(TaskState::Completed));
                            },
                            "Ignore"
                        }
                    } else if is_running {
                        span { class: "startup-status-text",
                            "\u{2699} Please wait…"
                        }
                    }
                }
            }
        }
    }
}
