use crate::Route;
use dioxus::prelude::*;

use super::images::ImagesTab;

/// Catch-all route: `/project/:name` → redirects to `/project/:name/images`.
///
/// While the redirect is in-flight it renders the ImagesTab directly so
/// the user doesn't see a blank flash.  The shared chrome (page header,
/// tab bar) is provided by the parent `#[layout(ProjectPage)]` which
/// stays mounted across this brief redirect.
#[component]
pub fn ProjectOverview(name: String) -> Element {
    let mut did_redirect = use_signal(|| false);
    let name_for_redirect = name.clone();
    let name_for_tab = name.clone();
    use_effect(move || {
        if !did_redirect() {
            did_redirect.set(true);
            navigator().replace(Route::ProjectImages {
                name: name_for_redirect.clone(),
            });
        }
    });
    rsx! {
        div { class: "dx-tabs-content dx-tabs-content-themed",
            ImagesTab { project_name: name_for_tab }
        }
    }
}
