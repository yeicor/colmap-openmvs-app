//! Server startup — runs all registered startup steps in a background task.
//!
//! The public [`startup`] function creates a task in the global task registry
//! and spawns a future that executes every registered startup step.  Each step
//! is simply a `#[cfg]`-guarded block inside the future so new steps can be
//! added without changing the API signature.

use crate::task_registry::{publish_event, TASK_REGISTRY};
use colmap_openmvs_api::TaskEvent;
use colmap_openmvs_api::TaskKind;

/// Run all registered startup steps in a background task and return its ID.
///
/// The caller should poll events via the standard task-event API to observe
/// progress.  Each step emits [`TaskEvent::Log`] before and after.
pub async fn startup() -> dioxus::Result<String> {
    tracing::info!(
        fullstack_address_or_localhost =
            dioxus::cli_config::fullstack_address_or_localhost().to_string(),
        server_url = dioxus::fullstack::get_server_url(),
        "Server listening for connections"
    );

    // DinD diagnostics (logged here once, not per-step)
    {
        let dind_summary = crate::runtimes::docker_dind::diagnostic_summary();
        let dind_active = crate::runtimes::docker_dind::is_active();
        if dind_active {
            tracing::info!("DinD path translation: active\n{dind_summary}");
        } else {
            tracing::debug!("DinD path translation: {dind_summary}");
        }
    }

    let task_id = TASK_REGISTRY.create_task(TaskKind::Startup, "system".into());
    let tid = task_id.clone();

    tokio::spawn(async move {
        // ═══════════════════════════════════════════════════════════════
        // Android: validate settings & set up runtime
        // ═══════════════════════════════════════════════════════════════
        //
        // `repair_paths()` checks whether the settings paths are still valid
        // (after a reinstall the JNI lib directory changes).  If it repaired
        // something it also calls `setup_android_runtime()` internally, so we
        // only need to call the latter when no repair was required.
        #[cfg(target_os = "android")]
        {
            publish_event(
                &tid,
                TaskEvent::Log("[1/1] Android environment setup…".into()),
            );

            match crate::android_settings_validation::AndroidSettingsValidation::repair_paths()
                .await
            {
                Ok(true) => {
                    // repair_paths() already called setup_android_runtime().
                    publish_event(
                        &tid,
                        TaskEvent::Log("[1/1] Android environment setup — done".into()),
                    );
                }
                Ok(false) => {
                    // No repair needed — set up runtime directly.
                    publish_event(
                        &tid,
                        TaskEvent::Log("[1/1] Android settings valid, setting up runtime…".into()),
                    );
                    match crate::android_startup::setup_android_runtime().await {
                        Ok(()) => {
                            publish_event(
                                &tid,
                                TaskEvent::Log("[1/1] Android environment setup — done".into()),
                            );
                        }
                        Err(e) => {
                            let msg = format!("[1/1] Android environment setup — failed: {e}");
                            publish_event(&tid, TaskEvent::Log(msg.clone()));
                            publish_event(&tid, TaskEvent::Failed(msg));
                            return;
                        }
                    }
                }
                Err(e) => {
                    let msg = format!("[1/1] Android environment setup — failed: {e}");
                    publish_event(&tid, TaskEvent::Log(msg.clone()));
                    publish_event(&tid, TaskEvent::Failed(msg));
                    return;
                }
            }
        }

        // ═══════════════════════════════════════════════════════════════
        // Future startup steps can be added here with the same pattern:
        //
        //   #[cfg(target_os = "...")]
        //   {
        //       publish_event(&tid, TaskEvent::Log("…".into()));
        //       match some_fn().await { … }
        //   }
        // ═══════════════════════════════════════════════════════════════

        #[cfg(not(target_os = "android"))]
        {
            publish_event(
                &tid,
                TaskEvent::Log("No platform-specific startup steps needed.".into()),
            );
        }

        publish_event(
            &tid,
            TaskEvent::Log("All startup steps completed successfully.".into()),
        );
        publish_event(&tid, TaskEvent::Completed);
    });

    Ok(task_id)
}
