use dioxus::prelude::*;

#[component]
pub fn LogsTab(project_name: String) -> Element {
    rsx! {
        div {
            class: "tab-content logs-tab",
            div {
                class: "placeholder-content",
                h3 { "Logs" }
                p { "Project: {project_name}" }
                p { "Processing logs and status information will be displayed here." }
            }
        }
    }
}
