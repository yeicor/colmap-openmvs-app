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

use logging::init as init_logging;
pub use views::{Project, Projects, ProjectsSidebar, SettingsView, StartupTasks};

use crate::server::startup;

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

fn main() {
    init_logging();

    // Resolve the backend URL from URL params / localStorage before launching.
    // On web (WASM) this also calls `dioxus::fullstack::set_server_url` so that
    // all generated server-function HTTP requests go to the configured origin.
    let backend_url_str = backend_url::read_initial_backend_url();
    backend_url::BACKEND_URL.set(backend_url_str.clone()).ok();
    if !backend_url_str.is_empty() {
        let leaked: &'static str = Box::leak(backend_url_str.clone().into_boxed_str());
        dioxus::fullstack::set_server_url(leaked);
    }
    info!(
        "Backend URL resolved to '{}'",
        backend_url::BACKEND_URL
            .get()
            .unwrap_or(&"<empty>".to_string())
    );

    info!("Starting colmap-openmvs-app client");
    dioxus::launch(App);
}
