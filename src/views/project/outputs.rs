use dioxus::prelude::*;

#[component]
pub fn OutputsTab(project_name: String) -> Element {
    rsx! {
        div {
            class: "tab-content outputs-tab",
            div {
                class: "placeholder-content",
                h3 { "Outputs" }
                p { "Project: {project_name}" }
                p { "Processing outputs and results will be displayed here." }
            }
        }
    }
}
