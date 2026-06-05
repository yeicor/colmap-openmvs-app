use crate::components::{
    progress::{Progress, ProgressIndicator},
    tabs::{TabContent, TabList, TabTrigger, Tabs},
};
use crate::mycomponents::{BackButton, PageHeader, PageHeaderButton};
use crate::Route;
use dioxus::prelude::*;
use tracing::{debug, info};

use dioxus_free_icons::icons::bs_icons::{BsBoxSeam, BsFileText, BsGear, BsImages};
use dioxus_free_icons::Icon;

mod images;
use images::ImagesTab;

mod config;
use config::ConfigTab;

mod logs;
use logs::LogsTab;

mod outputs;
use outputs::OutputsTab;

/// Shared pipeline progress context provided by the Project component and
/// consumed by both the PageHeader progress bar and the LogsTab.
///
/// Value is 0.0..=1.0; `None` means no pipeline is active.
pub type PipelineProgressCtx = Signal<Option<f32>>;

/// Whether a pipeline (full run or dry-run) is currently running.
/// Written by LogsTab, read by the PageHeader Run/Cancel button.
pub type PipelineIsRunningCtx = Signal<bool>;

/// Command channel from the PageHeader button to LogsTab.
/// `Some(true)` = start a full pipeline run, `Some(false)` = cancel, `None` = idle.
pub type PipelineCommandCtx = Signal<Option<bool>>;

#[component]
pub fn Project(name: String) -> Element {
    info!(project_name = %name, "Initializing project view");
    let mut active_tab = use_signal(|| Some("images".to_string()));

    // Provide a shared pipeline-progress signal accessible to LogsTab (write)
    // and to the PageHeader progress bar (read).
    let pipeline_progress: PipelineProgressCtx = use_signal(|| None);
    use_context_provider(|| pipeline_progress);

    // Whether any pipeline task is currently running (written by LogsTab).
    let pipeline_is_running: PipelineIsRunningCtx = use_signal(|| false);
    use_context_provider(|| pipeline_is_running);

    // Command signal: PageHeader writes, LogsTab reads and acts.
    let mut pipeline_command: PipelineCommandCtx = use_signal(|| None);
    use_context_provider(|| pipeline_command);

    let progress_value: Option<f64> = pipeline_progress().map(|p| p as f64);

    let on_run_clicked_name = name.clone();
    let on_value_changed_name = name.clone();
    let on_run_clicked = move |_| {
        if pipeline_is_running() {
            info!(project_name = %on_run_clicked_name, "User cancelled pipeline");
            // Cancel the running pipeline.
            pipeline_command.set(Some(false));
        } else {
            info!(project_name = %on_run_clicked_name, "Starting pipeline run");
            // Navigate to the Logs tab so the user sees what is happening,
            // then issue the start command.
            active_tab.set(Some("logs".to_string()));
            debug!("Tab switched to logs before starting pipeline");
            pipeline_command.set(Some(true));
        }
    };

    rsx! {
        div {
            id: "project",
            PageHeader {
                title: name.clone(),
                PageHeaderButton {
                    icon: rsx! {
                        if pipeline_is_running() { "⏹" } else { "▶️" }
                    },
                    extra: rsx! {
                        if pipeline_is_running() { "Cancel" } else { "Run" }
                    },
                    onclick: on_run_clicked,
                }
                BackButton {
                    onclick: move |_| { dioxus::prelude::navigator().push(Route::Projects {}); }
                }
                Progress {
                    value: progress_value.unwrap_or(0.0),
                    max: 1.0,
                    ProgressIndicator {}
                }
            }

            div {
                class: "main-content",
                Tabs {
                    value: active_tab,
                    default_value: "images".to_string(),
                    on_value_change: move |tab| {
                        debug!(project_name = %on_value_changed_name, new_tab = %tab, "Switching project tab");
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
