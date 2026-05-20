//! Client-side UI code for colmap-openmvs-app
//!
//! This package contains all client-side UI components, views, and the main application entry point.
//! It imports from the server package for types and function calls.

use dioxus::prelude::*;

pub mod components;
pub mod mycomponents;
pub mod server;
pub mod views;

pub use views::{Project, Projects, ProjectsSidebar, SettingsView};

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
}

#[component]
pub fn App() -> Element {
    // Inject Eruda console for debugging
    let _ = dioxus::document::eval(
        r#"
        if (typeof eruda === 'undefined') {
            const script = document.createElement('script');
            script.src = 'https://cdn.jsdelivr.net/npm/eruda@3.0.1';
            script.onload = () => {
                eruda.init();
            };
            document.body.appendChild(script);
        }
        "#,
    );

    rsx! {
        document::Link { rel: "icon", type: "image/png", href: asset!("/assets/icon.png") }
        document::Link { rel: "stylesheet", href: asset!("/assets/main.css") }
        document::Link { rel: "stylesheet", href: asset!("/assets/dx-components-theme.css") }
        document::Link { rel: "stylesheet", href: asset!("/assets/dx-components-theme-override.css") }
        document::Link { rel: "stylesheet", href: asset!("/assets/mycomponents.css") }
        Router::<Route> {}
    }
}

fn main() {
    dioxus::launch(App);
}
