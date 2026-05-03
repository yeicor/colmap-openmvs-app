use crate::components::tabs::{TabContent, TabList, TabTrigger, Tabs};
use crate::mycomponents::page_header::{BackButton, PageHeaderButton};
use crate::mycomponents::PageHeader;
use crate::Route;
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{BsBoxSeam, BsCamera2, BsFileText, BsGear, BsImages};
use dioxus_free_icons::Icon;
use dioxus_primitives::progress::{Progress, ProgressIndicator};

#[component]
pub fn Project(name: String) -> Element {
    let mut active_tab = use_signal(|| Some("images".to_string()));

    rsx! {
        document::Link { rel: "stylesheet", href: asset!("/assets/views/project.css") }

        div {
            id: "project",
            PageHeader {
                title: name.clone(),
                icon: Some(rsx! { Icon { icon: BsCamera2 } }),
                PageHeaderButton {
                    icon: rsx! { "▶️" },
                    extra: rsx! { "Run" },
                    extra_tooltip: Some(rsx! { "Start/stop the reconstruction pipeline for this project" }),
                    onclick: move |_| { dioxus::prelude::navigator().push(Route::Settings {}); }
                }
                BackButton {
                    onclick: move |_| { dioxus::prelude::navigator().push(Route::Projects {}); }
                }
                Progress {
                    ProgressIndicator {}
                }
            }

            div {
                class: "main-content",
                Tabs {
                    value: active_tab,
                    default_value: "images".to_string(),
                    on_value_change: move |tab| {
                        active_tab.set(Some(tab));
                    },
                    TabList {
                        TabTrigger {
                            value: "images".to_string(),
                            index: 0usize,
                            Icon { icon: BsImages }
                            span { class: "tab-label", "Images" }
                        }
                        TabTrigger {
                            value: "config".to_string(),
                            index: 1usize,
                            Icon { icon: BsGear }
                            span { class: "tab-label", "Config" }
                        }
                        TabTrigger {
                            value: "logs".to_string(),
                            index: 2usize,
                            Icon { icon: BsFileText }
                            span { class: "tab-label", "Logs" }
                        }
                        TabTrigger {
                            value: "outputs".to_string(),
                            index: 3usize,
                            Icon { icon: BsBoxSeam }
                            span { class: "tab-label", "Outputs" }
                        }
                    }
                    TabContent {
                        value: "images".to_string(),
                        index: 0usize,
                        div { class: "tab-content", "Images CRUD - Placeholder" }
                    }
                    TabContent {
                        value: "config".to_string(),
                        index: 1usize,
                        div { class: "tab-content", "Config CRUD - Placeholder" }
                    }
                    TabContent {
                        value: "logs".to_string(),
                        index: 2usize,
                        div { class: "tab-content", "Logs View - Placeholder" }
                    }
                    TabContent {
                        value: "outputs".to_string(),
                        index: 3usize,
                        div { class: "tab-content", "Outputs View - Placeholder" }
                    }
                }
            }
        }
    }
}
