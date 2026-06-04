use crate::components::alert_dialog::{
    AlertDialogAction, AlertDialogActions, AlertDialogCancel, AlertDialogContent, AlertDialogRoot,
    AlertDialogTitle,
};
use crate::mycomponents::{Banner, BannerType};
use crate::task_manager::{drive_task, start_task, TasksCtx};
use base64::Engine as _;
use colmap_openmvs_api::TaskEvent;
use colmap_openmvs_api::TaskKind;
use colmap_openmvs_api::TaskState;
use dioxus::document::eval;
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{
    BsArrowsFullscreen, BsBoxArrowUpRight, BsCheckAll, BsCloudDownload, BsGrid, BsImage, BsStar,
    BsTextareaResize, BsTrash3, BsUpload, BsViewList, BsXCircle,
};
use dioxus_free_icons::Icon;
use std::collections::HashMap;
use tracing::{debug, error, info};

// ---------------------------------------------------------------------------
// Blob-URL helpers
// ---------------------------------------------------------------------------

/// Convert raw image bytes into a URL the browser can load in `<img src>`.
///
/// * On **WASM** (web) targets: creates a native `Blob` URL via
///   `URL.createObjectURL`.  These are reference-counted by the browser and
///   must be released with `revoke_display_url` when no longer needed.
/// * On **native** (desktop / Android) targets: produces a `data:` URL which
///   the embedded WebView understands directly.  No explicit revocation is
///   needed — just drop the `String`.
fn bytes_to_display_url(bytes: &[u8]) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        use js_sys::{Array, Uint8Array};
        use web_sys::{Blob, BlobPropertyBag, Url};

        // Copy bytes into a JS Uint8Array, wrap in a Blob, then get an object URL.
        let typed = Uint8Array::new_with_length(bytes.len() as u32);
        typed.copy_from(bytes);
        let array = Array::of1(&typed);
        let opts = BlobPropertyBag::new();
        opts.set_type("image/*");
        if let Ok(blob) = Blob::new_with_u8_array_sequence_and_options(&array, &opts) {
            if let Ok(url) = Url::create_object_url_with_blob(&blob) {
                return url;
            }
        }
        // Fall through to data URL on any failure.
    }
    // Fallback / non-WASM: encode as a data URL; works in every WebView.
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:image/*;base64,{}", b64)
}

/// Release a blob URL created by `bytes_to_display_url`.  Safe to call with
/// data URLs (they start with `"data:"`, not `"blob:"`) — no-op in that case.
fn revoke_display_url(url: &str) {
    if url.starts_with("blob:") {
        #[cfg(target_arch = "wasm32")]
        let _ = web_sys::Url::revoke_object_url(url);
    }
}

// ---------------------------------------------------------------------------
// Event-callback builders (free functions – safe to call at any scope)
// Signals are Copy, so capturing them here is fine.
// ---------------------------------------------------------------------------

fn build_demo_cb(
    project_name: String,
    mut image_paths: Signal<Vec<String>>,
    mut list_version: Signal<u64>,
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
                        list_version += 1;
                        info_message.set(Some(format!("Demo ready ({} images).", n)));
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
    mut list_version: Signal<u64>,
    mut img_cache: Signal<HashMap<String, (String, usize)>>,
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
                    // Evict all cached URLs since every image's byte content changed.
                    for (_, (url, _)) in img_cache.write().drain() {
                        revoke_display_url(&url);
                    }
                    image_paths.set(paths);
                    list_version += 1;
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
    // Incremented on every mutation that may change image content (upload, delete,
    // resize, demo download).  The blob-URL cache effect depends on this signal
    // and on `image_paths`; a change to either triggers a cache rebuild.
    let mut list_version = use_signal(|| 0u64);
    // Cache: image_name → (display_url, size_bytes).
    // On WASM the display URL is a Blob URL; on native it is a data: URL.
    // Rebuilt whenever `list_version` or `image_paths` changes.
    let mut img_cache = use_signal(HashMap::<String, (String, usize)>::new);
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
    let mut show_images = use_signal(|| cfg!(not(target_os = "android")));

    // ── Load image list on mount + reconnect any running task ────────────
    let project_name_clone = project_name.clone();
    use_effect(move || {
        let project_name = project_name_clone.clone();
        debug!(project_name = %project_name, "Loading image list on mount");
        let demo_cb = build_demo_cb(
            project_name.clone(),
            image_paths,
            list_version,
            info_message,
            error_message,
            demo_loading,
        );
        let resize_cb = build_resize_cb(
            project_name.clone(),
            image_paths,
            list_version,
            img_cache,
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
                    list_version += 1;
                }
                Err(e) => {
                    error!(project_name = %project_name, error = %e, "Failed to load project images");
                    error_message.set(Some(format!("Failed to load images: {}", e)));
                }
            }

            // -- Reconnect demo task ------------------------------------------
            let reconnect_demo = {
                debug!(project_name = %project_name, "Looking for running demo task on server");
                crate::server::list_tasks(Some(TaskKind::DownloadDemo), Some(project_name.clone()))
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
            let reconnect_resize =
                crate::server::list_tasks(Some(TaskKind::BatchResize), Some(project_name.clone()))
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

    // ── Blob-URL cache management ─────────────────────────────────────────
    // Incrementally updates the cache: removes entries for deleted images,
    // fetches display URLs only for newly added ones, and leaves existing
    // entries untouched.  Callers that modify byte content (e.g. resize)
    // must clear `img_cache` before bumping `image_paths` to force a re-fetch.
    let project_name_cache = project_name.clone();
    use_effect(move || {
        let paths = image_paths();
        let version = list_version();

        let mut cache = img_cache.write();

        // Evict entries for images no longer in the project.
        cache.retain(|name, (url, _)| {
            if !paths.contains(name) {
                revoke_display_url(url);
                false
            } else {
                true
            }
        });

        // Only fetch images that aren't already cached.
        let needs_fetch: Vec<String> = paths
            .iter()
            .filter(|p| !cache.contains_key(*p))
            .cloned()
            .collect();
        drop(cache);

        if needs_fetch.is_empty() {
            return;
        }

        let project = project_name_cache.clone();
        spawn(async move {
            let fetches: Vec<_> = needs_fetch
                .iter()
                .map(|name| {
                    let p = project.clone();
                    let n = name.clone();
                    async move {
                        let result = crate::server::get_project_image_bytes(p, n.clone()).await;
                        (n, result)
                    }
                })
                .collect();
            for (name, result) in futures::future::join_all(fetches).await {
                if list_version() != version {
                    break;
                }
                match result {
                    Ok(stream) => {
                        let mut bytes = Vec::new();
                        let mut s = stream;
                        while let Some(chunk) = s.next().await {
                            match chunk {
                                Ok(data) => bytes.extend_from_slice(&data),
                                Err(e) => {
                                    error!(image_name = %name, error = %e, "Failed to read image stream");
                                    break;
                                }
                            }
                        }
                        let size = bytes.len();
                        let url = bytes_to_display_url(&bytes);
                        img_cache.write().insert(name, (url, size));
                    }
                    Err(e) => {
                        error!(image_name = %name, error = %e, "Failed to load image bytes");
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
                        list_version += 1;
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
                list_version,
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
                        image_paths.set(Vec::new());
                        list_version += 1;
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
                list_version,
                img_cache,
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
                    // Reuse the cached Blob / data URL — no extra network request.
                    let cache_entry = img_cache().get(&fullscreen_name).cloned();
                    let (full_image_url, size_bytes) =
                        cache_entry.unwrap_or_else(|| (String::new(), 0));
                    let size_mb = size_bytes as f64 / 1024.0 / 1024.0;
                    let img_id = format!("fullscreen-img-{}", fullscreen_name);
                    let metadata_id = format!("metadata-fullscreen-{}", fullscreen_name);
                    let img_id_onload = img_id.clone();
                    let metadata_id_onload = metadata_id.clone();
                    let fname_onload = fullscreen_name.clone();
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
                                    if full_image_url.is_empty() { "Loading…" } else { "" }
                                }
                                if !full_image_url.is_empty() {
                                    img {
                                        src: full_image_url.clone(),
                                        alt: fullscreen_name.clone(),
                                        class: "fullscreen-image",
                                        id: img_id.clone(),
                                        onload: move |_| {
                                            // Size is known from bytes; only read dimensions from the DOM.
                                            eval(&format!(
                                                r#"const img = document.getElementById('{id}');
                                                   const meta = document.getElementById('{mid}');
                                                   if (img && meta) {{
                                                     meta.innerHTML = img.naturalWidth + 'x' + img.naturalHeight
                                                       + ' · {sz:.3} MB · {fname}';
                                                   }}"#,
                                                id = img_id_onload,
                                                mid = metadata_id_onload,
                                                sz = size_mb,
                                                fname = fname_onload,
                                            ));
                                        }
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

                    button {
                        class: "btn btn-secondary",
                        title: if uploading() { "Uploading..." } else { "Upload images from disk" },
                        disabled: uploading() || demo_loading(),
                        onclick: {
                            let project_name = project_name.clone();
                            move |_| {
                                // Temporarily hide thumbnails before opening the file-picker
                                // — on Android the app may go to background and rendering
                                // many <img> elements can trigger OOM.
                                let was_showing = show_images();
                                show_images.set(false);
                                uploading.set(true);
                                error_message.set(None);
                                let pn = project_name.clone();
                                spawn(async move {
                                    // ── File picker (works everywhere: web, desktop, Android) ──
                                    // Opens the browser's native file-picker via a hidden
                                    // <input type="file"> element, reads each selected
                                    // file as bytes in the browser, and uploads them to
                                    // the server one-by-one through the existing
                                    // add_project_image endpoint.

                                    let files = crate::picker::pick_files(Some("image/*"), true).await;

                                    let count = files.len();
                                    let mut imported: Vec<String> = Vec::new();
                                    let mut failed: Vec<String> = Vec::new();

                                    for (name, bytes) in files {
                                        let byte_stream = crate::fullstack_compat::ByteStream::new(
                                            futures::stream::once(async {
                                                crate::fullstack_compat::body::Bytes::from(bytes)
                                            }),
                                        );
                                        match crate::server::add_project_image(
                                            pn.clone(),
                                            name.clone(),
                                            byte_stream,
                                        )
                                        .await
                                        {
                                            Ok(_) => imported.push(name),
                                            Err(e) => {
                                                error!("Failed to import image '{}': {}", name, e);
                                                failed.push(name);
                                            }
                                        }
                                    }

                                    if count > 0 {
                                        let fail_count = failed.len();
                                        info_message.set(Some(
                                            if fail_count > 0 {
                                                format!("Imported {} image(s), {} failed", count, fail_count)
                                            } else {
                                                format!("Imported {} image(s). You may want to optimize them using the 'Resize Images' button.", count)
                                            },
                                        ));
                                        if let Ok(paths) = crate::server::get_project_images(pn).await {
                                            image_paths.set(paths);
                                            list_version += 1;
                                        }
                                    }

                                    uploading.set(false);
                                    show_images.set(was_showing);
                                });
                            }
                        },
                        Icon { icon: BsUpload }
                        span {
                            if uploading() { "Uploading..." } else { "Upload" }
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
                            title: "Resize ALL images by resizing to a maximum dimension",
                            Icon { icon: BsTextareaResize }
                            span {
                                if resize_loading() {
                                    "Resizing..."
                                } else {
                                    "Resize Images"
                                }
                            }
                        }
                    }
                }

                div {
                    class: "toolbar-group",

                    button {
                        class: "btn btn-tertiary view-mode-toggle",
                        onclick: move |_| show_images.set(!show_images()),
                        title: if show_images() { "Switch to compact list view" } else { "Switch to image gallery" },
                        if show_images() {
                            Icon { icon: BsViewList }
                            span { "List" }
                        } else {
                            Icon { icon: BsGrid }
                            span { "Gallery" }
                        }
                    }

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

                    if has_images {
                        div {
                            class: "btn images-info",
                            "{num_images} " Icon { icon: BsImage }
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
            }

            if has_images {
                if show_images() {
                    div {
                        class: "image-gallery",
                        {
                            let paths = image_paths();
                            let selected = selected_images();
                            let cache = img_cache();
                            let mut elements = Vec::new();
                            for image_name in paths.into_iter() {
                                let is_selected = selected.contains(&image_name);
                                // Blob / data URL from the cache.  Shows nothing until the
                                // background fetch completes — the img element is simply not
                                // rendered, keeping the tile visible as a loading placeholder.
                                let Some((image_url, size_bytes)) = cache.get(&image_name).cloned()
                                    else { continue; };
                                let size_mb = size_bytes as f64 / 1024.0 / 1024.0;
                                // Safe ID (avoid special chars from file names).
                                let safe_image_name = urlencoding::encode(&image_name);
                                let img_id = format!("thumbnail-{}", safe_image_name);
                                let metadata_id = format!("metadata-{}", safe_image_name);
                                let image_name_for_checkbox = image_name.clone();
                                let image_name_for_fullscreen = image_name.clone();
                                let image_name_for_img = image_name.clone();
                                elements.push(rsx! {
                                    div {
                                        key: "{image_name}",
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
                                                // Size is already known; read only dimensions from the DOM.
                                                let js = format!(
                                                    r#"const img = document.getElementById('{id}');
                                                       const meta = document.getElementById('{mid}');
                                                       if (img && meta) {{
                                                         meta.innerHTML = img.naturalWidth + 'x' + img.naturalHeight
                                                           + '<br/>{sz:.3} MB';
                                                       }}"#,
                                                    id = img_id,
                                                    mid = metadata_id,
                                                    sz = size_mb,
                                                );
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
                        class: "image-list-compact",
                        for image_name in image_paths() {
                            div {
                                key: "{image_name}",
                                class: if selected_images().contains(&image_name) { "image-list-item selected" } else { "image-list-item" },
                                input {
                                    r#type: "checkbox",
                                    checked: selected_images().contains(&image_name),
                                    onchange: {
                                        let name = image_name.clone();
                                        move |_| toggle_select(name.clone())
                                    },
                                }
                                span {
                                    class: "item-name",
                                    onclick: {
                                        let name = image_name.clone();
                                        move |_| fullscreen_image.set(Some(name.clone()))
                                    },
                                    "{image_name}"
                                }
                                button {
                                    class: "list-fullscreen-btn",
                                    title: "View fullscreen",
                                    onclick: {
                                        let name = image_name.clone();
                                        move |_| fullscreen_image.set(Some(name.clone()))
                                    },
                                    Icon { icon: BsArrowsFullscreen }
                                }
                                span { class: "item-size",
                                    {
                                        let sz = img_cache().get(&image_name).map(|(_, s)| *s);
                                        match sz {
                                            Some(s) => format!("{:.2} MB", s as f64 / 1048576.0),
                                            None => String::new(),
                                        }
                                    }
                                }
                            }
                        }
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
                    AlertDialogTitle { "Resize ALL Images" }
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
                            "Resize"
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
