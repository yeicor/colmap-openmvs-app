// Initialize Android runtime environment on first server startup
#[cfg(target_os = "android")]
static ANDROID_STARTUP: std::sync::OnceLock<std::sync::Arc<tokio::sync::Mutex<bool>>> =
    std::sync::OnceLock::new();

pub async fn on_backend_started() -> dioxus::Result<()> {
    tracing::info!(url = %dioxus::cli_config::fullstack_address_or_localhost().to_string(), "Server listening for connections");

    // Log Docker-in-Docker path translation status so it's visible at startup
    {
        let dind_summary = crate::runtimes::docker_dind::diagnostic_summary();
        let dind_active = crate::runtimes::docker_dind::is_active();
        if dind_active {
            tracing::info!("DinD path translation: active\n{}", dind_summary);
        } else {
            tracing::debug!("DinD path translation: {}", dind_summary);
        }
    }
    #[cfg(target_os = "android")]
    {
        use tracing::{debug, info, warn};

        debug!("- Initializing android runtime");
        let started =
            ANDROID_STARTUP.get_or_init(|| std::sync::Arc::new(tokio::sync::Mutex::new(false)));

        let mut done = started.lock().await;
        if !*done {
            // First repair any invalid settings paths from a previous app install
            if let Err(e) = crate::repair_android_settings().await {
                warn!(error = %e, "Android startup: settings repair failed or skipped");
            }
            // Then set up the runtime with the repaired/validated settings
            match crate::setup_android_runtime().await {
                Ok(()) => info!("Android startup: completed successfully"),
                Err(e) => warn!(error = %e, "Android startup: failed or skipped"),
            }
            *done = true;
        }
    }

    Ok(())
}

pub async fn on_frontend_started() -> dioxus::Result<()> {
    Ok(())
}
