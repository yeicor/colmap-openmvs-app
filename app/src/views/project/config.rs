use dioxus::prelude::*;

#[component]
pub fn ConfigTab(project_name: String) -> Element {
    rsx! {
        div {
            class: "tab-content config-tab",
            div {
                class: "placeholder-content",
                h3 { "Configuration" }
                p { "Project: {project_name}" }
                p { "Configuration settings for this project will be displayed here." }
            }
        }
    }
}
