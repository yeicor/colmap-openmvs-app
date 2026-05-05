use crate::components::{
    button::{Button, ButtonVariant},
    tabs::{TabContent, TabList, TabTrigger, Tabs},
};
use crate::mycomponents::page_header::BackButton;
use crate::mycomponents::{Banner, BannerType, PageHeader};
use crate::server::{
    download_runtime_version, get_available_runtime_versions, get_runtime_info, get_settings,
    list_available_image_tags, list_runtime_images, prepare_runtime_image, remove_runtime_image,
    update_settings,
};
use crate::Route;
use colmap_openmvs_api::{ImageTagInfo, PrepareProgress, PreparedImageInfo, RuntimeInfo, Settings};
use dioxus::document::eval;
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{
    BsBoxSeam, BsDownload, BsFolder, BsGear, BsTerminal, BsTrash3,
};
use dioxus_free_icons::Icon;

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

            match get_available_runtime_versions().await {
                Ok(versions) => {
                    if let Some(first) = versions.first() {
                        selected_version.set(first.clone());
                    }
                    available_versions.set(versions);
                }
                Err(_) => {} // non-fatal – user can retry
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

    const COLMAP_IMAGE: &str = "mirror.gcr.io/yeicor/colmap-openmvs";

    // Initial image list and available tags
    use_effect(move || {
        spawn(async move {
            loading.set(true);
            tags_loading.set(true);

            match list_runtime_images().await {
                Ok(imgs) => ready_tags.set(imgs),
                Err(e) => error.set(format!("Failed to load images: {}", e)),
            }

            match list_available_image_tags().await {
                Ok(tags) => {
                    available_tags.set(tags);
                }
                Err(e) => error.set(format!("Failed to load tags: {}", e)),
            }

            loading.set(false);
            tags_loading.set(false);
        });
    });

    let mut handle_prepare = move |tag: String| {
        if tag.is_empty() || preparing() {
            return;
        }
        preparing.set(true);
        prepare_status.set("Starting…".to_string());
        error.set(String::new());
        success.set(String::new());

        let full_image = format!("{}:{}", COLMAP_IMAGE, tag);
        spawn(async move {
            match prepare_runtime_image(full_image.clone()).await {
                Ok(mut stream) => {
                    while let Some(Ok(event)) = stream.recv().await {
                        match event {
                            PrepareProgress::Downloading {
                                downloaded_bytes,
                                total_bytes,
                            } => {
                                let mb = downloaded_bytes as f64 / 1_048_576.0;
                                prepare_status.set(if let Some(total) = total_bytes {
                                    format!("{:.1}/{:.1} MB", mb, total as f64 / 1_048_576.0)
                                } else {
                                    format!("{:.1} MB", mb)
                                });
                            }
                            PrepareProgress::ExtractingLayer { layer, progress } => {
                                prepare_status.set(format!(
                                    "Layer {} {:.0}%",
                                    layer,
                                    progress * 100.0
                                ));
                            }
                            PrepareProgress::Error { message } => {
                                error.set(format!("Preparation failed: {}", message));
                                prepare_status.set(String::new());
                                preparing.set(false);
                                return;
                            }
                        }
                    }
                    success.set(format!("Tag '{}' prepared successfully!", tag));
                    prepare_status.set(String::new());
                    if let Ok(imgs) = list_runtime_images().await {
                        ready_tags.set(imgs);
                    }
                    preparing.set(false);
                }
                Err(e) => {
                    error.set(format!("Failed to start preparation: {}", e));
                    prepare_status.set(String::new());
                    preparing.set(false);
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

        // ── Header with runtime info ────────────────────────────────────────
        div {
            class: "images-header",
            p { class: "images-info", "For: PRoot runtime" }
            if !prepare_status().is_empty() {
                p { class: "prepare-progress", "⟳ {prepare_status}" }
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
                    for image in ready_tags() {
                        li {
                            key: "{image.hash}",
                            class: "tags-item",
                            div {
                                class: "tag-info",
                                span { class: "tag-name", "{image.tag}" }
                                if let Some(date) = &image.build_date {
                                    span { class: "tag-date", "{date}" }
                                }
                            }
                            div {
                                class: "tag-actions",
                                span { class: "tag-size", "{image.size_readable}" }
                                Button {
                                    variant: ButtonVariant::Destructive,
                                    title: "Remove this tag",
                                    onclick: move |_| handle_remove(image.hash.clone()),
                                    Icon { icon: BsTrash3 }
                                }
                            }
                        }
                    }
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
                    for tag_info in available_tags() {
                        li {
                            key: "{tag_info.name}",
                            class: "tags-item",
                            div {
                                class: "tag-info",
                                span { class: "tag-name", "{tag_info.name}" }
                                if let Some(date) = tag_info.build_date {
                                    span { class: "tag-date", "{date}" }
                                }
                            }
                            div {
                                class: "tag-actions",
                                Button {
                                    variant: ButtonVariant::Secondary,
                                    title: "Download and prepare this tag",
                                    onclick: move |_| handle_prepare(tag_info.name.clone()),
                                    disabled: preparing(),
                                    Icon { icon: BsDownload }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
