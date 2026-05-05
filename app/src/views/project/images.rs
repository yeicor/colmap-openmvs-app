use crate::components::alert_dialog::{
    AlertDialogAction, AlertDialogActions, AlertDialogCancel, AlertDialogContent, AlertDialogRoot,
    AlertDialogTitle,
};
use crate::mycomponents::{Banner, BannerType};
use colmap_openmvs_api::DemoProgressEvent;
use colmap_openmvs_api::ResizeProgressEvent;
use dioxus::document::eval;
use dioxus::fullstack::get_server_url;
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{
    BsArrowsFullscreen, BsCheckAll, BsImage, BsStar, BsTextareaResize, BsTrash3, BsUpload,
    BsXCircle,
};
use dioxus_free_icons::Icon;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;

static CACHE_BUSTER: LazyLock<AtomicU64> = LazyLock::new(|| AtomicU64::new(0));
fn generate_cache_busting_num() -> u64 {
    CACHE_BUSTER.fetch_add(1, Ordering::Relaxed)
}

#[component]
pub fn ImagesTab(project_name: String) -> Element {
    let mut image_paths = use_signal(|| Vec::<String>::new());
    let mut selected_images = use_signal(|| Vec::<String>::new());
    let mut demo_loading = use_signal(|| false);
    let mut resize_loading = use_signal(|| false);
    let mut resize_dialog_open = use_signal(|| false);
    let mut resize_max_dimension = use_signal(|| 1024u32);
    let mut error_message = use_signal::<Option<String>>(|| None);
    let mut info_message = use_signal::<Option<String>>(|| None);
    let mut fullscreen_image = use_signal::<Option<String>>(|| None);
    let mut uploading = use_signal(|| false);

    // Load image list on mount
    let project_name_clone = project_name.clone();
    use_effect(move || {
        info_message.read_unchecked(); // trigger re-run when info_message changes to show updates during loading
        error_message.read_unchecked(); // trigger re-run when error_message changes to show updates during loading
        let project_name = project_name_clone.clone();
        spawn(async move {
            match crate::server::get_project_images(project_name).await {
                Ok(imgs) => {
                    image_paths.set(imgs);
                }
                Err(e) => {
                    error_message.set(Some(format!("Failed to load images: {}", e)));
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

    let on_load_demo = {
        let project_name = project_name.clone();
        move |_| {
            demo_loading.set(true);
            info_message.set(Some("Starting download...".to_string()));
            let project_name = project_name.clone();
            spawn(async move {
                match crate::server::download_demo_images(project_name.clone()).await {
                    Ok(mut stream) => {
                        let mut total_files = 0;
                        let mut total_bytes = 0;
                        while let Some(Ok(event)) = stream.recv().await {
                            match event {
                                DemoProgressEvent::DownloadProgress {
                                    downloaded_bytes,
                                    total_bytes,
                                } => {
                                    let downloaded_mb = downloaded_bytes as f64 / 1_000_000.0;
                                    let total_mb = total_bytes as f64 / 1_000_000.0;
                                    info_message.set(Some(format!(
                                        "Downloading... ({:.1} / {:.1} MB)",
                                        downloaded_mb, total_mb
                                    )));
                                }
                                DemoProgressEvent::ExtractionProgress {
                                    #[allow(unused_variables)]
                                    last_file,
                                    total_files: files,
                                    total_bytes: bytes,
                                } => {
                                    let size_mb = bytes as f64 / 1_000_000.0;
                                    total_files = files; // update total files count
                                    total_bytes = bytes; // update total bytes count
                                    info_message.set(Some(format!(
                                        "Extracting... ({} files, {:.1} MB)",
                                        files, size_mb
                                    )));
                                }
                                DemoProgressEvent::Error { message } => {
                                    error_message.set(Some(message));
                                    demo_loading.set(false);
                                    return;
                                }
                            }
                        }
                        info_message.set(Some(format!(
                            "Demo downloaded ({} images, {:.1} MB). You may want to optimize them using the 'Optimize Images' button.",
                            total_files,
                            total_bytes as f64 / 1_000_000.0
                        )));
                    }
                    Err(e) => {
                        error_message.set(Some(format!("Failed to download demo images: {}", e)));
                    }
                }
                demo_loading.set(false);
            });
        }
    };

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
            spawn(async move {
                match crate::server::batch_resize_images(project_name.clone(), max_dimension).await
                {
                    Ok(mut stream) => {
                        while let Some(Ok(event)) = stream.recv().await {
                            match event {
                                ResizeProgressEvent::ResizeProgress {
                                    name,
                                    completed,
                                    total_files,
                                } => {
                                    info_message.set(Some(format!(
                                        "Resized: {} ({}/{})",
                                        name, completed, total_files
                                    )));
                                }
                                ResizeProgressEvent::Error { message } => {
                                    error_message.set(Some(message));
                                    resize_loading.set(false);
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error_message.set(Some(format!("Failed to start batch resize: {}", e)));
                    }
                }
                resize_loading.set(false);
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

    let mut selected_images2 = selected_images.clone();
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
                } else { rsx! {}}
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
                        onclick: on_load_demo,
                        disabled: demo_loading() || uploading(),
                        title: "Download demo images from ETH3D (Creative Commons Attribution-NonCommercial-ShareAlike 4.0 International License)",
                        Icon { icon: BsStar }
                        span {
                            if demo_loading() {
                                "Downloading..."
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
