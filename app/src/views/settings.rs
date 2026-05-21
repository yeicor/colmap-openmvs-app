use crate::components::{
    button::{Button, ButtonVariant},
    tabs::{TabContent, TabList, TabTrigger, Tabs},
};
use crate::mycomponents::BackButton;
use crate::mycomponents::{Banner, BannerType, PageHeader};
use crate::server::{
    download_runtime_version, get_available_runtime_versions, get_runtime_info, get_settings,
    get_task_info, list_available_image_tags, list_runtime_images, list_tasks,
    prepare_runtime_image, remove_runtime_image, subscribe_task_events, update_settings,
};
use crate::Route;
use chrono::{DateTime, Duration, Utc};
use colmap_openmvs_api::{
    ImageTagInfo, PrepareProgress, PreparedImageInfo, RuntimeInfo, Settings, TaskEvent, TaskState,
};
use dioxus::document::eval;
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{
    BsBoxSeam, BsDownload, BsFolder, BsGear, BsTerminal, BsTrash3,
};
use dioxus_free_icons::Icon;

// ---------------------------------------------------------------------------
// Date helpers
// ---------------------------------------------------------------------------

/// Parse an ISO-8601 / RFC-3339 date string and return (relative_text, tooltip_text).
/// Falls back gracefully if the string cannot be parsed.
fn format_relative_date(date_str: &str) -> (String, String) {
    let parsed = DateTime::parse_from_rfc3339(date_str)
        .or_else(|_| {
            // Try with explicit format (Docker Hub sometimes omits the T separator)
            DateTime::parse_from_str(date_str, "%Y-%m-%dT%H:%M:%S%.fZ")
        })
        .map(|dt| dt.with_timezone(&Utc));

    match parsed {
        Ok(dt) => {
            let now = Utc::now();
            let diff = now.signed_duration_since(dt);

            let relative = if diff < Duration::minutes(1) {
                "just now".to_string()
            } else if diff < Duration::hours(1) {
                let m = diff.num_minutes();
                format!("{} minute{} ago", m, if m == 1 { "" } else { "s" })
            } else if diff < Duration::days(1) {
                let h = diff.num_hours();
                format!("{} hour{} ago", h, if h == 1 { "" } else { "s" })
            } else if diff < Duration::days(7) {
                let d = diff.num_days();
                format!("{} day{} ago", d, if d == 1 { "" } else { "s" })
            } else if diff < Duration::days(30) {
                let w = diff.num_weeks();
                format!("{} week{} ago", w, if w == 1 { "" } else { "s" })
            } else if diff < Duration::days(365) {
                let mo = diff.num_days() / 30;
                format!("{} month{} ago", mo, if mo == 1 { "" } else { "s" })
            } else {
                let y = diff.num_days() / 365;
                format!("{} year{} ago", y, if y == 1 { "" } else { "s" })
            };

            // Tooltip: "Jan 15, 2024 at 10:30 UTC"
            let tooltip = dt.format("%b %-e, %Y at %H:%M UTC").to_string();
            (relative, tooltip)
        }
        Err(_) => ("unknown date".to_string(), date_str.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Small date badge component
// ---------------------------------------------------------------------------

/// Renders "📅 3 months ago" with an exact-date tooltip (using the HTML title attribute).
#[component]
fn DateBadge(date: String) -> Element {
    let (rel, tooltip) = format_relative_date(&date);
    rsx! {
        span {
            class: "tag-date-relative",
            title: "{tooltip}",
            "📅 {rel}"
        }
    }
}

// ---------------------------------------------------------------------------
// localStorage helpers (gracefully no-ops on server / SSR)
// ---------------------------------------------------------------------------

/// Store a task ID in localStorage.  Key is sanitised to avoid JS injection.
fn ls_set_task_id(context_key: &str, task_id: &str) {
    let safe_key = context_key.replace(['\'', '\\'], "_");
    let safe_val = task_id.replace(['\'', '\\'], "_");
    let _ = eval(&format!(
        "try {{ localStorage.setItem('colmap_task_{safe_key}', '{safe_val}'); }} catch(e) {{}}"
    ));
}

/// Read a task ID from localStorage (async – must be awaited inside spawn).
async fn ls_get_task_id(context_key: &str) -> Option<String> {
    let safe_key = context_key.replace(['\'', '\\'], "_");
    let js = eval(&format!(
        "return (localStorage.getItem('colmap_task_{safe_key}') || '');"
    ));
    js.await
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .filter(|s| !s.is_empty())
}

/// Remove a task ID from localStorage.
fn ls_clear_task_id(context_key: &str) {
    let safe_key = context_key.replace(['\'', '\\'], "_");
    let _ = eval(&format!(
        "try {{ localStorage.removeItem('colmap_task_{safe_key}'); }} catch(e) {{}}"
    ));
}

// ---------------------------------------------------------------------------
// Top-level view
// ---------------------------------------------------------------------------

#[component]
pub fn SettingsView() -> Element {
    let mut active_tab = use_signal(|| Some("general".to_string()));

    rsx! {
        document::Link { rel: "stylesheet", href: asset!("/assets/views/settings.css") }

        div {
            id: "settings",

            PageHeader {
                title: "Settings".to_string(),
                icon: Some(rsx! { Icon { icon: BsGear } }),
                BackButton {
                    onclick: move |_| { dioxus::prelude::navigator().push(Route::Projects {}); }
                }
            }

            Tabs {
                value: active_tab,
                default_value: "general".to_string(),
                on_value_change: move |tab| active_tab.set(Some(tab)),

                TabList {
                    TabTrigger {
                        value: "general".to_string(),
                        index: 0usize,
                        Icon { icon: BsGear }
                        span { class: "tab-label", " General" }
                    }
                    TabTrigger {
                        value: "runtime".to_string(),
                        index: 1usize,
                        Icon { icon: BsTerminal }
                        span { class: "tab-label", " Runtime" }
                    }
                    TabTrigger {
                        value: "images".to_string(),
                        index: 2usize,
                        Icon { icon: BsBoxSeam }
                        span { class: "tab-label", " Images" }
                    }
                }

                if active_tab() == Some("general".to_string()) {
                    TabContent {
                        value: "general".to_string(),
                        index: 0usize,
                        GeneralTab {}
                    }
                }
                if active_tab() == Some("runtime".to_string()) {
                    TabContent {
                        value: "runtime".to_string(),
                        index: 1usize,
                        RuntimeTab {}
                    }
                }
                if active_tab() == Some("images".to_string()) {
                    TabContent {
                        value: "images".to_string(),
                        index: 2usize,
                        RuntimeImagesTab {}
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// General tab  (existing settings – projects folder)
// ---------------------------------------------------------------------------

#[component]
fn GeneralTab() -> Element {
    let mut projects_folder = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(String::new);
    let mut success = use_signal(String::new);
    let mut has_changed = use_signal(|| false);

    use_effect(move || {
        spawn(async move {
            loading.set(true);
            error.set(String::new());
            match get_settings().await {
                Ok(s) => {
                    projects_folder.set(s.projects_folder);
                }
                Err(e) => error.set(format!("Failed to load settings: {}", e)),
            }
            loading.set(false);
        });
    });

    let handle_save = move |_| {
        spawn(async move {
            error.set(String::new());
            success.set(String::new());
            let folder = projects_folder().trim().to_string();
            if folder.is_empty() {
                error.set("Projects folder path cannot be empty".to_string());
                return;
            }
            match update_settings(Settings {
                projects_folder: folder,
                default_image_tag: None,
            })
            .await
            {
                Ok(_) => {
                    success.set("Settings saved successfully!".to_string());
                    has_changed.set(false);
                }
                Err(e) => error.set(format!("Failed to save settings: {}", e)),
            }
        });
    };

    let handle_cancel = move |_| {
        spawn(async move {
            match get_settings().await {
                Ok(s) => {
                    projects_folder.set(s.projects_folder);
                    has_changed.set(false);
                    error.set(String::new());
                }
                Err(e) => error.set(format!("Failed to reload settings: {}", e)),
            }
        });
    };

    rsx! {
        Banner {
            message: error(),
            banner_type: BannerType::Error,
            on_close: move |_| error.set(String::new()),
        }
        Banner {
            message: success(),
            banner_type: BannerType::Info,
            on_close: move |_| success.set(String::new()),
        }

        if loading() {
            p { class: "loading", "Loading settings…" }
        } else {
            div {
                class: "settings-form",
                div {
                    class: "form-group",
                    label { "Projects Folder" }
                    div {
                        class: "folder-row",
                        input {
                            r#type: "text",
                            value: "{projects_folder}",
                            placeholder: "./projects",
                            class: "folder-input",
                            oninput: move |evt| {
                                projects_folder.set(evt.value());
                                has_changed.set(true);
                                error.set(String::new());
                                success.set(String::new());
                            },
                        }
                        input {
                            r#type: "file",
                            directory: true,
                            style: "display: none;",
                            id: "projects-folder-input",
                            onchange: move |evt| {
                                if let Some(file) = evt.files().into_iter().next() {
                                    projects_folder.set(
                                        file.path().to_str().expect("Invalid path").to_string(),
                                    );
                                    has_changed.set(true);
                                    error.set(String::new());
                                    success.set(String::new());
                                }
                            }
                        }
                        Button {
                            variant: ButtonVariant::Secondary,
                            onclick: move |_| {
                                eval("document.querySelector('#projects-folder-input').click()");
                            },
                            Icon { icon: BsFolder }
                        }
                    }
                }



                if has_changed() {
                    div {
                        class: "form-actions",
                        Button {
                            variant: ButtonVariant::Primary,
                            onclick: handle_save,
                            "Save"
                        }
                        Button {
                            variant: ButtonVariant::Secondary,
                            onclick: handle_cancel,
                            "Cancel"
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime tab  (PRoot binary management)
// ---------------------------------------------------------------------------

#[component]
fn RuntimeTab() -> Element {
    let mut active_runtime_tab = use_signal(|| Some("proot".to_string()));

    rsx! {
        div {
            class: "runtime-sub-tabs",

            div {
                class: "runtime-tab-triggers",
                button {
                    class: if active_runtime_tab() == Some("proot".to_string()) { "runtime-trigger active" } else { "runtime-trigger" },
                    onclick: move |_| active_runtime_tab.set(Some("proot".to_string())),
                    "PRoot"
                }
                button {
                    class: "runtime-trigger disabled",
                    disabled: true,
                    title: "Docker support coming soon",
                    "Docker (Coming Soon)"
                }
            }

            if active_runtime_tab() == Some("proot".to_string()) {
                runtime_proot_tab {}
            }
        }
    }
}

#[component]
fn runtime_proot_tab() -> Element {
    let mut runtime_info = use_signal(|| None::<RuntimeInfo>);
    let mut available_versions = use_signal(Vec::<String>::new);
    let mut selected_version = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let mut downloading = use_signal(|| false);
    let mut error = use_signal(String::new);
    let mut success = use_signal(String::new);

    use_effect(move || {
        spawn(async move {
            loading.set(true);
            error.set(String::new());

            match get_runtime_info().await {
                Ok(info) => runtime_info.set(Some(info)),
                Err(e) => error.set(format!("Failed to load runtime info: {}", e)),
            }

            if let Ok(versions) = get_available_runtime_versions().await {
                if let Some(first) = versions.first() {
                    selected_version.set(first.clone());
                }
                available_versions.set(versions);
            }

            loading.set(false);
        });
    });

    let handle_install = move |_| {
        let version = selected_version();
        if version.is_empty() {
            return;
        }
        downloading.set(true);
        error.set(String::new());
        success.set(String::new());
        spawn(async move {
            match download_runtime_version(version).await {
                Ok(_) => {
                    success.set("PRoot installed/updated successfully!".to_string());
                    if let Ok(info) = get_runtime_info().await {
                        runtime_info.set(Some(info));
                    }
                }
                Err(e) => error.set(format!("Failed to install PRoot: {}", e)),
            }
            downloading.set(false);
        });
    };

    let handle_refresh_versions = move |_| {
        spawn(async move {
            match get_available_runtime_versions().await {
                Ok(versions) => {
                    if let Some(first) = versions.first() {
                        selected_version.set(first.clone());
                    }
                    available_versions.set(versions);
                }
                Err(e) => error.set(format!("Failed to fetch versions: {}", e)),
            }
        });
    };

    rsx! {
        Banner {
            message: error(),
            banner_type: BannerType::Error,
            on_close: move |_| error.set(String::new()),
        }
        Banner {
            message: success(),
            banner_type: BannerType::Info,
            on_close: move |_| success.set(String::new()),
        }

        if loading() {
            p { class: "loading", "Loading runtime info…" }
        } else {
            div {
                class: "settings-section",

                if let Some(info) = runtime_info() {
                    div {
                        class: "runtime-status",

                        div {
                            class: "status-row",
                            span { class: "status-label", "Platform" }
                            span {
                                class: if info.supported { "status-badge ok" } else { "status-badge error" },
                                if info.supported { "✓ Supported" } else { "✗ Not Supported" }
                            }
                        }

                        if let Some(reason) = &info.unsupported_reason {
                            p { class: "status-note error-note", "{reason}" }
                        }

                        div {
                            class: "status-row",
                            span { class: "status-label", "Binary" }
                            span {
                                class: if info.installed { "status-badge ok" } else { "status-badge warn" },
                                if info.installed { "✓ Installed" } else { "✗ Not Installed" }
                            }
                            if let Some(v) = &info.version {
                                code { class: "version-text", "v{v}" }
                            }
                        }
                    }

                    if info.supported {
                        div {
                            class: "install-form",

                            label { class: "install-label", "Version" }

                            if available_versions().is_empty() {
                                span { class: "versions-empty", "No versions available" }
                            } else {
                                select {
                                    class: "version-select",
                                    onchange: move |e| selected_version.set(e.value()),
                                    for v in available_versions() {
                                        option {
                                            value: "{v}",
                                            selected: v == selected_version(),
                                            "{v}"
                                        }
                                    }
                                }
                            }

                            Button {
                                variant: ButtonVariant::Ghost,
                                title: "Refresh version list",
                                onclick: handle_refresh_versions,
                                "↻"
                            }

                            Button {
                                variant: ButtonVariant::Primary,
                                onclick: handle_install,
                                if downloading() {
                                    "Installing…"
                                } else if info.installed {
                                    Icon { icon: BsDownload }
                                    " Update"
                                } else {
                                    Icon { icon: BsDownload }
                                    " Install"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Images tab  (container image management)
// ---------------------------------------------------------------------------

#[component]
fn RuntimeImagesTab() -> Element {
    let mut ready_tags = use_signal(Vec::<PreparedImageInfo>::new);
    let mut available_tags = use_signal(Vec::<ImageTagInfo>::new);
    let mut preparing = use_signal(|| false);
    let mut prepare_status = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let mut tags_loading = use_signal(|| true);
    let mut error = use_signal(String::new);
    let mut success = use_signal(String::new);
    let mut default_image_tag = use_signal(String::new);
    let mut projects_folder = use_signal(String::new);
    // Task being prepared – tag name only (for the "already preparing" guard)
    let mut preparing_tag = use_signal(String::new);

    const COLMAP_IMAGE: &str = "mirror.gcr.io/yeicor/colmap-openmvs";

    // ── Helper: subscribe to a prepare task and drive the UI state ──────────
    let drive_prepare_stream = move |tag: String, task_id: String| {
        spawn(async move {
            match subscribe_task_events(task_id.clone()).await {
                Ok(mut stream) => {
                    while let Some(Ok(event)) = stream.recv().await {
                        match event {
                            TaskEvent::PrepareProgress(PrepareProgress::Downloading {
                                downloaded_bytes,
                                total_bytes,
                            }) => {
                                let mb = downloaded_bytes as f64 / 1_048_576.0;
                                prepare_status.set(if let Some(total) = total_bytes {
                                    format!("{:.1}/{:.1} MB", mb, total as f64 / 1_048_576.0)
                                } else {
                                    format!("{:.1} MB", mb)
                                });
                            }
                            TaskEvent::PrepareProgress(PrepareProgress::ExtractingLayer {
                                layer,
                                progress,
                            }) => {
                                prepare_status.set(format!(
                                    "Layer {} {:.0}%",
                                    layer,
                                    progress * 100.0
                                ));
                            }
                            TaskEvent::PrepareProgress(PrepareProgress::Error { message }) => {
                                error.set(format!("Preparation failed: {}", message));
                                prepare_status.set(String::new());
                                preparing.set(false);
                                preparing_tag.set(String::new());
                                ls_clear_task_id("prepare");
                                return;
                            }
                            TaskEvent::Completed => {
                                success.set(format!("Tag '{}' prepared successfully!", tag));
                                prepare_status.set(String::new());
                                if let Ok(imgs) = list_runtime_images().await {
                                    ready_tags.set(imgs);
                                }
                                preparing.set(false);
                                preparing_tag.set(String::new());
                                ls_clear_task_id("prepare");
                                return;
                            }
                            TaskEvent::Failed(msg) => {
                                error.set(format!("Preparation failed: {}", msg));
                                prepare_status.set(String::new());
                                preparing.set(false);
                                preparing_tag.set(String::new());
                                ls_clear_task_id("prepare");
                                return;
                            }
                            _ => {}
                        }
                    }
                    // Stream ended without explicit Completed event – treat as success
                    success.set(format!("Tag '{}' prepared successfully!", tag));
                    prepare_status.set(String::new());
                    if let Ok(imgs) = list_runtime_images().await {
                        ready_tags.set(imgs);
                    }
                    preparing.set(false);
                    preparing_tag.set(String::new());
                    ls_clear_task_id("prepare");
                }
                Err(e) => {
                    error.set(format!("Failed to subscribe to preparation events: {}", e));
                    prepare_status.set(String::new());
                    preparing.set(false);
                    preparing_tag.set(String::new());
                    ls_clear_task_id("prepare");
                }
            }
        });
    };

    // ── On mount: load images + available tags, then reconnect any running task ──
    use_effect(move || {
        spawn(async move {
            loading.set(true);
            tags_loading.set(true);

            match list_runtime_images().await {
                Ok(imgs) => {
                    if imgs.len() == 1 && default_image_tag().is_empty() {
                        if let Ok(settings) = get_settings().await {
                            if settings.default_image_tag.is_none() {
                                let tag = imgs[0].tag.clone();
                                let folder = settings.projects_folder.clone();
                                projects_folder.set(folder.clone());
                                if let Err(e) = update_settings(Settings {
                                    projects_folder: folder,
                                    default_image_tag: Some(tag.clone()),
                                })
                                .await
                                {
                                    error.set(format!("Failed to auto-select default tag: {}", e));
                                } else {
                                    default_image_tag.set(tag);
                                }
                            } else {
                                default_image_tag
                                    .set(settings.default_image_tag.unwrap_or_default());
                                projects_folder.set(settings.projects_folder);
                            }
                        }
                    } else if let Ok(settings) = get_settings().await {
                        default_image_tag.set(settings.default_image_tag.unwrap_or_default());
                        projects_folder.set(settings.projects_folder);
                    }
                    ready_tags.set(imgs);
                }
                Err(e) => error.set(format!("Failed to load images: {}", e)),
            }

            match list_available_image_tags().await {
                Ok(tags) => available_tags.set(tags),
                Err(e) => error.set(format!("Failed to load tags: {}", e)),
            }

            loading.set(false);
            tags_loading.set(false);

            // ── Reconnect to any in-progress prepare task ──────────────────
            // 1. Try localStorage first (survives browser close/reopen)
            let stored_id = ls_get_task_id("prepare").await;
            // 2. Fall back to server-side task list (survives tab navigation)
            let reconnect_id = if let Some(id) = stored_id {
                Some(id)
            } else {
                list_tasks(Some("PrepareImage".to_string()), None)
                    .await
                    .ok()
                    .and_then(|tasks| {
                        tasks
                            .into_iter()
                            .find(|t| t.state == TaskState::Running)
                            .map(|t| t.id)
                    })
            };

            if let Some(task_id) = reconnect_id {
                if let Ok(Some(info)) = get_task_info(task_id.clone()).await {
                    if info.state == TaskState::Running {
                        // Extract the tag name from the context_key (full image ref "repo:tag")
                        let tag = info
                            .context_key
                            .split_once(':')
                            .map(|x| x.1)
                            .unwrap_or("unknown")
                            .to_string();
                        preparing.set(true);
                        preparing_tag.set(tag.clone());
                        prepare_status.set("Reconnecting…".to_string());
                        drive_prepare_stream(tag, task_id.clone());
                    }
                }
            }
        });
    });

    // ── Start a new prepare task ────────────────────────────────────────────
    let mut handle_prepare = move |tag: String| {
        if tag.is_empty() || preparing() {
            return;
        }
        preparing.set(true);
        preparing_tag.set(tag.clone());
        prepare_status.set("Starting…".to_string());
        error.set(String::new());
        success.set(String::new());

        let full_image = format!("{}:{}", COLMAP_IMAGE, tag);
        spawn(async move {
            match prepare_runtime_image(full_image.clone()).await {
                Ok(task_id) => {
                    ls_set_task_id("prepare", &task_id);
                    drive_prepare_stream(tag, task_id);
                }
                Err(e) => {
                    error.set(format!("Failed to start preparation: {}", e));
                    prepare_status.set(String::new());
                    preparing.set(false);
                    preparing_tag.set(String::new());
                }
            }
        });
    };

    let handle_remove = move |hash: String| {
        spawn(async move {
            match remove_runtime_image(hash.clone()).await {
                Ok(_) => {
                    success.set("Tag removed.".to_string());
                    if let Ok(imgs) = list_runtime_images().await {
                        ready_tags.set(imgs);
                    }
                }
                Err(e) => error.set(format!("Failed to remove tag: {}", e)),
            }
        });
    };

    let handle_set_default = move |tag: String| {
        let folder = projects_folder().clone();
        spawn(async move {
            error.set(String::new());
            success.set(String::new());
            match update_settings(Settings {
                projects_folder: folder,
                default_image_tag: Some(tag.clone()),
            })
            .await
            {
                Ok(_) => {
                    default_image_tag.set(tag.clone());
                    success.set(format!("Default tag set to '{}'", tag));
                }
                Err(e) => error.set(format!("Failed to set default tag: {}", e)),
            }
        });
    };

    let handle_unset_default = move |_| {
        let folder = projects_folder().clone();
        spawn(async move {
            error.set(String::new());
            success.set(String::new());
            match update_settings(Settings {
                projects_folder: folder,
                default_image_tag: None,
            })
            .await
            {
                Ok(_) => {
                    default_image_tag.set(String::new());
                    success.set("Default tag cleared".to_string());
                }
                Err(e) => error.set(format!("Failed to clear default tag: {}", e)),
            }
        });
    };

    rsx! {
        Banner {
            message: error(),
            banner_type: BannerType::Error,
            on_close: move |_| error.set(String::new()),
        }
        Banner {
            message: success(),
            banner_type: BannerType::Info,
            on_close: move |_| success.set(String::new()),
        }

        // ── Header with runtime info + in-progress indicator ───────────────
        div {
            class: "images-header",
            p { class: "images-info", "For: PRoot runtime" }
            if !prepare_status().is_empty() {
                p { class: "prepare-progress", "⟳ Preparing '{preparing_tag}': {prepare_status}" }
            }
        }

        // ── Ready Tags list ─────────────────────────────────────────────────
        div {
            class: "tags-container",
            h2 { class: "section-title", "Ready Tags" }

            if loading() {
                p { class: "loading", "Loading…" }
            } else if ready_tags().is_empty() {
                p { class: "empty", "No tags prepared yet." }
            } else {
                ul {
                    class: "tags-list",
                    {ready_tags().into_iter().map(|image| {
                        let tag = image.tag.clone();
                        let tag2 = image.tag.clone();
                        let hash = image.hash.clone();
                        let build_date = image.build_date.clone();
                        let size_readable = image.size_readable.clone();
                        let size = image.size;
                        let is_default = tag == default_image_tag();
                        rsx! {
                            li {
                                key: "{hash}",
                                class: "tags-item",

                                // ── Line 1: name + action buttons ──────────────
                                div {
                                    class: "tags-item-top",
                                    span {
                                        class: "tag-name",
                                        title: "{tag}",
                                        "{tag}"
                                    }
                                    div {
                                        class: "tag-actions",
                                        if is_default {
                                            Button {
                                                variant: ButtonVariant::Primary,
                                                title: "Unset as default tag",
                                                onclick: handle_unset_default,
                                                "✓ Default"
                                            }
                                        } else {
                                            Button {
                                                variant: ButtonVariant::Secondary,
                                                title: "Set as default tag",
                                                onclick: move |_| handle_set_default(tag2.clone()),
                                                "Set Default"
                                            }
                                        }
                                        Button {
                                            variant: ButtonVariant::Destructive,
                                            title: "Remove this prepared tag",
                                            onclick: move |_| handle_remove(hash.clone()),
                                            Icon { icon: BsTrash3 }
                                        }
                                    }
                                }

                                // ── Line 2: metadata (date + size) ─────────────
                                div {
                                    class: "tags-item-meta",
                                    if let Some(date) = build_date {
                                        DateBadge { date }
                                    }
                                    span {
                                        class: "tag-meta-size",
                                        title: "{size} bytes",
                                        "💾 {size_readable}"
                                    }
                                }
                            }
                        }
                    })}
                }
            }
        }

        // ── Available Tags list ──────────────────────────────────────────────
        div {
            class: "tags-container",
            h2 { class: "section-title", "Available Tags" }

            if tags_loading() {
                p { class: "loading", "Loading…" }
            } else if available_tags().is_empty() {
                p { class: "empty", "Failed to load tags." }
            } else {
                ul {
                    class: "tags-list",
                    {available_tags().into_iter().map(|tag_info| {
                        let name = tag_info.name.clone();
                        let name2 = tag_info.name.clone();
                        let build_date = tag_info.build_date.clone();
                        rsx! {
                            li {
                                key: "{name}",
                                class: "tags-item",

                                // ── Line 1: name + download button ─────────────
                                div {
                                    class: "tags-item-top",
                                    span {
                                        class: "tag-name",
                                        title: "{name}",
                                        "{name}"
                                    }
                                    div {
                                        class: "tag-actions",
                                        Button {
                                            variant: ButtonVariant::Secondary,
                                            title: "Download and prepare this tag",
                                            onclick: move |_| handle_prepare(name2.clone()),
                                            disabled: preparing(),
                                            Icon { icon: BsDownload }
                                        }
                                    }
                                }

                                // ── Line 2: build date ─────────────────────────
                                if let Some(date) = build_date {
                                    div {
                                        class: "tags-item-meta",
                                        DateBadge { date }
                                    }
                                }
                            }
                        }
                    })}
                }
            }
        }
    }
}
