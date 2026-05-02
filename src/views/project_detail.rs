use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{BsArrowLeft, BsBoxSeam, BsFileText, BsGear, BsImages};
use dioxus_free_icons::Icon;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Images,
    Config,
    Logs,
    Outputs,
}

#[component]
pub fn ProjectDetail(name: String) -> Element {
    let mut active_tab = use_signal(|| Tab::Images);

    let tab_button = |tab: Tab, label: &str, icon_fn: fn() -> Element| {
        let is_active = active_tab() == tab;
        rsx! {
            button {
                class: if is_active { "tab-btn tab-btn-active" } else { "tab-btn" },
                onclick: move |_| active_tab.set(tab),
                {icon_fn()}
                span { class: "tab-label", "{label}" }
            }
        }
    };

    let content = match active_tab() {
        Tab::Images => rsx! { div { class: "tab-content", "Images CRUD - Placeholder" } },
        Tab::Config => rsx! { div { class: "tab-content", "Config CRUD - Placeholder" } },
        Tab::Logs => rsx! { div { class: "tab-content", "Logs View - Placeholder" } },
        Tab::Outputs => rsx! { div { class: "tab-content", "Outputs View - Placeholder" } },
    };

    rsx! {
        document::Link { rel: "stylesheet", href: asset!("/assets/views/project_detail.css") }

        div {
            id: "project-detail",
            div {
                class: "header",
                Link {
                    to: Route::Projects {},
                    class: "btn-icon btn-primary",
                    title: "Back to projects",
                    Icon { icon: BsArrowLeft }
                }
                h1 { "{name}" }
            }

            div {
                class: "main-content",
                div {
                    class: "tabs",
                    {tab_button(Tab::Images, "Images", || rsx! { Icon { icon: BsImages } })}
                    {tab_button(Tab::Config, "Config", || rsx! { Icon { icon: BsGear } })}
                    {tab_button(Tab::Logs, "Logs", || rsx! { Icon { icon: BsFileText } })}
                    {tab_button(Tab::Outputs, "Outputs", || rsx! { Icon { icon: BsBoxSeam } })}
                }

                {content}
            }
        }
    }
}
