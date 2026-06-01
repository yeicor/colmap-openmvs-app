use crate::components::{
    button::{Button, ButtonVariant},
    tabs::{TabContent, TabList, TabTrigger, Tabs},
};
use crate::mycomponents::BackButton;
use crate::mycomponents::{Banner, BannerType, PageHeader};
use crate::server::{
    delete_runtime_binary, download_runtime_version, get_available_runtime_versions,
    get_docker_runtime_info, get_runtime_info, get_settings, get_task_info,
    list_available_image_tags, list_docker_images, list_runtime_images, list_tasks,
    pick_projects_folder, pick_settings_file, prepare_docker_image, prepare_runtime_image,
    remove_docker_image, remove_runtime_image, update_settings,
};
use crate::task_manager::{drive_task, start_task, TasksCtx};
use crate::{backend_url, Route};
use chrono::{DateTime, Duration, Utc};
use colmap_openmvs_api::{
    ImageTagInfo, PreparedImageInfo, RuntimeInfo, TaskEvent, TaskKind, TaskState,
};
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{
    BsBox, BsDownload, BsFolder, BsGear, BsHdd, BsTerminal, BsTrash3,
};
use dioxus_free_icons::Icon;

// ---------------------------------------------------------------------------
// Date helpers
// ---------------------------------------------------------------------------

fn format_relative_date(date_str: &str) -> (String, String) {
    let parsed = DateTime::parse_from_rfc3339(date_str)
        .or_else(|_| DateTime::parse_from_str(date_str, "%Y-%m-%dT%H:%M:%S%.fZ"))
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
            let tooltip = dt.format("%b %-e, %Y at %H:%M UTC").to_string();
            (relative, tooltip)
        }
        Err(_) => ("unknown date".to_string(), date_str.to_string()),
    }
}

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
// Top-level view  (2 tabs: General | Runtime)
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

            div {
                class: "main-content",
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
                }

                if active_tab() == Some("general".to_string()) {
                    TabContent { value: "general".to_string(), index: 0usize,
                        GeneralTab {}
                    }
                }
                if active_tab() == Some("runtime".to_string()) {
                    TabContent { value: "runtime".to_string(), index: 1usize,
                        RuntimeTab {}
                    }
                }
            }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// General tab
// ---------------------------------------------------------------------------

#[component]
fn GeneralTab() -> Element {
    let mut projects_folder = use_signal(String::new);
    let mut settings_file_path = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(String::new);
    let mut success = use_signal(String::new);
    let mut has_changed = use_signal(|| false);

    // Backend URL — configurable on all platforms.
    let current_backend_url = backend_url::BACKEND_URL.get().cloned().unwrap_or_default();
    let current_backend_url_for_memo = current_backend_url.clone();
    let current_backend_url_for_cancel = current_backend_url.clone();
    let mut pending_backend_url = use_signal(|| current_backend_url.clone());
    let backend_url_changed =
        use_memo(move || pending_backend_url() != current_backend_url_for_memo.clone());
    let mut confirm_backend_reload = use_signal(|| false);

    use_effect(move || {
        spawn(async move {
            loading.set(true);
            match get_settings().await {
                Ok(s) => {
                    projects_folder.set(s.projects_folder);
                    settings_file_path.set(s.settings_file_path.unwrap_or_default());
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
            let projects = projects_folder().trim().to_string();
            if projects.is_empty() {
                error.set("Projects folder path cannot be empty".to_string());
                return;
            }
            let settings_path = if settings_file_path().trim().is_empty() {
                None
            } else {
                Some(settings_file_path().trim().to_string())
            };
            match get_settings().await {
                Ok(mut settings) => {
                    settings.projects_folder = projects;
                    settings.settings_file_path = settings_path;
                    match update_settings(settings).await {
                        Ok(_) => {
                            success.set("Settings saved.".to_string());
                            has_changed.set(false);
                        }
                        Err(e) => error.set(format!("Failed to save: {}", e)),
                    }
                }
                Err(e) => error.set(format!("Failed to load settings: {}", e)),
            }
        });
    };

    let handle_cancel = move |_| {
        spawn(async move {
            if let Ok(s) = get_settings().await {
                projects_folder.set(s.projects_folder);
                settings_file_path.set(s.settings_file_path.unwrap_or_default());
                has_changed.set(false);
                error.set(String::new());
            }
        });
    };

    rsx! {
        Banner { message: error(), banner_type: BannerType::Error, on_close: move |_| error.set(String::new()) }
        Banner { message: success(), banner_type: BannerType::Info, on_close: move |_| success.set(String::new()) }

        if loading() {
            p { class: "loading", "Loading…" }
        } else {
            div { class: "settings-form",
                div { class: "form-group",
                    label { title: "Root directory where all project folders are stored.", "Projects Folder" }
                    div { class: "folder-row",
                        input {
                            r#type: "text",
                            value: "{projects_folder}",
                            placeholder: "./projects",
                            class: "folder-input",
                            oninput: move |e| { projects_folder.set(e.value()); has_changed.set(true); error.set(String::new()); success.set(String::new()); },
                        }
                        Button {
                            variant: ButtonVariant::Secondary,
                            title: "Browse for folder (server-side dialog)",
                            onclick: move |_| {
                                spawn(async move {
                                    match pick_projects_folder().await {
                                        Ok(path) if !path.is_empty() => {
                                            projects_folder.set(path);
                                            has_changed.set(true);
                                            error.set(String::new());
                                        }
                                        Ok(_) => {} // user cancelled
                                        Err(e) => error.set(format!("Folder picker: {}", e)),
                                    }
                                });
                            },
                            Icon { icon: BsFolder }
                        }
                    }
                }

                div { class: "form-group",
                    label { title: "Override the settings.json path. Leave empty to use projects_folder/settings.json. Can also be set via COLMAP_SETTINGS_PATH env var.", "Settings File (optional)" }
                    div { class: "folder-row",
                        input {
                            r#type: "text",
                            value: "{settings_file_path}",
                            placeholder: "Leave empty for default",
                            class: "folder-input",
                            oninput: move |e| { settings_file_path.set(e.value()); has_changed.set(true); error.set(String::new()); success.set(String::new()); },
                        }
                        Button {
                            variant: ButtonVariant::Secondary,
                            title: "Browse for settings file (server-side dialog)",
                            onclick: move |_| {
                                spawn(async move {
                                    match pick_settings_file().await {
                                        Ok(path) if !path.is_empty() => {
                                            settings_file_path.set(path);
                                            has_changed.set(true);
                                            error.set(String::new());
                                        }
                                        Ok(_) => {} // user cancelled
                                        Err(e) => error.set(format!("File picker: {}", e)),
                                    }
                                });
                            },
                            Icon { icon: BsFolder }
                        }
                    }
                }

                if has_changed() {
                    div { class: "form-actions",
                        Button { variant: ButtonVariant::Primary, onclick: handle_save, "Save" }
                        Button { variant: ButtonVariant::Secondary, onclick: handle_cancel, "Cancel" }
                    }
                }

                // Backend URL — only meaningful on web/WASM deployments where the
                // API server lives at a different origin from the static UI.
                if backend_url::backend_url_configurable() {
                    hr { class: "settings-divider" }
                    div { class: "form-group",
                        label {
                            title: "Base URL of the API backend (e.g. https://api.example.com). \
                                    Leave empty to use the same origin as this page. \
                                    Can also be set via ?backend= URL parameter.",
                            "Backend API URL"
                        }
                        div { class: "folder-row",
                            input {
                                r#type: "url",
                                value: "{pending_backend_url}",
                                placeholder: "https://your-backend.example.com",
                                class: "folder-input",
                                oninput: move |e| {
                                    pending_backend_url.set(e.value());
                                    confirm_backend_reload.set(false);
                                },
                            }
                            if !pending_backend_url().is_empty() {
                                Button {
                                    variant: ButtonVariant::Secondary,
                                    title: "Clear and use same-origin",
                                    onclick: move |_| pending_backend_url.set(String::new()),
                                    "×"
                                }
                            }
                        }
                        if backend_url_changed() {
                            if confirm_backend_reload() {
                                div { class: "form-actions",
                                    p { class: "settings-warning",
                                            "⚠️ {backend_url::needs_restart_message()} Unsaved work will be lost."
                                        }
                                    Button {
                                                        variant: ButtonVariant::Primary,
                                                        onclick: move |_| {
                                                            let url = pending_backend_url();
                                                            backend_url::save_backend_url(&url);
                                                            backend_url::reload_or_exit();
                                                        },
                                                        "Confirm & Restart"
                                                    }
                                    Button {
                                        variant: ButtonVariant::Secondary,
                                        onclick: move |_| confirm_backend_reload.set(false),
                                        "Cancel"
                                    }
                                }
                            } else {
                                div { class: "form-actions",
                                    Button {
                                                        variant: ButtonVariant::Primary,
                                                        onclick: move |_| confirm_backend_reload.set(true),
                                                        "Save & Restart"
                                                    }
                                    Button {
                                        variant: ButtonVariant::Secondary,
                                        onclick: move |_| {
                                            pending_backend_url.set(current_backend_url_for_cancel.clone());
                                            confirm_backend_reload.set(false);
                                        },
                                        "Cancel"
                                    }
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
// Runtime tab  (runtime selector + selected runtime's panel)
// ---------------------------------------------------------------------------

#[component]
fn RuntimeTab() -> Element {
    // Preload both runtime statuses so the selector cards show live status
    let proot_info = use_signal(|| None::<RuntimeInfo>);
    let docker_info = use_signal(|| None::<RuntimeInfo>);
    let mut active_runtime = use_signal(|| "proot".to_string());
    let proot_has_default = use_signal(|| false);
    let docker_has_default = use_signal(|| false);

    // Load both statuses and default image info concurrently on mount
    use_effect(move || {
        let mut pi = proot_info;
        let mut di = docker_info;
        let mut ar = active_runtime;
        let mut phd = proot_has_default;
        let mut dhd = docker_has_default;
        spawn(async move {
            // Load runtime info
            if let Ok(info) = get_runtime_info().await {
                pi.set(Some(info));
            }
            if let Ok(info) = get_docker_runtime_info().await {
                di.set(Some(info));
            }

            // Load settings to get the selected runtime and default image info
            if let Ok(settings) = get_settings().await {
                let (runtime, _tag) = settings.parse_default_image();

                // Set which runtimes have defaults
                if let Some(rt) = runtime {
                    if rt == "proot" {
                        phd.set(true);
                    } else if rt == "docker" {
                        dhd.set(true);
                    }
                    // Set active runtime to the one with the default
                    ar.set(rt.to_string());
                }
            }
        });
    });

    // Helper: availability label + CSS class from RuntimeInfo
    let status_class = |info: Option<RuntimeInfo>| -> (&'static str, &'static str) {
        match info {
            Some(i) if i.supported && i.installed => ("✓ Ready", "rt-badge rt-badge-ok"),
            Some(i) if i.supported => ("⚠ Not installed", "rt-badge rt-badge-warn"),
            Some(_) => ("✗ Unavailable", "rt-badge rt-badge-err"),
            None => ("…", "rt-badge rt-badge-dim"),
        }
    };

    let (proot_label, proot_cls) = status_class(proot_info());
    let (docker_label, docker_cls) = status_class(docker_info());

    rsx! {
        div { class: "runtime-tab",

            // ── Runtime selector ─────────────────────────────────────────────
            div {
                class: "rt-selector",
                role: "radiogroup",
                aria_label: "Choose a container runtime",

                // PRoot option
                button {
                    class: if active_runtime() == "proot" { "rt-option rt-option-active" } else { "rt-option" },
                    role: "radio",
                    aria_checked: (active_runtime() == "proot").to_string(),
                    title: "PRoot — userspace container runner, works without root or Docker",
                    onclick: move |_| active_runtime.set("proot".to_string()),

                    div { class: "rt-option-icon", Icon { icon: BsHdd } }
                    div { class: "rt-option-body",
                        div { class: "rt-option-name-row",
                            span { class: "rt-option-name", "PRoot" }
                            if proot_has_default() {
                                span { class: "rt-option-default-mark", "✓ Default" }
                            }
                        }
                        span { class: "{proot_cls}", "{proot_label}" }
                    }
                    // Active indicator dot
                    div { class: "rt-option-dot" }
                }

                // Docker option
                button {
                    class: if active_runtime() == "docker" { "rt-option rt-option-active" } else { "rt-option" },
                    role: "radio",
                    aria_checked: (active_runtime() == "docker").to_string(),
                    title: "Docker — system Docker daemon; requires Docker to be installed",
                    onclick: move |_| active_runtime.set("docker".to_string()),

                    div { class: "rt-option-icon", Icon { icon: BsBox } }
                    div { class: "rt-option-body",
                        div { class: "rt-option-name-row",
                            span { class: "rt-option-name", "Docker" }
                            if docker_has_default() {
                                span { class: "rt-option-default-mark", "✓ Default" }
                            }
                        }
                        span { class: "{docker_cls}", "{docker_label}" }
                    }
                    div { class: "rt-option-dot" }
                }
            }

            // ── Selected runtime panel ────────────────────────────────────────
            if active_runtime() == "proot" {
                ProotPanel {
                    on_default_changed: move |_| {
                        let mut phd = proot_has_default;
                        let mut dhd = docker_has_default;
                        spawn(async move {
                            if let Ok(settings) = get_settings().await {
                                let (runtime, _tag) = settings.parse_default_image();
                                phd.set(runtime == Some("proot"));
                                dhd.set(runtime == Some("docker"));
                            }
                        });
                    }
                }
            } else {
                DockerPanel {
                    on_default_changed: move |_| {
                        let mut phd = proot_has_default;
                        let mut dhd = docker_has_default;
                        spawn(async move {
                            if let Ok(settings) = get_settings().await {
                                let (runtime, _tag) = settings.parse_default_image();
                                phd.set(runtime == Some("proot"));
                                dhd.set(runtime == Some("docker"));
                            }
                        });
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PRoot tab  (Status + Storage + Images, single page)
// ---------------------------------------------------------------------------

#[component]
fn ProotPanel(on_default_changed: EventHandler<()>) -> Element {
    let mut runtime_info = use_signal(|| None::<RuntimeInfo>);
    let mut available_versions = use_signal(Vec::<String>::new);
    let mut selected_version = use_signal(String::new);
    let mut proot_images_dir = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let mut downloading = use_signal(|| false);
    let mut deleting = use_signal(|| false);
    let mut error = use_signal(String::new);
    let mut success = use_signal(String::new);
    let mut dir_changed = use_signal(|| false);

    use_effect(move || {
        spawn(async move {
            loading.set(true);
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
            if let Ok(s) = get_settings().await {
                proot_images_dir.set(s.proot_images_dir);
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
        spawn(async move {
            match download_runtime_version(version).await {
                Ok(_) => {
                    success.set("PRoot installed/updated.".to_string());
                    if let Ok(info) = get_runtime_info().await {
                        runtime_info.set(Some(info));
                    }
                }
                Err(e) => error.set(format!("Failed: {}", e)),
            }
            downloading.set(false);
        });
    };

    let handle_delete = move |_| {
        deleting.set(true);
        error.set(String::new());
        spawn(async move {
            match delete_runtime_binary().await {
                Ok(_) => {
                    success.set("PRoot binary deleted.".to_string());
                    if let Ok(info) = get_runtime_info().await {
                        runtime_info.set(Some(info));
                    }
                }
                Err(e) => error.set(format!("Failed: {}", e)),
            }
            deleting.set(false);
        });
    };

    let handle_save_dir = move |_| {
        spawn(async move {
            let folder = proot_images_dir().trim().to_string();
            if folder.is_empty() {
                error.set("Path cannot be empty.".to_string());
                return;
            }
            match get_settings().await {
                Ok(mut s) => {
                    s.proot_images_dir = folder;
                    match update_settings(s).await {
                        Ok(_) => {
                            success.set("Images directory saved.".to_string());
                            dir_changed.set(false);
                        }
                        Err(e) => error.set(format!("Failed: {}", e)),
                    }
                }
                Err(e) => error.set(format!("Failed to load settings: {}", e)),
            }
        });
    };

    rsx! {
        Banner { message: error(), banner_type: BannerType::Error, on_close: move |_| error.set(String::new()) }
        Banner { message: success(), banner_type: BannerType::Info, on_close: move |_| success.set(String::new()) }

        if loading() {
            p { class: "loading", "Loading…" }
        } else {
            div { class: "runtime-tab",

                // ── Status & Binary card ─────────────────────────────────────
                div { class: "runtime-card",
                    div { class: "runtime-card-title", "Binary" }

                    if let Some(info) = runtime_info() {
                        div { class: "status-row",
                            span {
                                class: if info.supported { "status-badge ok" } else { "status-badge error" },
                                if info.supported { "✓ Supported" } else { "✗ Unsupported" }
                            }
                            span {
                                class: if info.installed { "status-badge ok" } else { "status-badge warn" },
                                if info.installed { "✓ Installed" } else { "✗ Not installed" }
                            }
                            if let Some(v) = &info.version {
                                code { class: "version-text", "v{v}" }
                            }
                        }
                        if let Some(reason) = &info.unsupported_reason {
                            p { class: "status-note error-note", "{reason}" }
                        }

                        if info.supported {
                            div { class: "status-actions",
                                if available_versions().is_empty() {
                                    span { class: "versions-empty", "No versions found" }
                                } else {
                                    select {
                                        class: "version-select",
                                        title: "Select a PRoot version to install",
                                        onchange: move |e| selected_version.set(e.value()),
                                        for v in available_versions() {
                                            option { value: "{v}", selected: v == selected_version(), "{v}" }
                                        }
                                    }
                                }
                                Button {
                                    variant: ButtonVariant::Ghost,
                                    title: "Refresh available versions",
                                    onclick: move |_| {
                                        spawn(async move {
                                            if let Ok(versions) = get_available_runtime_versions().await {
                                                if let Some(first) = versions.first() { selected_version.set(first.clone()); }
                                                available_versions.set(versions);
                                            }
                                        });
                                    },
                                    "↻"
                                }
                                Button {
                                    variant: ButtonVariant::Primary,
                                    title: if info.installed { "Download and replace the current PRoot binary" } else { "Download and install PRoot" },
                                    disabled: downloading(),
                                    onclick: handle_install,
                                    if downloading() { "Installing…" } else {
                                        Icon { icon: BsDownload }
                                        if info.installed { " Update" } else { " Install" }
                                    }
                                }
                                if info.installed {
                                    Button {
                                        variant: ButtonVariant::Ghost,
                                        title: "Remove the custom PRoot binary from disk (only works for non-system installs)",
                                        disabled: deleting(),
                                        onclick: handle_delete,
                                        if deleting() { "Removing…" } else {
                                            Icon { icon: BsTrash3 }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ── Storage card ─────────────────────────────────────────────
                div { class: "runtime-card",
                    div { class: "runtime-card-title", "Storage" }
                    div { class: "form-group",
                        label { title: "Directory where PRoot container image rootfs archives are extracted and stored.", "Images Directory" }
                        div { class: "folder-row",
                            input {
                                r#type: "text",
                                class: "folder-input",
                                value: "{proot_images_dir}",
                                placeholder: "./proot-images",
                                oninput: move |e| { proot_images_dir.set(e.value()); dir_changed.set(true); error.set(String::new()); success.set(String::new()); },
                            }
                            Button {
                                variant: ButtonVariant::Secondary,
                                title: "Browse for folder (server-side dialog)",
                                onclick: move |_| {
                                    spawn(async move {
                                        match crate::server::pick_projects_folder().await {
                                            Ok(path) if !path.is_empty() => {
                                                proot_images_dir.set(path);
                                                dir_changed.set(true);
                                                error.set(String::new());
                                            }
                                            Ok(_) => {}
                                            Err(e) => error.set(format!("Folder picker: {}", e)),
                                        }
                                    });
                                },
                                Icon { icon: BsFolder }
                            }
                        }
                        if dir_changed() {
                            div { class: "form-actions",
                                Button { variant: ButtonVariant::Primary, onclick: handle_save_dir, "Save" }
                                Button {
                                    variant: ButtonVariant::Secondary,
                                    onclick: move |_| {
                                        spawn(async move {
                                            if let Ok(s) = get_settings().await {
                                                proot_images_dir.set(s.proot_images_dir);
                                                dir_changed.set(false);
                                                error.set(String::new());
                                            }
                                        });
                                    },
                                    "Cancel"
                                }
                            }
                        }
                    }
                }

                // ── Images card ──────────────────────────────────────────────
                div { class: "runtime-card",
                    div { class: "runtime-card-title", "Images" }
                    RuntimeImagesSection { runtime_type: "proot".to_string(), on_default_changed }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Docker tab  (Status + Images, single page)
// ---------------------------------------------------------------------------

#[component]
fn DockerPanel(on_default_changed: EventHandler<()>) -> Element {
    let mut runtime_info = use_signal(|| None::<RuntimeInfo>);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(String::new);

    use_effect(move || {
        spawn(async move {
            loading.set(true);
            match get_docker_runtime_info().await {
                Ok(info) => runtime_info.set(Some(info)),
                Err(e) => error.set(format!("Failed to check Docker: {}", e)),
            }
            loading.set(false);
        });
    });

    rsx! {
        Banner { message: error(), banner_type: BannerType::Error, on_close: move |_| error.set(String::new()) }

        if loading() {
            p { class: "loading", "Loading…" }
        } else {
            div { class: "runtime-tab",

                // ── Status card ───────────────────────────────────────────────
                div { class: "runtime-card",
                    div { class: "runtime-card-title", "Status" }

                    if let Some(info) = runtime_info() {
                        div { class: "status-row",
                            span {
                                class: if info.supported { "status-badge ok" } else { "status-badge error" },
                                if info.supported { "✓ Available" } else { "✗ Not found" }
                            }
                            if let Some(v) = &info.version {
                                code { class: "version-text", "{v}" }
                            }
                        }
                        if let Some(reason) = &info.unsupported_reason {
                            p { class: "status-note error-note", "{reason}" }
                        }
                        if !info.supported {
                            p { class: "status-note",
                                "Install Docker from "
                                a { href: "https://docs.docker.com/get-docker/", target: "_blank",
                                    "docs.docker.com/get-docker"
                                }
                            }
                        }
                    }
                }

                // ── Images card ───────────────────────────────────────────────
                div { class: "runtime-card",
                    div { class: "runtime-card-title", "Images" }
                    RuntimeImagesSection { runtime_type: "docker".to_string(), on_default_changed }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared images section  (works for both PRoot and Docker)
// ---------------------------------------------------------------------------

/// Build the event callback for a prepare-image task.
#[derive(Clone, Copy)]
struct PrepareUiSignals {
    ready_tags: Signal<Vec<PreparedImageInfo>>,
    prepare_status: Signal<String>,
    preparing: Signal<bool>,
    preparing_tag: Signal<String>,
    error: Signal<String>,
    success: Signal<String>,
}

fn build_prepare_cb(
    tag: String,
    runtime_type: String,
    signals: PrepareUiSignals,
) -> impl FnMut(TaskEvent) + 'static {
    let PrepareUiSignals {
        mut ready_tags,
        mut prepare_status,
        mut preparing,
        mut preparing_tag,
        mut error,
        mut success,
    } = signals;

    move |event: TaskEvent| match event {
        TaskEvent::PrepareProgress(colmap_openmvs_api::PrepareProgress::Downloading {
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
        TaskEvent::PrepareProgress(colmap_openmvs_api::PrepareProgress::ExtractingLayer {
            layer,
            progress,
        }) => {
            prepare_status.set(format!("Layer {} {:.0}%", layer, progress * 100.0));
        }
        TaskEvent::PrepareProgress(colmap_openmvs_api::PrepareProgress::Error { message }) => {
            error.set(format!("Preparation failed: {}", message));
            prepare_status.set(String::new());
            preparing.set(false);
            preparing_tag.set(String::new());
        }
        TaskEvent::Completed => {
            success.set(format!("'{}' prepared successfully!", tag));
            prepare_status.set(String::new());
            preparing.set(false);
            preparing_tag.set(String::new());
            let rt = runtime_type.clone();
            spawn(async move {
                let imgs = if rt == "docker" {
                    list_docker_images().await
                } else {
                    list_runtime_images().await
                };
                if let Ok(imgs) = imgs {
                    ready_tags.set(imgs);
                }
            });
        }
        TaskEvent::Failed(msg) => {
            error.set(format!("Preparation failed: {}", msg));
            prepare_status.set(String::new());
            preparing.set(false);
            preparing_tag.set(String::new());
        }
        _ => {}
    }
}

#[component]
fn RuntimeImagesSection(runtime_type: String, on_default_changed: EventHandler<()>) -> Element {
    // Store runtime_type in a signal so closures can capture it without moving a String.
    let rt_signal = use_signal(|| runtime_type.clone());
    let rt = runtime_type.clone();
    let mut tasks_ctx = use_context::<TasksCtx>();
    let mut ready_tags = use_signal(Vec::<PreparedImageInfo>::new);
    let mut available_tags = use_signal(Vec::<ImageTagInfo>::new);
    let mut preparing = use_signal(|| false);
    let mut prepare_status = use_signal(String::new);
    let mut loading = use_signal(|| true);
    let mut tags_loading = use_signal(|| true);
    let mut error = use_signal(String::new);
    let mut success = use_signal(String::new);
    let mut default_image_tag = use_signal(String::new);
    let mut preparing_tag = use_signal(String::new);

    const COLMAP_IMAGE: &str = "mirror.gcr.io/yeicor/colmap-openmvs";

    // On mount: load images + available tags, reconnect any running task
    use_effect(move || {
        let rt_inner = rt.clone();
        spawn(async move {
            loading.set(true);
            tags_loading.set(true);

            // Load current default tag from settings
            if let Ok(s) = get_settings().await {
                let (settings_runtime, tag) = s.parse_default_image();
                // Only use this default if it's for the current runtime
                if settings_runtime == Some(rt_inner.as_str()) {
                    if let Some(tag_str) = tag {
                        default_image_tag.set(tag_str.to_string());
                    }
                }
            }

            // Load prepared/pulled images
            let images_result = if rt_inner == "docker" {
                list_docker_images().await
            } else {
                list_runtime_images().await
            };

            match images_result {
                Ok(imgs) => {
                    ready_tags.set(imgs);
                }
                Err(e) => error.set(format!("Failed to load images: {}", e)),
            }

            // Load available tags from registry
            match list_available_image_tags().await {
                Ok(tags) => available_tags.set(tags),
                Err(e) => error.set(format!("Failed to load available tags: {}", e)),
            }

            loading.set(false);
            tags_loading.set(false);

            // Reconnect to any in-progress prepare task
            let reconnect_id = list_tasks(Some("PrepareImage".to_string()), None)
                .await
                .ok()
                .and_then(|tasks| {
                    tasks
                        .into_iter()
                        .find(|t| t.state == TaskState::Running)
                        .map(|t| t.id)
                });

            if let Some(task_id) = reconnect_id {
                if let Ok(Some(info)) = get_task_info(task_id.clone()).await {
                    if info.state == TaskState::Running {
                        let tag = info
                            .context_key
                            .split_once(':')
                            .map(|x| x.1)
                            .unwrap_or("unknown")
                            .to_string();
                        preparing.set(true);
                        preparing_tag.set(tag.clone());
                        prepare_status.set("Reconnecting…".to_string());
                        let label = format!("Preparing {}", tag);
                        tasks_ctx
                            .write()
                            .register(task_id.clone(), label, TaskKind::PrepareImage);
                        let cb = build_prepare_cb(
                            tag,
                            rt_inner.clone(),
                            PrepareUiSignals {
                                ready_tags,
                                prepare_status,
                                preparing,
                                preparing_tag,
                                error,
                                success,
                            },
                        );
                        drive_task(task_id, tasks_ctx, cb);
                    }
                }
            }
        });
    });

    // Start a new prepare task
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
        let label = format!("Preparing {}", tag);
        let rt_spawn = rt_signal();
        spawn(async move {
            let result = if rt_spawn == "docker" {
                prepare_docker_image(full_image.clone()).await
            } else {
                prepare_runtime_image(full_image.clone()).await
            };
            match result {
                Ok(task_id) => {
                    let cb = build_prepare_cb(
                        tag,
                        rt_spawn,
                        PrepareUiSignals {
                            ready_tags,
                            prepare_status,
                            preparing,
                            preparing_tag,
                            error,
                            success,
                        },
                    );
                    start_task(task_id, label, TaskKind::PrepareImage, tasks_ctx, cb);
                }
                Err(e) => {
                    error.set(format!("Failed to start: {}", e));
                    prepare_status.set(String::new());
                    preparing.set(false);
                    preparing_tag.set(String::new());
                }
            }
        });
    };

    let handle_remove = move |remove_id: String| {
        let rt_rm = rt_signal();
        spawn(async move {
            let result = if rt_rm == "docker" {
                remove_docker_image(remove_id).await
            } else {
                remove_runtime_image(remove_id).await
            };
            match result {
                Ok(_) => {
                    success.set("Image removed.".to_string());
                    let imgs = if rt_rm == "docker" {
                        list_docker_images().await
                    } else {
                        list_runtime_images().await
                    };
                    if let Ok(imgs) = imgs {
                        ready_tags.set(imgs);
                    }
                }
                Err(e) => error.set(format!("Failed to remove: {}", e)),
            }
        });
    };

    let handle_set_default = move |tag: String| {
        let rt_sd = rt_signal();
        spawn(async move {
            match get_settings().await {
                Ok(mut s) => {
                    s.set_default_image(&rt_sd, &tag);
                    match update_settings(s).await {
                        Ok(_) => {
                            default_image_tag.set(tag.clone());
                            success.set(format!("Default set to '{}'", tag));
                            on_default_changed.call(());
                        }
                        Err(e) => error.set(format!("Failed: {}", e)),
                    }
                }
                Err(e) => error.set(format!("Failed to load settings: {}", e)),
            }
        });
    };

    let handle_unset_default = move |_| {
        spawn(async move {
            match get_settings().await {
                Ok(mut s) => {
                    s.clear_default_image();
                    match update_settings(s).await {
                        Ok(_) => {
                            default_image_tag.set(String::new());
                            success.set("Default cleared.".to_string());
                            on_default_changed.call(());
                        }
                        Err(e) => error.set(format!("Failed: {}", e)),
                    }
                }
                Err(e) => error.set(format!("Failed to load settings: {}", e)),
            }
        });
    };

    rsx! {
        Banner { message: error(), banner_type: BannerType::Error, on_close: move |_| error.set(String::new()) }
        Banner { message: success(), banner_type: BannerType::Info, on_close: move |_| success.set(String::new()) }

        // In-progress indicator
        if !prepare_status().is_empty() {
            div { class: "images-header",
                p { class: "prepare-progress", "⟳ Preparing '{preparing_tag}': {prepare_status}" }
            }
        }

        // ── Ready Images ────────────────────────────────────────────────────
        div { class: "tags-container",
            h2 { class: "section-title", "Ready" }

            if loading() {
                p { class: "loading", "Loading…" }
            } else if ready_tags().is_empty() {
                p { class: "empty", "No images ready. Pull one from the Available list below." }
            } else {
                ul { class: "tags-list",
                    {ready_tags().into_iter().map(|image| {
                        let tag = image.tag.clone();
                        let tag2 = image.tag.clone();
                        // For Docker, remove by tag; for PRoot, remove by hash (which equals tag)
                        let remove_id = image.hash.clone();
                        let build_date = image.build_date.clone();
                        let size_readable = image.size_readable.clone();
                        let size = image.size;
                        let is_default = tag == default_image_tag();
                        rsx! {
                            li { key: "{remove_id}", class: "tags-item",
                                div { class: "tags-item-top",
                                    span { class: "tag-name", title: "{tag}", "{tag}" }
                                    div { class: "tag-actions",
                                        if is_default {
                                            Button {
                                                variant: ButtonVariant::Primary,
                                                title: "Currently the default image — click to unset",
                                                onclick: handle_unset_default,
                                                "✓ Default"
                                            }
                                        } else {
                                            Button {
                                                variant: ButtonVariant::Secondary,
                                                title: "Use this image as the default for pipeline runs",
                                                onclick: move |_| handle_set_default(tag2.clone()),
                                                "Set Default"
                                            }
                                        }
                                        Button {
                                            variant: ButtonVariant::Destructive,
                                            title: "Remove this image",
                                            onclick: move |_| handle_remove(remove_id.clone()),
                                            Icon { icon: BsTrash3 }
                                        }
                                    }
                                }
                                div { class: "tags-item-meta",
                                    if let Some(date) = build_date { DateBadge { date } }
                                    span { class: "tag-meta-size", title: "{size} bytes", "💾 {size_readable}" }
                                }
                            }
                        }
                    })}
                }
            }
        }

        // ── Available Tags ──────────────────────────────────────────────────
        div { class: "tags-container",
            h2 { class: "section-title", "Available" }

            if tags_loading() {
                p { class: "loading", "Loading…" }
            } else if available_tags().is_empty() {
                p { class: "empty", "Could not load available tags." }
            } else {
                ul { class: "tags-list",
                    {available_tags().into_iter().map(|tag_info| {
                        let name = tag_info.name.clone();
                        let name2 = tag_info.name.clone();
                        let build_date = tag_info.build_date.clone();
                        rsx! {
                            li { key: "{name}", class: "tags-item",
                                div { class: "tags-item-top",
                                    span { class: "tag-name", title: "{name}", "{name}" }
                                    div { class: "tag-actions",
                                        Button {
                                            variant: ButtonVariant::Secondary,
                                            title: "Pull this image",
                                            disabled: preparing(),
                                            onclick: move |_| handle_prepare(name2.clone()),
                                            Icon { icon: BsDownload }
                                        }
                                    }
                                }
                                if let Some(date) = build_date {
                                    div { class: "tags-item-meta", DateBadge { date } }
                                }
                            }
                        }
                    })}
                }
            }
        }
    }
}
