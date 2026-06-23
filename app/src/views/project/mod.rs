use dioxus::prelude::*;

pub mod config;
pub mod images;
pub mod layout;
pub mod logs;
pub mod outputs;
pub use layout::ProjectPage;

pub mod overview;
pub use overview::ProjectOverview;

// ── Route components ──────────────────────────────────────────────────
// These are now rendered inside `#[layout(ProjectPage)]`, so each one
// returns only its own tab content — the shared chrome (header, tabs bar)
// comes from ProjectPage which stays mounted across sub-route changes.

/// Tab content for `/project/:name/images` (and overview redirects here).
#[allow(unused_variables)]
#[component]
pub fn ProjectGallery(name: String) -> Element {
    rsx! {
        div { class: "dx-tabs-content dx-tabs-content-themed",
            images::GalleryTab { project_name: name.clone() }
        }
    }
}

/// Tab content for `/project/:name/config`.
#[allow(unused_variables)]
#[component]
pub fn ProjectConfig(name: String) -> Element {
    rsx! {
        div { class: "dx-tabs-content dx-tabs-content-themed",
            config::ConfigTab { project_name: name.clone() }
        }
    }
}

/// Tab content for `/project/:name/logs`.
#[allow(unused_variables)]
#[component]
pub fn ProjectLogs(name: String) -> Element {
    rsx! {
        div { class: "dx-tabs-content dx-tabs-content-themed",
            logs::LogsTab { project_name: name.clone() }
        }
    }
}

/// Tab content for `/project/:name/outputs`.
#[allow(unused_variables)]
#[component]
pub fn ProjectOutputs(name: String) -> Element {
    rsx! {
        div { class: "dx-tabs-content dx-tabs-content-themed",
            outputs::OutputsTab { project_name: name.clone() }
        }
    }
}

/// Shared pipeline progress context provided by the ProjectPage and
/// consumed by both the PageHeader progress bar and the LogsTab.
///
/// Value is 0.0..=1.0; `None` means no pipeline is active.
pub type PipelineProgressCtx = Signal<Option<f32>>;

/// Whether a pipeline (full run or recover-logs) is currently running.
/// Written by LogsTab, read by the PageHeader Run/Cancel button.
pub type PipelineIsRunningCtx = Signal<bool>;

/// Command channel from the PageHeader button to LogsTab.
/// `Some(true)` = start a full pipeline run, `Some(false)` = cancel, `None` = idle.
pub type PipelineCommandCtx = Signal<Option<bool>>;
