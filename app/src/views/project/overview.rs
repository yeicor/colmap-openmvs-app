use crate::Route;
use dioxus::prelude::*;

use super::layout::ProjectPage;

/// Catch-all route: `/project/:name` → redirects to `/project/:name/images`.
#[component]
pub fn ProjectOverview(name: String) -> Element {
    let mut did_redirect = use_signal(|| false);
    use_effect(move || {
        if !did_redirect() {
            did_redirect.set(true);
            navigator().replace(Route::ProjectImages { name: name.clone() });
        }
    });
    // Show the project page while the redirect is in-flight so the user
    // doesn't see a blank flash.
    rsx! { ProjectPage {} }
}
