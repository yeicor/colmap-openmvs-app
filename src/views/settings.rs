use dioxus::prelude::*;

use crate::{server::get_settings, Route};

#[component]
pub fn Settings() -> Element {
    let settings = use_resource(|| get_settings());
    rsx! {
        div {
            id: "settings",
            h1 { "Settings" }

            Link {
                to: Route::Projects {},
                "Go to Projects"
            }

            match &*settings.read_unchecked() {
                Some(Ok(settings)) => rsx! {
                    p { "Projects Folder: {settings.projects_folder}" }
                },
                Some(Err(err)) => rsx! {
                    p { "Error loading settings: {err}" }
                },
                None => rsx! {
                    p { "Loading settings..." }
                }
            }
        }
    }
}
