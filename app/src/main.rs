//! Client-side UI code for colmap-openmvs-app
//!
//! This package contains all client-side UI components, views, and the main application entry point.
//! It imports from the server package for types and function calls.

use dioxus::prelude::*;
use tracing::info;
pub mod backend_url;
pub mod components;
pub mod logging;
pub mod mycomponents;
pub mod server;
pub mod task_manager;
pub mod views;

#[cfg(feature = "demo")]
pub mod demo;

use logging::init as init_logging;
pub use views::{Project, Projects, ProjectsSidebar, SettingsView, StartupTasks};

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(ProjectsSidebar)]
        #[route("/")]
        Projects {},
        #[route("/settings")]
        SettingsView {},
        #[route("/project/:name")]
        Project { name: String },
    #[route("/startup")]
    StartupTasks {},
}

#[component]
pub fn App() -> Element {
    log_build_info();

    // Eruda debug console — only injected in debug builds.
    // The eruda.js file is copied from node_modules by build.rs and referenced
    // here so that dx includes it in the asset bundle.
    #[cfg(debug_assertions)]
    {
        let eruda_url = asset!("/assets/lib/eruda/eruda.js").to_string();
        let _ = dioxus::document::eval(&format!(
            r#"
            if (typeof eruda === 'undefined') {{
                const s = document.createElement('script');
                s.src = '{eruda_url}';
                s.onload = () => eruda.init();
                document.head.appendChild(s);
            }}
            "#
        ));
    }

    use crate::task_manager::{TasksCtx, TasksState};
    use_context_provider(|| Signal::new(TasksState::default()) as TasksCtx);

    use_future(move || async move {
        if let Err(e) = server::startup().await {
            tracing::error!(error = ?e, "Server startup failed");
        }
    });

    // Fetch the server-side color-scheme preference once on startup.
    // On Android the WebView may not propagate `prefers-color-scheme` CSS media
    // queries correctly, so the server returns an explicit override (`Some`).
    // On other platforms the server returns `None` and we leave the `data-theme`
    // attribute untouched so the CSS media query continues to work normally.
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

    rsx! {
        document::Link { rel: "icon", type: "image/png", href: asset!("/assets/icon.png") }
        document::Link { rel: "stylesheet", href: asset!("/assets/main.css") }
        document::Link { rel: "stylesheet", href: asset!("/assets/dx-components-theme.css") }
        document::Link { rel: "stylesheet", href: asset!("/assets/dx-components-theme-override.css") }
        document::Link { rel: "stylesheet", href: asset!("/assets/mycomponents.css") }
        document::Link { rel: "stylesheet", href: asset!("/assets/tasks-panel.css") }
        document::Title { "COLMAP + OpenMVS" }
        Router::<Route> {}
    }
}

fn init_backend() {
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
            "Backend URL resolved to '{}'",
            backend_url::BACKEND_URL
                .get()
                .unwrap_or(&"<empty>".to_string())
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
    init_backend();
    dioxus::launch(App);
}
