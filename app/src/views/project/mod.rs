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
// Each renders the shared ProjectPage which provides the chrome + tabs.

/// Route component for `/project/:name/images`.
#[allow(unused_variables)]
#[component]
pub fn ProjectImages(name: String) -> Element {
    rsx! { ProjectPage {} }
}

/// Route component for `/project/:name/config`.
#[allow(unused_variables)]
#[component]
pub fn ProjectConfig(name: String) -> Element {
    rsx! { ProjectPage {} }
}

/// Route component for `/project/:name/logs`.
#[allow(unused_variables)]
#[component]
pub fn ProjectLogs(name: String) -> Element {
    rsx! { ProjectPage {} }
}

/// Route component for `/project/:name/outputs`.
#[allow(unused_variables)]
#[component]
pub fn ProjectOutputs(name: String) -> Element {
    rsx! { ProjectPage {} }
}

/// Shared pipeline progress context provided by the ProjectPage and
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
