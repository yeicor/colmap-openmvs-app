//! Android startup tasks view for validating and repairing settings.

use colmap_openmvs_api::TaskEvent;
use colmap_openmvs_api::TaskState;
use dioxus::prelude::*;
use std::collections::VecDeque;

use crate::server::{poll_task_events, repair_android_settings};

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
    let mut task_id = use_signal(|| None::<String>);
    let mut logs = use_signal(VecDeque::<LogEntry>::new);
    let mut task_state = use_signal(|| None::<TaskState>);
    let mut is_completed = use_signal(|| false);

    // Auto-start the repair task on mount, then stream its events.
    use_effect(move || {
        if task_id().is_none() {
            spawn(async move {
                match repair_android_settings().await {
                    Ok(id) => {
                        task_id.set(Some(id.clone()));
                        stream_task_events(id, logs, task_state, is_completed).await;
                    }
                    Err(e) => {
                        logs.with_mut(|l| {
                            l.push_back(LogEntry {
                                message: format!("Failed to start repair: {}", e),
                            })
                        });
                        task_state.set(Some(TaskState::Failed(
                            "Failed to start repair".to_string(),
                        )));
                    }
                }
            });
        }
    });

    let is_running = match task_state() {
        Some(TaskState::Running) => true,
        None => task_id().is_some(),
        _ => false,
    };
    let is_failed = matches!(task_state(), Some(TaskState::Failed(_)));
    let completed = is_completed();

    let on_retry = move |_: Event<MouseData>| {
        task_id.set(None);
        logs.set(VecDeque::new());
        task_state.set(None);
        is_completed.set(false);
    };

    let on_continue = move |_: Event<MouseData>| {
        use_navigator().push("/");
    };

    rsx! {
        div {
            class: "startup-tasks-container",
            div {
                class: "startup-header",
                h1 { "Android System Setup" }
                p { "Validating and repairing system configuration..." }
            }

            div {
                class: "startup-logs",
                for entry in logs.read().iter() {
                    div { class: "log-line", "{entry.message}" }
                }
            }

            div {
                class: "startup-buttons",
                if completed {
                    button {
                        class: "action-btn action-btn-primary",
                        onclick: on_continue,
                        "Continue"
                    }
                } else if is_failed {
                    button {
                        class: "action-btn",
                        onclick: on_retry,
                        "Retry"
                    }
                    button {
                        class: "action-btn action-btn-primary",
                        onclick: on_continue,
                        "Skip"
                    }
                } else if is_running {
                    div {
                        class: "spinner",
                        "Please wait..."
                    }
                }
            }
        }
    }
}

/// Poll a task's event stream and update signals as events arrive.
async fn stream_task_events(
    id: String,
    mut logs: Signal<VecDeque<LogEntry>>,
    mut task_state: Signal<Option<TaskState>>,
    mut is_completed: Signal<bool>,
) {
    logs.with_mut(|l| {
        l.push_back(LogEntry {
            message: "Android settings validation starting...".to_string(),
        })
    });

    let mut cursor = 0usize;

    loop {
        match poll_task_events(id.clone(), cursor).await {
            Ok(batch) => {
                if !batch.task_found {
                    // Task gone — treat as completed.
                    task_state.set(Some(TaskState::Completed));
                    is_completed.set(true);
                    return;
                }

                cursor = batch.cursor;
                let is_terminal = batch.is_terminal;

                for event in batch.events {
                    match event {
                        TaskEvent::Log(msg) => {
                            logs.with_mut(|l| l.push_back(LogEntry { message: msg }));
                        }
                        TaskEvent::Completed => {
                            logs.with_mut(|l| {
                                l.push_back(LogEntry {
                                    message: "Android settings repair completed successfully."
                                        .to_string(),
                                })
                            });
                            task_state.set(Some(TaskState::Completed));
                            is_completed.set(true);
                            return;
                        }
                        TaskEvent::Failed(msg) => {
                            logs.with_mut(|l| {
                                l.push_back(LogEntry {
                                    message: format!("Error: {}", msg),
                                })
                            });
                            task_state.set(Some(TaskState::Failed(msg)));
                            return;
                        }
                        _ => {}
                    }
                }

                if is_terminal {
                    return;
                }
            }
            Err(e) => {
                logs.with_mut(|l| {
                    l.push_back(LogEntry {
                        message: format!("Failed to poll events: {}", e),
                    })
                });
                task_state.set(Some(TaskState::Failed("Event polling failed".to_string())));
                return;
            }
        }

        sleep_poll().await;
    }
}
