use crate::components::alert_dialog::{
    AlertDialogAction, AlertDialogActions, AlertDialogCancel, AlertDialogContent, AlertDialogRoot,
    AlertDialogTitle,
};
use crate::mycomponents::{Banner, BannerType};
use crate::task_manager::{drive_task, start_task, TasksCtx};
use colmap_openmvs_api::TaskEvent;
use colmap_openmvs_api::TaskKind;
use colmap_openmvs_api::TaskState;
use dioxus::document::eval;
use dioxus::fullstack::get_server_url;
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{
    BsArrowsFullscreen, BsBoxArrowUpRight, BsCheckAll, BsCloudDownload, BsImage, BsStar,
    BsTextareaResize, BsTrash3, BsUpload, BsXCircle,
};
use dioxus_free_icons::Icon;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use tracing::{debug, error, info};

static CACHE_BUSTER: LazyLock<AtomicU64> = LazyLock::new(|| AtomicU64::new(0));
fn generate_cache_busting_num() -> u64 {
    CACHE_BUSTER.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Event-callback builders (free functions – safe to call at any scope)
// Signals are Copy, so capturing them here is fine.
// ---------------------------------------------------------------------------

fn build_demo_cb(
    project_name: String,
    mut image_paths: Signal<Vec<String>>,
    mut info_message: Signal<Option<String>>,
    mut error_message: Signal<Option<String>>,
    mut demo_loading: Signal<bool>,
) -> impl FnMut(TaskEvent) + 'static {
    move |event: TaskEvent| match event {
        TaskEvent::DemoProgress(colmap_openmvs_api::DemoProgressEvent::FetchingFileList) => {
            info_message.set(Some("Fetching file list…".to_string()));
        }
        TaskEvent::DemoProgress(colmap_openmvs_api::DemoProgressEvent::DownloadProgress {
            filename,
            downloaded,
            total: t,
        }) => {
            if filename.is_empty() {
                info_message.set(Some(format!("Downloaded {} / {} images.", downloaded, t)));
            } else {
                info_message.set(Some(format!(
                    "Downloading… ({}/{}: {})",
                    downloaded + 1,
                    t,
                    filename
                )));
            }
        }
        TaskEvent::DemoProgress(colmap_openmvs_api::DemoProgressEvent::Error { message }) => {
            error_message.set(Some(message));
            demo_loading.set(false);
        }
        TaskEvent::Completed => {
            let p = project_name.clone();
            spawn(async move {
                match crate::server::get_project_images(p.clone()).await {
                    Ok(paths) => {
                        let n = paths.len();
                        image_paths.set(paths);
                        info_message.set(Some(format!(
                            "Demo ready ({} images). You may want to optimize them using the 'Optimize Images' button.",
                            n
                        )));
                    }
                    Err(e) => error_message.set(Some(format!("Failed to reload images: {}", e))),
                }
                demo_loading.set(false);
            });
        }
        TaskEvent::Failed(msg) => {
            error_message.set(Some(format!("Demo download failed: {}", msg)));
            demo_loading.set(false);
        }
        _ => {}
    }
}

fn build_resize_cb(
    project_name: String,
    mut image_paths: Signal<Vec<String>>,
    mut info_message: Signal<Option<String>>,
    mut error_message: Signal<Option<String>>,
    mut resize_loading: Signal<bool>,
) -> impl FnMut(TaskEvent) + 'static {
    move |event: TaskEvent| match event {
        TaskEvent::ResizeProgress(colmap_openmvs_api::ResizeProgressEvent::ResizeProgress {
            name,
            completed,
            total_files,
        }) => {
            info_message.set(Some(format!(
                "Resized: {} ({}/{})",
                name, completed, total_files
            )));
        }
        TaskEvent::ResizeProgress(colmap_openmvs_api::ResizeProgressEvent::Error { message }) => {
            error_message.set(Some(message));
            resize_loading.set(false);
        }
        TaskEvent::Completed => {
            info_message.set(Some("Batch resize complete.".to_string()));
            resize_loading.set(false);
            let p = project_name.clone();
            spawn(async move {
                if let Ok(paths) = crate::server::get_project_images(p).await {
                    image_paths.set(paths);
                }
            });
        }
        TaskEvent::Failed(msg) => {
            error_message.set(Some(format!("Resize failed: {}", msg)));
            resize_loading.set(false);
        }
        _ => {}
    }
}

#[component]
pub fn ImagesTab(project_name: String) -> Element {
    debug!(project_name = %project_name, "Initializing images tab");
    let mut tasks_ctx = use_context::<TasksCtx>();
    let mut image_paths = use_signal(Vec::<String>::new);
    let mut selected_images = use_signal(Vec::<String>::new);
    let mut demo_loading = use_signal(|| false);
    let mut demo_dialog_open = use_signal(|| false);
    let mut resize_loading = use_signal(|| false);
    let mut resize_dialog_open = use_signal(|| false);
    let mut resize_max_dimension = use_signal(|| 1024u32);
    let mut error_message = use_signal::<Option<String>>(|| None);
    let mut info_message = use_signal::<Option<String>>(|| None);
    let mut fullscreen_image = use_signal::<Option<String>>(|| None);
    let mut uploading = use_signal(|| false);

    // ── Load image list on mount + reconnect any running task ────────────
    let project_name_clone = project_name.clone();
    use_effect(move || {
        let project_name = project_name_clone.clone();
        debug!(project_name = %project_name, "Loading image list on mount");
        let demo_cb = build_demo_cb(
            project_name.clone(),
            image_paths,
            info_message,
            error_message,
            demo_loading,
        );
        let resize_cb = build_resize_cb(
            project_name.clone(),
            image_paths,
            info_message,
            error_message,
            resize_loading,
        );
        spawn(async move {
            match crate::server::get_project_images(project_name.clone()).await {
                Ok(imgs) => {
                    let count = imgs.len();
                    info!(project_name = %project_name, image_count = count, "Successfully loaded project images");
                    image_paths.set(imgs);
                }
                Err(e) => {
                    error!(project_name = %project_name, error = %e, "Failed to load project images");
                    error_message.set(Some(format!("Failed to load images: {}", e)));
                }
            }

            // -- Reconnect demo task ------------------------------------------
            let reconnect_demo = {
                debug!(project_name = %project_name, "Looking for running demo task on server");
                crate::server::list_tasks(
                    Some("DownloadDemo".to_string()),
                    Some(project_name.clone()),
                )
                .await
                .ok()
                .and_then(|tasks| {
                    tasks
                        .into_iter()
                        .find(|t| t.state == TaskState::Running)
                        .map(|t| t.id)
                })
            };
            if let Some(task_id) = reconnect_demo {
                if let Ok(Some(info)) = crate::server::get_task_info(task_id.clone()).await {
                    if info.state == TaskState::Running {
                        demo_loading.set(true);
                        info_message.set(Some("Reconnecting to demo download…".to_string()));
                        let label = format!("Demo: {}", project_name);
                        tasks_ctx
                            .write()
                            .register(task_id.clone(), label, TaskKind::DownloadDemo);
                        drive_task(task_id, tasks_ctx, demo_cb);
                    }
                }
            }

            // -- Reconnect resize task ----------------------------------------
            let reconnect_resize = crate::server::list_tasks(
                Some("BatchResize".to_string()),
                Some(project_name.clone()),
            )
            .await
            .ok()
            .and_then(|tasks| {
                tasks
                    .into_iter()
                    .find(|t| t.state == TaskState::Running)
                    .map(|t| t.id)
            });
            if let Some(task_id) = reconnect_resize {
                if let Ok(Some(info)) = crate::server::get_task_info(task_id.clone()).await {
                    if info.state == TaskState::Running {
                        resize_loading.set(true);
                        info_message.set(Some("Reconnecting to resize task…".to_string()));
                        let label = format!("Resize: {}", project_name);
                        tasks_ctx
                            .write()
                            .register(task_id.clone(), label, TaskKind::BatchResize);
                        drive_task(task_id, tasks_ctx, resize_cb);
                    }
                }
            }
        });
    });

    let on_delete_selected = {
        let project_name = project_name.clone();
        move |_| {
            let project_name = project_name.clone();
            let to_delete: Vec<String> = selected_images().clone();
            spawn(async move {
                for image_name in to_delete {
                    let _ =
                        crate::server::delete_project_image(project_name.clone(), image_name).await;
                }
                match crate::server::get_project_images(project_name).await {
                    Ok(imgs) => {
                        image_paths.set(imgs);
                        info_message.set(Some("Images deleted successfully".to_string()));
                    }
                    Err(e) => {
                        error_message.set(Some(format!("Failed to reload images: {}", e)));
                    }
                }
            });
            selected_images.set(Vec::new());
        }
    };

    // Factory that creates per-dataset onclick handlers; all signals are Copy so the
    // only thing we need to clone is the project_name String.
    let make_demo_handler = |source_id: &'static str| {
        let project_name = project_name.clone();
        move |_| {
            demo_loading.set(true);
            demo_dialog_open.set(false);
            info_message.set(Some(format!("Starting {} dataset download…", source_id)));
            let project_name = project_name.clone();
            let cb = build_demo_cb(
                project_name.clone(),
                image_paths,
                info_message,
                error_message,
                demo_loading,
            );
            spawn(async move {
                let task_id = match crate::server::download_demo_images(
                    project_name.clone(),
                    source_id.to_string(),
                )
                .await
                {
                    Ok(id) => id,
                    Err(e) => {
                        error_message.set(Some(format!("Failed to start demo download: {}", e)));
                        demo_loading.set(false);
                        return;
                    }
                };
                let label = format!("Demo: {}", project_name);
                start_task(task_id, label, TaskKind::DownloadDemo, tasks_ctx, cb);
            });
        }
    };
    let on_load_demo_et = make_demo_handler("ET");
    let on_load_demo_kermit = make_demo_handler("kermit");

    let on_clear_all = {
        let project_name = project_name.clone();
        move |_| {
            let project_name = project_name.clone();
            spawn(async move {
                match crate::server::clear_project_images(project_name).await {
                    Ok(_) => {
                        info_message.set(Some("All images cleared successfully".to_string()));
                    }
                    Err(e) => {
                        error_message.set(Some(format!("Failed to clear images: {}", e)));
                    }
                }
            });
        }
    };

    let on_open_resize_dialog = move |_| {
        #[cfg(any(feature = "mobile", target_os = "android", target_os = "ios"))]
        resize_max_dimension.set(1024);
        #[cfg(not(any(feature = "mobile", target_os = "android", target_os = "ios")))]
        resize_max_dimension.set(2048);
        resize_dialog_open.set(true);
    };

    let on_confirm_resize = {
        let project_name = project_name.clone();
        move |_| {
            resize_dialog_open.set(false);
            resize_loading.set(true);
            info_message.set(Some("Starting batch resize...".to_string()));
            let project_name = project_name.clone();
            let max_dimension = resize_max_dimension();
            let cb = build_resize_cb(
                project_name.clone(),
                image_paths,
                info_message,
                error_message,
                resize_loading,
            );
            spawn(async move {
                let task_id =
                    match crate::server::batch_resize_images(project_name.clone(), max_dimension)
                        .await
                    {
                        Ok(id) => id,
                        Err(e) => {
                            error_message.set(Some(format!("Failed to start batch resize: {}", e)));
                            resize_loading.set(false);
                            return;
                        }
                    };
                let label = format!("Resize: {}", project_name);
                start_task(task_id, label, TaskKind::BatchResize, tasks_ctx, cb);
            });
        }
    };
    let on_file_upload = {
        let project_name = project_name.clone();
        move |evt: FormEvent| {
            uploading.set(true);
            error_message.set(None);
            let project_name = project_name.clone();
            spawn(async move {
                let mut count = 0;
                for file in evt.files() {
                    match file.read_bytes().await {
                        Ok(bytes) => {
                            match crate::server::add_project_image(
                                project_name.clone(),
                                file.name(),
                                bytes.to_vec(),
                            )
                            .await
                            {
                                Ok(_) => {
                                    count += 1;
                                }
                                Err(e) => {
                                    error_message.set(Some(format!(
                                        "Failed to upload {}: {}",
                                        file.name(),
                                        e
                                    )));
                                }
                            }
                        }
                        Err(e) => {
                            error_message.set(Some(format!(
                                "Failed to read {}: {}",
                                file.name(),
                                e
                            )));
                        }
                    }
                }
                info_message.set(Some(format!("Uploaded {} image(s)", count)));
                uploading.set(false);
            });
        }
    };

    let mut selected_images2 = selected_images;
    let mut toggle_select = move |image_name: String| {
        let mut selected = selected_images();
        if selected.contains(&image_name) {
            selected.retain(|x| x != &image_name);
        } else {
            selected.push(image_name);
        }
        selected_images2.set(selected);
    };

    let select_all = move |_| {
        if selected_images().len() == image_paths().len() && !image_paths().is_empty() {
            selected_images.set(Vec::new());
        } else {
            selected_images.set(image_paths());
        }
    };

    let has_images = !image_paths().is_empty();
    let has_selection = !selected_images().is_empty();
    let all_selected = selected_images().len() == image_paths().len() && has_images;
    let num_images = image_paths().len();
    let num_selected = selected_images().len();

    rsx! {
        div {
            class: "tab-content images-tab",

            Banner {
                message: error_message().unwrap_or_default(),
                banner_type: BannerType::Error,
                on_close: move |_| error_message.set(None),
            }

            Banner {
                message: info_message().unwrap_or_default(),
                banner_type: BannerType::Info,
                on_close: move |_| info_message.set(None),
            }

            // ── Demo dataset selector modal ─────────────────────────────────────
            if demo_dialog_open() {
                div {
                    class: "demo-selector-overlay",
                    onclick: move |_| demo_dialog_open.set(false),

                    div {
                        class: "demo-selector-modal",
                        onclick: move |evt| evt.stop_propagation(),

                        div {
                            class: "demo-selector-header",
                            h2 { class: "demo-selector-title", Icon { icon: BsStar } " Download Demo Image Datasets" }
                            button {
                                class: "demo-close-btn",
                                title: "Close",
                                onclick: move |_| demo_dialog_open.set(false),
                                "×"
                            }
                        }

                        div {
                            class: "demo-cards",

                            // ── E.T. card ──────────────────────────────────────
                            div {
                                class: "demo-card",
                                div {
                                    class: "demo-card-body",
                                    div { class: "demo-card-icon", "👽" }
                                    div {
                                        class: "demo-card-body-text",
                                        h3 { class: "demo-card-title", "E.T." }
                                        div {
                                            class: "demo-card-meta",
                                            span { class: "demo-card-author", "By Noah Snavely" }
                                            span { class: "demo-card-license", "GPL" }
                                        }
                                    }
                                }
                                div {
                                    class: "demo-card-actions",
                                    a {
                                        href: "https://github.com/snavely/bundler_sfm/tree/master/examples/ET",
                                        target: "_blank",
                                        rel: "noopener noreferrer",
                                        class: "btn demo-source-link",
                                        Icon { icon: BsBoxArrowUpRight }
                                        "Source"
                                    }
                                    button {
                                        class: "btn btn-secondary",
                                        disabled: demo_loading() || uploading(),
                                        onclick: on_load_demo_et,
                                        Icon { icon: BsCloudDownload }
                                        "Download"
                                    }
                                }
                            }

                            // ── Kermit card ────────────────────────────────────
                            div {
                                class: "demo-card",
                                div {
                                    class: "demo-card-body",
                                    div { class: "demo-card-icon", "🐸" }
                                    div {
                                        class: "demo-card-body-text",
                                        h3 { class: "demo-card-title", "Kermit" }
                                        div {
                                            class: "demo-card-meta",
                                            span { class: "demo-card-author", "By Noah Snavely" }
                                            span { class: "demo-card-license", "GPL" }
                                        }
                                    }
                                }
                                div {
                                    class: "demo-card-actions",
                                    a {
                                        href: "https://github.com/snavely/bundler_sfm/tree/master/examples/kermit",
                                        target: "_blank",
                                        rel: "noopener noreferrer",
                                        class: "btn demo-source-link",
                                        Icon { icon: BsBoxArrowUpRight }
                                        "Source"
                                    }
                                    button {
                                        class: "btn btn-secondary",
                                        disabled: demo_loading() || uploading(),
                                        onclick: on_load_demo_kermit,
                                        Icon { icon: BsCloudDownload }
                                        "Download"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── Fullscreen image viewer ───────────────────────────────────────
            {
                if let Some(fullscreen_name) = fullscreen_image() {
                    let safe_fullscreen_name = fullscreen_name.clone();
                    let safe_project_name = project_name.clone();
                    let full_image_url = format!("{}/projects/{}/images/{}",
                        get_server_url(),
                        safe_project_name,
                        safe_fullscreen_name
                    );
                    let img_id = format!("fullscreen-img-{}", safe_fullscreen_name);
                    let metadata_id = format!("metadata-fullscreen-{}", safe_fullscreen_name);
                    rsx! {
                        div {
                            class: "fullscreen-modal",
                            onclick: move |_| fullscreen_image.set(None),

                            div {
                                class: "fullscreen-container",
                                onclick: move |evt| evt.stop_propagation(),

                                button {
                                    class: "fullscreen-close",
                                    onclick: move |_| fullscreen_image.set(None),
                                    title: "Close (ESC)",
                                    "×"
                                }

                                div {
                                    class: "fullscreen-caption",
                                    id: metadata_id.clone(),
                                    "Loading..."
                                }
                                img {
                                    src: full_image_url.clone(),
                                    alt: fullscreen_name.clone(),
                                    class: "fullscreen-image",
                                    id: img_id.clone(),
                                    onload: move |_| {
                                        eval(&format!(r#"
                                            (async () => {{
                                                const img = document.getElementById('{}');
                                                const metadataDiv = document.getElementById('{}');
                                                if (img && metadataDiv) {{
                                                    const width = img.naturalWidth;
                                                    const height = img.naturalHeight;
                                                    let sizeBytes = 0;
                                                    try {{
                                                        const res = await fetch('{}', {{ method: 'HEAD' }});
                                                        const size = res.headers.get('Content-Length');
                                                        sizeBytes = parseInt(size);
                                                    }} catch (e) {{}}
                                                    metadataDiv.innerHTML = `${{width}}x${{height}} · ${{(sizeBytes / 1024 / 1024).toFixed(3)}} MB · {}`;
                                                }}
                                            }})();
                                        "#, img_id, metadata_id, full_image_url, fullscreen_name));
                                    }
                                }
                            }
                        }
                    }
                } else {
                    rsx! {}
                }
            }

            div {
                class: "images-toolbar",

                div {
                    class: "toolbar-group",

                    form {
                        onsubmit: move |evt| {
                            evt.prevent_default();
                            eval("document.getElementById('file-upload-input').click()");
                        },
                        div {
                            class: "file-input-wrapper",
                            input {
                                id: "file-upload-input",
                                r#type: "file",
                                name: "images",
                                class: "hidden-file-input",
                                multiple: true,
                                accept: "image/*",
                                disabled: uploading() || demo_loading(),
                                onchange: on_file_upload,
                            }
                            button {
                                r#type: "submit",
                                class: "btn btn-secondary",
                                title: if uploading() { "Uploading..." } else { "Upload images from disk" },
                                disabled: uploading() || demo_loading(),
                                Icon { icon: BsUpload }
                                span {
                                    if uploading() {
                                        "Uploading..."
                                    } else {
                                        "Upload"
                                    }
                                }
                            }
                        }
                    }

                    button {
                        class: "btn btn-secondary",
                        onclick: move |_| demo_dialog_open.set(true),
                        disabled: demo_loading() || uploading(),
                        title: "Choose and download demo images from bundler_sfm examples",
                        Icon { icon: BsStar }
                        span {
                            if demo_loading() {
                                "Downloading…"
                            } else {
                                "Demo Images"
                            }
                        }
                    }

                    if has_images {
                        button {
                            class: "btn btn-secondary",
                            onclick: on_open_resize_dialog,
                            disabled: resize_loading() || uploading() || demo_loading(),
                            title: "Optimize ALL images by resizing to a maximum dimension",
                            Icon { icon: BsTextareaResize }
                            span {
                                if resize_loading() {
                                    "Optimizing..."
                                } else {
                                    "Optimize Images"
                                }
                            }
                        }
                    }
                }

                div {
                    class: "toolbar-group",

                    if has_images {
                        button {
                            class: "btn btn-tertiary",
                            onclick: select_all,
                            title: if all_selected { "Deselect all" } else { "Select all" },
                            Icon { icon: BsCheckAll }
                            span {
                                if all_selected {
                                    "Deselect All"
                                } else {
                                    "Select All"
                                }
                            }
                        }
                    }

                    if has_selection {
                        button {
                            class: "btn btn-danger",
                            onclick: on_delete_selected,
                            title: "Delete selected images",
                            Icon { icon: BsTrash3 }
                            span { "Delete ({num_selected})" }
                        }
                    }

                    if has_images {
                        button {
                            class: "btn btn-danger",
                            onclick: on_clear_all,
                            title: "Delete all images",
                            Icon { icon: BsXCircle }
                            span { "Clear All" }
                        }
                    }
                }

                if has_images {
                    div {
                        class: "btn images-info",
                        "{num_images} " Icon { icon: BsImage }  // TODO: better icon and pluralization
                    }
                }
            }

            if has_images {
                div {
                    class: "image-gallery",
                    {
                        let paths = image_paths();
                        let selected = selected_images();
                        let mut elements = Vec::new();
                        for image_name in paths.into_iter() {
                            let safe_image_name = urlencoding::encode(&image_name);
                            let safe_project_name = urlencoding::encode(&project_name);
                            let is_selected = selected.contains(&image_name);
                            let image_url = format!(
                                "{}/projects/{}/images/{}?_drop_cache={}",
                                get_server_url(),
                                safe_project_name,
                                safe_image_name,
                                generate_cache_busting_num(),
                            );
                            let img_id = format!("thumbnail-{}", safe_image_name);
                            let metadata_id = format!("metadata-{}", safe_image_name);
                            let image_name_for_checkbox = image_name.clone();
                            let image_name_for_fullscreen = image_name.clone();
                            let image_name_for_img = image_name.clone();
                            elements.push(rsx! {
                                div {
                                    class: if is_selected { "image-item selected" } else { "image-item" },

                                    div {
                                        class: "image-checkbox",
                                        input {
                                            r#type: "checkbox",
                                            checked: is_selected,
                                            onchange: move |_| toggle_select(image_name_for_checkbox.clone()),
                                            id: format!("checkbox-{}", safe_image_name),
                                        }
                                    }

                                    button {
                                        class: "image-fullscreen-btn",
                                        title: "View fullscreen",
                                        onclick: move |_| fullscreen_image.set(Some(image_name_for_fullscreen.clone())),
                                        Icon { icon: BsArrowsFullscreen }
                                    }

                                    div {
                                        class: "image-info-overlay",
                                        div {
                                            class: "image-name",
                                            div {
                                                class: "image-metadata",
                                                id: metadata_id.clone(),
                                                "Loading..."
                                            }
                                            "{image_name}"
                                        }
                                    }

                                    img {
                                        src: image_url.clone(),
                                        alt: image_name.clone(),
                                        id: img_id.clone(),
                                        onclick: move |_| toggle_select(image_name_for_img.clone()),
                                        class: "thumbnail",
                                        onload: move |_| {
                                            let js = format!(r#"
                                                (async () => {{
                                                    const img = document.getElementById('{}');
                                                    const metadataDiv = document.getElementById('{}');
                                                    if (img && metadataDiv) {{
                                                        const width = img.naturalWidth;
                                                        const height = img.naturalHeight;
                                                        let sizeBytes = 0;
                                                        try {{
                                                            const res = await fetch('{}', {{ method: 'HEAD' }});
                                                            const size = res.headers.get('Content-Length');
                                                            sizeBytes = parseInt(size);
                                                        }} catch (e) {{}}
                                                        metadataDiv.innerHTML = `${{width}}x${{height}}<br/>${{(sizeBytes / 1024 / 1024).toFixed(3)}} MB`;
                                                    }}
                                                }})();
                                            "#, img_id, metadata_id, image_url);
                                            eval(&js);
                                        }
                                    }
                                }
                            });
                        }
                        rsx! { for element in elements { {element} } }
                    }
                }

            } else {
                div {
                    class: "empty-gallery",
                    Icon { icon: BsImage, class: "empty-icon" }
                    p { "No images in this project" }
                    p {
                        class: "hint",
                        "Upload images, capture photos, or download demo images to get started."
                    }
                }
            }

            AlertDialogRoot {
                open: resize_dialog_open(),
                AlertDialogContent {
                    AlertDialogTitle { "Optimize ALL Images" }
                    div {
                        class: "resize-dialog-content",
                        p { "Maximum dimension (in pixels):" }
                        div {
                            class: "resize-slider-container",
                            input {
                                r#type: "range",
                                min: "64",
                                max: "8192",
                                step: "32",
                                value: "{resize_max_dimension}",
                                oninput: move |evt| {
                                    if let Ok(val) = evt.value().parse::<u32>() {
                                        resize_max_dimension.set(val);
                                    }
                                }
                            }
                            span {
                                class: "resize-value-display",
                                "{resize_max_dimension()} px"
                            }
                        }
                    }
                    AlertDialogActions {
                        AlertDialogAction {
                            on_click: on_confirm_resize,
                            "Optimize"
                        }
                        AlertDialogCancel {
                            on_click: move |_| resize_dialog_open.set(false),
                            "Cancel"
                        }
                    }
                }
            }
        }
    }
}
