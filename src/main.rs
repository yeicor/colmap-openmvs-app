use dioxus::prelude::*;

mod views;
use views::{Projects, Settings};

mod common;

mod server;

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[route("/")]
    Projects {},
    #[route("/settings")]
    Settings {},
    // #[route("/project/:project_id")]
    // Project { project_id: String },
}

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        document::Link { rel: "icon", type: "image/png", href: asset!("/assets/icon.png") }
        Router::<Route> {}
    }
}
