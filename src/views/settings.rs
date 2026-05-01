use dioxus::prelude::*;

use crate::Route;

#[component]
pub fn Settings() -> Element {
    rsx! {
        div {
            id: "settings",
            h1 { "Settings" }

            Link {
                to: Route::Projects {},
                "Go to Projects"
            }
        }
    }
}
