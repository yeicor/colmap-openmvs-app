//! Client-side UI code for colmap-openmvs-app
//!
//! This package contains all client-side UI components, views, and the main application entry point.
//! It imports from the server package for types and function calls.

use dioxus::prelude::*;
use tracing::info;
pub mod components;
pub mod mycomponents;
pub mod server;
pub mod task_manager;
pub mod views;

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
    // Inject an ES module importmap so bare/relative specifiers inside
    // dynamically eval'd scripts resolve to the locally-vendored assets.
    //
    // Mappings:
    //   'three'                        → three.module.js (fingerprinted)
    //   '/utils/BufferGeometryUtils.js'→ BufferGeometryUtils.js (fingerprinted)
    //     └─ After dx flattens assets, GLTFLoader (at /assets/GLTFLoader-HASH.js)
    //       resolves its `from '../utils/BufferGeometryUtils.js'` import to
    //       /utils/BufferGeometryUtils.js.  The importmap catches that path.
    //
    // Uses eval so the importmap is in the DOM before any dynamic import() call,
    // even if the component renders after the WASM bootstrap script.
    // build.rs downloads all files; asset!() ensures they are copied into the
    // bundle and returns the (possibly fingerprinted) serving URL.
    {
        let three_url = asset!(
            "/assets/lib/three/three.module.js",
            AssetOptions::js().with_minify(false)
        )
        .to_string();
        // with_minify(false) → dx skips esbuild for these files. Without it
        // esbuild may reformat them into a non-module (IIFE/CJS) format, which
        // causes `import('three')` to fail with "Importing a module script
        // failed". Files with bare 'three' imports would also ERROR in esbuild.
        let buf_geo_url = asset!(
            "/assets/lib/utils/BufferGeometryUtils.js",
            AssetOptions::js().with_minify(false)
        )
        .to_string();
        let skeleton_url = asset!(
            "/assets/lib/utils/SkeletonUtils.js",
            AssetOptions::js().with_minify(false)
        )
        .to_string();
        let _ = dioxus::document::eval(&format!(
            r#"
            if (!document.querySelector('script[type="importmap"]')) {{
                const m = document.createElement('script');
                m.type = 'importmap';
                m.textContent = JSON.stringify({{
                    imports: {{
                        three: '{three_url}',
                        '/utils/BufferGeometryUtils.js': '{buf_geo_url}',
                        '/utils/SkeletonUtils.js': '{skeleton_url}'
                    }}
                }});
                document.head.insertBefore(m, document.head.firstChild);
            }}
            "#
        ));
    }

    // Eruda debug console — only injected in debug builds.
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
        Router::<Route> {}
    }
}

fn main() {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info,colmap_openmvs_backend=trace");
    }
    info!("Starting colmap-openmvs-app client");
    dioxus::launch(App);
}
