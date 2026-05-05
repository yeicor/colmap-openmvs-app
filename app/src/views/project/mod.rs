use crate::components::{
    progress::{Progress, ProgressIndicator},
    tabs::{TabContent, TabList, TabTrigger, Tabs},
};
use crate::mycomponents::{BackButton, PageHeader, PageHeaderButton};
use crate::Route;
use dioxus::prelude::*;

use dioxus_free_icons::icons::bs_icons::{BsBoxSeam, BsCamera2, BsFileText, BsGear, BsImages};
use dioxus_free_icons::Icon;

mod images;
use images::ImagesTab;

mod config;
use config::ConfigTab;

mod logs;
use logs::LogsTab;

mod outputs;
use outputs::OutputsTab;

#[component]
pub fn Project(name: String) -> Element {
    let mut active_tab = use_signal(|| Some("images".to_string()));

    rsx! {
        document::Link { rel: "stylesheet", href: asset!("/assets/views/project.css") }
        document::Link { rel: "stylesheet", href: asset!("/assets/views/project/images.css") }

        div {
            id: "project",
            PageHeader {
                title: name.clone(),
                icon: Some(rsx! { Icon { icon: BsCamera2 } }),
                PageHeaderButton {
                    icon: rsx! { "▶️" },
                    extra: rsx! { "Run" },
                    extra_tooltip: Some(rsx! { "Start/stop the reconstruction pipeline for this project" }),
                    onclick: move |_| { /* TODO */ }
                }
                BackButton {
                    onclick: move |_| { dioxus::prelude::navigator().push(Route::Projects {}); }
                }
                Progress {
                    value: 0.0,
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
                    if active_tab() == Some("images".to_string()) {
                        TabContent {
                            value: "images".to_string(),
                            index: 0usize,
                            ImagesTab { project_name: name.clone() }
                        }
                    }
                    if active_tab() == Some("config".to_string()) {
                        TabContent {
                            value: "config".to_string(),
                            index: 1usize,
                            ConfigTab { project_name: name.clone() }
                        }
                    }
                    if active_tab() == Some("logs".to_string()) {
                        TabContent {
                            value: "logs".to_string(),
                            index: 2usize,
                            LogsTab { project_name: name.clone() }
                        }
                    }
                    if active_tab() == Some("outputs".to_string()) {
                        TabContent {
                            value: "outputs".to_string(),
                            index: 3usize,
                            OutputsTab { project_name: name.clone() }
                        }
                    }
                }
            }
        }
    }
}
