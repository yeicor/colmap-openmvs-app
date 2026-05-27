//! Android startup tasks view for validating and repairing settings.

use colmap_openmvs_api::TaskEvent;
use colmap_openmvs_api::TaskState;
use dioxus::prelude::*;
use std::collections::VecDeque;

use crate::server::{repair_android_settings, subscribe_task_events};

#[derive(Clone)]
struct LogEntry {
    message: String,
}

#[component]
pub fn StartupTasks() -> Element {
    let mut task_id = use_signal(|| None::<String>);
    let mut logs = use_signal(|| VecDeque::<LogEntry>::new());
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
                        class: "btn btn-primary",
                        onclick: on_continue,
                        "Continue"
                    }
                } else if is_failed {
                    button {
                        class: "btn btn-secondary",
                        onclick: on_retry,
                        "Retry"
                    }
                    button {
                        class: "btn btn-primary",
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

/// Subscribe to a repair task's event stream and update signals as events arrive.
/// Handles keep-alives by ignoring them.
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

    match subscribe_task_events(id).await {
        Ok(mut stream) => {
            while let Some(Ok(event)) = stream.recv().await {
                match event {
                    TaskEvent::Log(msg) if msg.contains("Keep-alive") => {
                        // ignore heartbeats
                    }
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
            // Stream ended without terminal event – fall back to completed.
            task_state.set(Some(TaskState::Completed));
            is_completed.set(true);
        }
        Err(e) => {
            logs.with_mut(|l| {
                l.push_back(LogEntry {
                    message: format!("Failed to subscribe to events: {}", e),
                })
            });
            task_state.set(Some(TaskState::Failed(
                "Event subscription failed".to_string(),
            )));
        }
    }
}
