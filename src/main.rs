use dioxus::prelude::*;

mod views;
use views::{Project, Projects, Settings, Sidebar};

mod common;

mod server;

#[allow(warnings, clippy::all)]
mod components;

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[layout(Sidebar)]
        #[route("/")]
        Projects {},
        #[route("/settings")]
        Settings {},
        #[route("/project/:name")]
        Project { name: String },
}

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        document::Link { rel: "icon", type: "image/png", href: asset!("/assets/icon.png") }
        document::Link { rel: "stylesheet", href: asset!("/assets/main.css") }
        document::Link { rel: "stylesheet", href: asset!("/assets/dx-components-theme.css") }
        document::Link { rel: "stylesheet", href: asset!("/assets/dx-components-theme-override.css") }
        Router::<Route> {}
    }
}
