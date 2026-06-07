//! Client-side UI code for colmap-openmvs-app
//!
//! This package contains all client-side UI components, views, and the main application entry point.
//! It imports from the server package for types and function calls.

use colmap_openmvs_api::{TaskEvent, TaskState};
use dioxus::prelude::*;
use tracing::info;
pub mod backend_url;
pub mod components;
pub mod fullstack_compat;
pub mod logging;
pub mod mycomponents;
pub mod picker;
pub mod server;
pub mod task_manager;
pub mod viewer_conversion;
pub mod views;

#[cfg(feature = "demo")]
pub mod demo;

use logging::init as init_logging;
pub use views::{
    ProjectConfig, ProjectImages, ProjectLogs, ProjectOutputs, ProjectOverview, ProjectPage,
    Projects, ProjectsSidebar, SettingsGeneral, SettingsPageLayout, SettingsRuntime, StartupTasks,
};

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(ProjectsSidebar)]
        #[route("/")]
        Projects {},
        #[route("/settings")]
        SettingsGeneral {},
        #[route("/settings/runtime")]
        SettingsRuntime {},
        #[route("/project/:name")]
        ProjectOverview { name: String },
        #[route("/project/:name/images")]
        ProjectImages { name: String },
        #[route("/project/:name/config")]
        ProjectConfig { name: String },
        #[route("/project/:name/logs")]
        ProjectLogs { name: String },
        #[route("/project/:name/outputs")]
        ProjectOutputs { name: String },
    #[route("/startup")]
    StartupTasks {},
}

#[component]
pub fn App() -> Element {
    info!("App component: initializing...");
    use crate::mycomponents::ToastContainer;
    use crate::task_manager::{StartupCtx, TasksCtx, TasksState};
    use_context_provider(|| Signal::new(TasksState::default()) as TasksCtx);

    // Global toast notification system (single container for all floating toasts).
    mycomponents::use_toast_provider();

    // Shared startup task state — kick off the server startup immediately so it can
    // finish within the 1-second grace window without ever showing the startup
    // page.  Components inside the router tree (ProjectsSidebar, StartupTasks)
    // consume this context to decide whether / where to redirect.
    info!("App component: creating startup context...");
    let startup = StartupCtx::new();
    use_context_provider(|| startup);

    {
        // Start the startup task in background as early as possible.
        info!("App component: spawning startup task...");
        let mut task_id = startup.task_id;
        let mut is_completed = startup.is_completed;
        let mut task_state = startup.task_state;
        spawn(async move {
            match crate::server::startup().await {
                Ok(id) => {
                    task_id.set(Some(id.clone()));
                    let mut cursor = 0usize;
                    loop {
                        match crate::server::poll_task_events(id.clone(), cursor).await {
                            Ok(batch) => {
                                if !batch.task_found {
                                    is_completed.set(true);
                                    task_state.set(Some(TaskState::Completed));
                                    return;
                                }
                                cursor = batch.cursor;
                                if batch.is_terminal {
                                    let mut found_terminal = false;
                                    for event in batch.events {
                                        if matches!(event, TaskEvent::Completed) {
                                            is_completed.set(true);
                                            task_state.set(Some(TaskState::Completed));
                                            found_terminal = true;
                                        } else if let TaskEvent::Failed(msg) = event {
                                            is_completed.set(true);
                                            task_state.set(Some(TaskState::Failed(msg)));
                                            found_terminal = true;
                                        }
                                    }
                                    if !found_terminal {
                                        // Terminal batch with no explicit event =
                                        // treat as completed.
                                        is_completed.set(true);
                                        task_state.set(Some(TaskState::Completed));
                                    }
                                    return;
                                }
                            }
                            Err(e) => {
                                // Non-fatal; keep polling on transient failures.
                                tracing::warn!(
                                    error = %e,
                                    "Startup task poll failed; retrying"
                                );
                            }
                        }
                        // Re-use task_manager's sleep helper.
                        crate::task_manager::sleep_poll().await;
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to start startup task");
                    is_completed.set(true);
                    task_state.set(Some(TaskState::Failed(e.to_string())));
                }
            }
        });
    }

    // Fetch the server-side color-scheme preference once on startup.
    // On Android the WebView may not propagate `prefers-color-scheme` CSS media
    // queries correctly, so the server returns an explicit override (`Some`).
    // On other platforms the server returns `None` and we leave the `data-theme`
    // attribute untouched so the CSS media query continues to work normally.
    info!("App component: setting up color-scheme effect...");
    use_effect(move || {
        spawn(async move {
            match crate::server::get_dark_mode().await {
                Ok(Some(is_dark)) => {
                    let theme = if is_dark { "dark" } else { "light" };
                    let _ = dioxus::document::eval(&format!(
                        "document.documentElement.setAttribute('data-theme', '{theme}');"
                    ));
                }
                Ok(None) => {} // Let CSS media query handle it
                Err(e) => tracing::warn!(error = %e, "Failed to fetch dark-mode preference"),
            }
        });
    });

    info!("App component: rendering UI...");
    rsx! {
        document::Link { rel: "icon", type: "image/png", href: asset!("/assets/icon.png") }
        // ── Global stylesheets (loaded eagerly, no flashing on route changes) ────────────────
        document::Link { rel: "stylesheet", href: asset!("/assets/main.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("/assets/dx-components-theme.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("/assets/dx-components-theme-override.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("/assets/mycomponents.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("/assets/tasks-panel.css", AssetOptions::css().with_preload(true)) }
        // ── View stylesheets (preloaded to avoid FOUC) ───────────────────
        document::Link { rel: "stylesheet", href: asset!("/assets/views/startup.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("/assets/views/projects.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("/assets/views/settings.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("/assets/views/project.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("/assets/views/project/images.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("/assets/views/project/config.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("/assets/views/project/logs.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("/assets/views/project/outputs.css", AssetOptions::css().with_preload(true)) }
        // ── Component-library stylesheets (preloaded to avoid FOUC) ──────
        document::Link { rel: "stylesheet", href: asset!("./components/alert_dialog/style.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("./components/button/style.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("./components/progress/style.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("./components/separator/style.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("./components/sheet/style.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("./components/sidebar/style.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("./components/tabs/style.css", AssetOptions::css().with_preload(true)) }
        document::Link { rel: "stylesheet", href: asset!("./components/tooltip/style.css", AssetOptions::css().with_preload(true)) }
        Router::<Route> {}
        ToastContainer {}
    }
}

fn init_backend_url() {
    #[cfg(not(any(feature = "demo", feature = "server")))]
    {
        // Resolve the backend URL from URL params / localStorage before launching.
        // On web (WASM) this also calls `dioxus::fullstack::set_server_url` so that
        // all generated server-function HTTP requests go to the configured origin.
        let mut backend_url_str = backend_url::read_initial_backend_url();

        // Validate the backend URL using `http::Uri` parsing (same as Dioxus
        // uses internally). If invalid, warn and reset to empty (same-origin
        // fallback) so the app doesn't silently fail on all requests.
        if !backend_url_str.is_empty() && !backend_url::is_valid_backend_url(&backend_url_str) {
            tracing::warn!(
                url = %backend_url_str,
                "Invalid backend URL configured — resetting to empty (same-origin)"
            );
            backend_url::save_backend_url("");
            backend_url_str = String::new();
        }
        backend_url::BACKEND_URL.set(backend_url_str.clone()).ok();
        if !backend_url_str.is_empty() {
            // Strip trailing slash to avoid double-slash when Dioxus prepends the
            // server URL to paths that start with `/` (e.g. `/api/startup`).
            // Without this, "http://127.0.0.1:8080/" + "/api/startup" produces
            // "http://127.0.0.1:8080//api/startup", whose path `//api/startup`
            // is treated as a protocol-relative URL by the browser.
            let normalized = backend_url_str.trim_end_matches('/').to_string();
            let leaked: &'static str = Box::leak(normalized.into_boxed_str());
            dioxus::fullstack::set_server_url(leaked);
        }
        info!(
            url = %backend_url::BACKEND_URL.get().unwrap_or(&"<empty>".to_string()),
            "Backend URL resolved",
        );
    }
}

pub fn log_build_info() {
    info!(
        event = "build_info",
        package.name = env!("CARGO_PKG_NAME"),
        package.version = env!("CARGO_PKG_VERSION"),
        build.date = env!("BUILD_DATE"),
        build.profile = env!("PROFILE"),
        build.target = env!("TARGET"),
        git.branch = env!("GIT_BRANCH"),
        git.tag = env!("GIT_TAG"),
        git.hash = env!("GIT_HASH"),
        git.hash_full = env!("GIT_HASH_FULL"),
        git.dirty = env!("GIT_DIRTY"),
        rust.version = env!("RUSTC_VERSION"),
        "Build information"
    );
}

fn main() {
    init_logging();
    log_build_info();

    // Parse CLI arguments and merge with config file / env vars.
    // This also initializes the global settings singleton.
    // Skipped on WASM (client side) where CLI args don't exist,
    // and on builds without the server feature.
    #[cfg(all(not(target_arch = "wasm32"), feature = "server"))]
    {
        use colmap_openmvs_backend::initialize_from_env;
        initialize_from_env();
    }

    init_backend_url();

    info!("Launching Dioxus application...");

    // Use hash-based routing on static web builds (no server to rewrite routes).
    // With fullstack, hydrate via default WebHistory so SSR works correctly.
    dioxus::LaunchBuilder::new()
        .with_cfg(web! {
            let mut cfg = dioxus::web::Config::new();
            #[cfg(not(feature = "fullstack"))]
            {
                cfg = cfg.history(std::rc::Rc::new(dioxus::web::HashHistory::new(false)));
            }
            cfg
        })
        .with_cfg(desktop! {
           dioxus::desktop::Config::new().with_window(
               dioxus::desktop::WindowBuilder::new()
                   .with_title("Photos to 3D Model Offline")
           )
        })
        .launch(App);
}
