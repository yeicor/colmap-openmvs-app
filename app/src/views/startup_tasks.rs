//! Android startup tasks view for validating and repairing settings

use colmap_openmvs_api::TaskState;
use dioxus::prelude::*;
use std::collections::VecDeque;

use crate::server::{get_task_info, repair_android_settings};

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

    // Auto-start the repair task on mount
    {
        let mut tid = task_id;
        let mut lg = logs;
        let mut ts = task_state;
        let ic = is_completed;

        use_effect(move || {
            if tid().is_none() {
                spawn(async move {
                    match repair_android_settings().await {
                        Ok(id) => {
                            tid.set(Some(id.clone()));
                            poll_task_status(id, lg, ts, ic).await;
                        }
                        Err(e) => {
                            let err_msg = format!("Failed to start repair: {}", e);
                            lg.with_mut(|l| l.push_back(LogEntry { message: err_msg }));
                            ts.set(Some(TaskState::Failed(
                                "Failed to start repair".to_string(),
                            )));
                        }
                    }
                });
            }
        });
    }

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
        let navigator = use_navigator();
        navigator.push("/");
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
                {logs.read().iter().map(|entry| {
                    rsx! {
                        div {
                            class: "log-line",
                            "{entry.message}"
                        }
                    }
                })}
            }

            div {
                class: "startup-buttons",
                {if completed {
                    rsx! {
                        button {
                            class: "btn btn-primary",
                            onclick: on_continue,
                            "Continue"
                        }
                    }
                } else if is_failed {
                    rsx! {
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
                    }
                } else if is_running {
                    rsx! {
                        div {
                            class: "spinner",
                            "Please wait..."
                        }
                    }
                } else {
                    rsx! {}
                }}
            }
        }
    }
}

async fn poll_task_status(
    task_id: String,
    mut logs: Signal<VecDeque<LogEntry>>,
    mut task_state: Signal<Option<TaskState>>,
    mut is_completed: Signal<bool>,
) {
    logs.with_mut(|l| {
        l.push_back(LogEntry {
            message: "Android settings validation starting...".to_string(),
        });
    });

    loop {
        // Poll for task status
        match get_task_info(task_id.clone()).await {
            Ok(Some(info)) => {
                match &info.state {
                    TaskState::Completed => {
                        logs.with_mut(|l| {
                            l.push_back(LogEntry {
                                message: "Android settings repair completed successfully."
                                    .to_string(),
                            });
                        });
                        task_state.set(Some(TaskState::Completed));
                        is_completed.set(true);
                        break;
                    }
                    TaskState::Failed(msg) => {
                        logs.with_mut(|l| {
                            l.push_back(LogEntry {
                                message: format!("Error: {}", msg),
                            });
                        });
                        task_state.set(Some(TaskState::Failed(msg.clone())));
                        break;
                    }
                    TaskState::Running => {
                        // Continue polling
                    }
                }
            }
            Ok(None) => {
                logs.with_mut(|l| {
                    l.push_back(LogEntry {
                        message: "Task not found".to_string(),
                    });
                });
                task_state.set(Some(TaskState::Failed("Task not found".to_string())));
                break;
            }
            Err(e) => {
                logs.with_mut(|l| {
                    l.push_back(LogEntry {
                        message: format!("Error checking status: {}", e),
                    });
                });
            }
        }
    }
}
