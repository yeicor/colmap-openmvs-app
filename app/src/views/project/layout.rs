use crate::components::{
    progress::{Progress, ProgressIndicator},
    tabs::{TabList, TabTrigger, Tabs},
};
use crate::mycomponents::{BackButton, PageHeader, PageHeaderButton};
use crate::Route;
use dioxus::prelude::*;
use tracing::{debug, info};

use dioxus_free_icons::icons::bs_icons::{BsBoxSeam, BsFileText, BsGear, BsImages};
use dioxus_free_icons::Icon;

use super::{PipelineCommandCtx, PipelineIsRunningCtx, PipelineProgressCtx};

/// Shared layout for all `/project/:name/*` routes.
///
/// Because this is a `#[layout]` component, it stays mounted when the user
/// navigates between project sub-routes (Images, Config, Logs, Outputs).
/// The page-header (with its CSS `fade-slide-in` animation) therefore stays
/// in the DOM and does NOT re-animate on every tab switch.
///
/// Each child route component only provides its own tab content via `Outlet`.
#[component]
pub fn ProjectPage() -> Element {
    info!("Initializing project page");

    // ── Extract route params ──────────────────────────────────────────
    let route = use_route::<Route>();
    let name = match &route {
        Route::ProjectOverview { name }
        | Route::ProjectImages { name }
        | Route::ProjectConfig { name }
        | Route::ProjectLogs { name }
        | Route::ProjectOutputs { name } => name.clone(),
        _ => {
            tracing::warn!("ProjectPage: unexpected route, using empty name");
            String::new()
        }
    };

    // ── Shared pipeline signal contexts (provided by AppShell) ─────────
    let pipeline_progress = use_context::<PipelineProgressCtx>();
    let pipeline_is_running = use_context::<PipelineIsRunningCtx>();
    let mut pipeline_command = use_context::<PipelineCommandCtx>();

    let progress_value: Option<f64> = pipeline_progress().map(|p| p as f64);

    // ── Active tab derived from the current route ─────────────────────
    let mut active_tab = use_signal(|| None);
    use_effect(move || {
        let route = use_route::<Route>();
        let tab = match &route {
            Route::ProjectImages { .. } | Route::ProjectOverview { .. } => "images",
            Route::ProjectConfig { .. } => "config",
            Route::ProjectLogs { .. } => "logs",
            Route::ProjectOutputs { .. } => "outputs",
            _ => "images",
        };
        active_tab.set(Some(tab.to_string()));
    });
    let initial_tab = match &route {
        Route::ProjectImages { .. } | Route::ProjectOverview { .. } => "images",
        Route::ProjectConfig { .. } => "config",
        Route::ProjectLogs { .. } => "logs",
        Route::ProjectOutputs { .. } => "outputs",
        _ => "images",
    };
    if active_tab.peek().is_none() {
        active_tab.set(Some(initial_tab.to_string()));
    }

    // ── Tab-change handler — navigates to the matching route ──────────
    let on_tab_change = {
        let name = name.clone();
        move |tab: String| {
            debug!(project_name = %name, new_tab = %tab, "Switching project tab via route");
            let r = match tab.as_str() {
                "images" => Route::ProjectImages { name: name.clone() },
                "config" => Route::ProjectConfig { name: name.clone() },
                "logs" => Route::ProjectLogs { name: name.clone() },
                "outputs" => Route::ProjectOutputs { name: name.clone() },
                _ => return,
            };
            navigator().push(r);
        }
    };

    // ── Run / Cancel button handler ───────────────────────────────────
    let on_run_clicked = {
        let name = name.clone();
        move |_| {
            if pipeline_is_running() {
                info!(project_name = %name, "User cancelled pipeline");
                pipeline_command.set(Some(false));
            } else {
                info!(project_name = %name, "Starting pipeline run");
                navigator().push(Route::ProjectLogs { name: name.clone() });
                pipeline_command.set(Some(true));
            }
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
                    onclick: move |_| { navigator().push(Route::Projects {}); }
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
                    on_value_change: on_tab_change,
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
                }
                Outlet::<Route> {}
            }
        }
    }
}
