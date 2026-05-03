use crate::mycomponents::{Banner, BannerType};
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{
    BsArrowsFullscreen, BsCamera, BsDownload, BsImage, BsTrash3, BsUpload, BsXCircle,
};
use dioxus_free_icons::Icon;

#[component]
pub fn ImagesTab(project_name: String) -> Element {
    let mut images = use_signal(|| Vec::<String>::new());
    let mut selected_images = use_signal(|| Vec::<String>::new());
    let mut demo_loading = use_signal(|| false);
    let mut demo_progress = use_signal(|| 0u32);
    let mut error_message = use_signal::<Option<String>>(|| None);
    let mut success_message = use_signal::<Option<String>>(|| None);
    let mut fullscreen_image = use_signal::<Option<String>>(|| None);
    let mut uploading = use_signal(|| false);

    // Load images on mount
    let project_name_clone = project_name.clone();
    use_effect(move || {
        let project_name = project_name_clone.clone();
        spawn(async move {
            match crate::server::get_project_images(project_name).await {
                Ok(imgs) => {
                    images.set(imgs);
                    error_message.set(None);
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
                        images.set(imgs);
                        success_message.set(Some("Images deleted successfully".to_string()));
                        error_message.set(None);
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
            demo_progress.set(0);
            error_message.set(None);
            success_message.set(None);
            let project_name = project_name.clone();
            spawn(async move {
                match crate::server::download_demo_images(project_name.clone()).await {
                    Ok(_) => {
                        demo_progress.set(100);
                        match crate::server::get_project_images(project_name).await {
                            Ok(imgs) => {
                                images.set(imgs);
                                success_message
                                    .set(Some("Demo images downloaded successfully".to_string()));
                                error_message.set(None);
                            }
                            Err(e) => {
                                error_message
                                    .set(Some(format!("Failed to load demo images: {}", e)));
                            }
                        }
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
                        images.set(Vec::new());
                        selected_images.set(Vec::new());
                        success_message.set(Some("All images cleared successfully".to_string()));
                        error_message.set(None);
                    }
                    Err(e) => {
                        error_message.set(Some(format!("Failed to clear images: {}", e)));
                    }
                }
            });
        }
    };

    let on_file_upload = {
        let project_name = project_name.clone();
        move |_evt: FormEvent| {
            let project_name = project_name.clone();
            
            // Get files from the file input element
            #[cfg(target_arch = "wasm32")]
            {
                if let Ok(Some(window)) = (|| Ok::<Option<web_sys::Window>, ()>(web_sys::window()) )() {
                    if let Some(document) = window.document() {
                        if let Some(input_element) = document.get_element_by_id("file-upload-input") {
                            if let Some(input) = input_element.dyn_ref::<web_sys::HtmlInputElement>() {
                                if let Some(file_list) = input.files() {
                                    let project_name_clone = project_name.clone();
                                    spawn(async move {
                                        uploading.set(true);
                                        error_message.set(None);
                                        success_message.set(None);

                                        let mut upload_errors = Vec::new();
                                        let mut successful_uploads = 0;

                                        // Process each selected file
                                        for i in 0..file_list.length() {
                                            if let Some(file) = file_list.get(i) {
                                                let file_name = file.name();
                                                let file_size = file.size();

                                                // Read file as bytes
                                                match read_file_as_bytes(&file).await {
                                                    Ok(bytes) => {
                                                        // Upload to server
                                                        match crate::server::add_project_image(
                                                            project_name_clone.clone(),
                                                            file_name.clone(),
                                                            bytes,
                                                        )
                                                        .await
                                                        {
                                                            Ok(_) => {
                                                                successful_uploads += 1;
                                                                println!(
                                                                    "Successfully uploaded: {} ({} bytes)",
                                                                    file_name, file_size
                                                                );
                                                            }
                                                            Err(e) => {
                                                                let err_msg =
                                                                    format!("Failed to upload '{}': {}", file_name, e);
                                                                upload_errors.push(err_msg);
                                                            }
                                                        }
                                                    }
                                                    Err(e) => {
                                                        let err_msg =
                                                            format!("Failed to read file '{}': {}", file_name, e);
                                                        upload_errors.push(err_msg);
                                                    }
                                                }
                                            }
                                        }

                                        // Refresh image list
                                        if successful_uploads > 0 {
                                            match crate::server::get_project_images(project_name_clone.clone())
                                                .await
                                            {
                                                Ok(imgs) => {
                                                    images.set(imgs);
                                                    let msg = if upload_errors.is_empty() {
                                                        format!("Successfully uploaded {} image(s)", successful_uploads)
                                                    } else {
                                                        format!(
                                                            "Uploaded {} image(s) with {} error(s)",
                                                            successful_uploads,
                                                            upload_errors.len()
                                                        )
                                                    };
                                                    success_message.set(Some(msg));
                                                }
                                                Err(e) => {
                                                    error_message
                                                        .set(Some(format!("Failed to reload images: {}", e)));
                                                }
                                            }
                                        }

                                        // Show errors if any
                                        if !upload_errors.is_empty() {
                                            error_message.set(Some(upload_errors.join("; ")));
                                        }

                                        uploading.set(false);

                                        // Clear file input
                                        if let Ok(Some(window)) = (|| Ok::<Option<web_sys::Window>, ()>(web_sys::window()) )() {
                                            if let Some(document) = window.document() {
                                                if let Some(input_element) = document.get_element_by_id("file-upload-input") {
                                                    if let Some(input) = input_element.dyn_ref::<web_sys::HtmlInputElement>() {
                                                        input.set_value("");
                                                    }
                                                }
                                            }
                                        }
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    let on_camera_capture = {
        let project_name = project_name.clone();
        move |_evt: FormEvent| {
            let project_name = project_name.clone();
            
            // Get file from camera input element
            #[cfg(target_arch = "wasm32")]
            {
                if let Ok(Some(window)) = (|| Ok::<Option<web_sys::Window>, ()>(web_sys::window()) )() {
                    if let Some(document) = window.document() {
                        if let Some(input_element) = document.get_element_by_id("camera-input") {
                            if let Some(input) = input_element.dyn_ref::<web_sys::HtmlInputElement>() {
                                if let Some(file_list) = input.files() {
                                    if file_list.length() > 0 {
                                        if let Some(file) = file_list.get(0) {
                                            let file_name = file.name();
                                            spawn(async move {
                                                uploading.set(true);
                                                error_message.set(None);
                                                success_message.set(None);

                                                // Read file as bytes
                                                match read_file_as_bytes(&file).await {
                                                    Ok(bytes) => {
                                                        // Upload to server
                                                        match crate::server::add_project_image(
                                                            project_name.clone(),
                                                            file_name.clone(),
                                                            bytes,
                                                        )
                                                        .await
                                                        {
                                                            Ok(_) => {
                                                                // Refresh image list
                                                                match crate::server::get_project_images(
                                                                    project_name.clone(),
                                                                )
                                                                .await
                                                                {
                                                                    Ok(imgs) => {
                                                                        images.set(imgs);
                                                                        success_message.set(Some(
                                                                            format!("Image '{}' captured and uploaded successfully", file_name),
                                                                        ));
                                                                        error_message.set(None);
                                                                    }
                                                                    Err(e) => {
                                                                        error_message.set(Some(
                                                                            format!("Image uploaded but failed to reload list: {}", e),
                                                                        ));
                                                                    }
                                                                }
                                                            }
                                                            Err(e) => {
                                                                error_message.set(Some(format!(
                                                                    "Failed to upload image '{}': {}",
                                                                    file_name, e
                                                                )));
                                                            }
                                                        }
                                                    }
                                                    Err(e) => {
                                                        error_message.set(Some(format!(
                                                            "Failed to read camera image: {}",
                                                            e
                                                        )));
                                                    }
                                                }

                                                uploading.set(false);

                                                // Clear file input
                                                if let Ok(Some(window)) = (|| Ok::<Option<web_sys::Window>, ()>(web_sys::window()) )() {
                                                    if let Some(document) = window.document() {
                                                        if let Some(input_element) = document.get_element_by_id("camera-input") {
                                                            if let Some(input) = input_element.dyn_ref::<web_sys::HtmlInputElement>() {
                                                                input.set_value("");
                                                            }
                                                        }
                                                    }
                                                }
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
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
        if selected_images().len() == images().len() && !images().is_empty() {
            selected_images.set(Vec::new());
        } else {
            selected_images.set(images());
        }
    };

    let has_images = !images().is_empty();
    let has_selection = !selected_images().is_empty();
    let all_selected = selected_images().len() == images().len() && has_images;
    let num_images = images().len();
    let num_selected = selected_images().len();

    rsx! {
        div {
            class: "tab-content images-tab",

            // Error banner
            Banner {
                message: error_message().unwrap_or_default(),
                banner_type: BannerType::Error,
                on_close: move |_| error_message.set(None),
            }

            // Success banner
            Banner {
                message: success_message().unwrap_or_default(),
                banner_type: BannerType::Info,
                on_close: move |_| success_message.set(None),
            }

            // Fullscreen modal
            if let Some(image_name) = fullscreen_image() {
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

                        img {
                            src: format!("/api/projects/{}/images/{}", project_name, image_name),
                            alt: image_name.clone(),
                            class: "fullscreen-image",
                        }

                        div {
                            class: "fullscreen-caption",
                            "{image_name}"
                        }
                    }
                }
            }

            // Toolbar
            div {
                class: "images-toolbar",

                div {
                    class: "toolbar-group",

                    // Upload from disk
                    div {
                        class: "file-input-wrapper",
                        input {
                            r#type: "file",
                            id: "file-upload-input",
                            class: "hidden-file-input",
                            multiple: true,
                            accept: "image/*",
                            disabled: uploading(),
                            oninput: on_file_upload,
                        }
                        label {
                            r#for: "file-upload-input",
                            class: "btn btn-secondary",
                            title: if uploading() { "Uploading..." } else { "Upload images from disk" },
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

                    // Camera capture
                    div {
                        class: "file-input-wrapper",
                        input {
                            r#type: "file",
                            id: "camera-input",
                            class: "hidden-file-input",
                            accept: "image/*",
                            capture: "environment",
                            disabled: uploading(),
                            oninput: on_camera_capture,
                        }
                        label {
                            r#for: "camera-input",
                            class: "btn btn-secondary",
                            title: if uploading() { "Uploading..." } else { "Capture image with camera" },
                            Icon { icon: BsCamera }
                            span {
                                if uploading() {
                                    "Uploading..."
                                } else {
                                    "Camera"
                                }
                            }
                        }
                    }

                    // Download demo images
                    button {
                        class: "btn btn-secondary",
                        onclick: on_load_demo,
                        disabled: demo_loading() || uploading(),
                        title: "Download demo images from ETH3D",
                        Icon { icon: BsDownload }
                        span {
                            if demo_loading() {
                                if demo_progress() > 0 && demo_progress() < 100 {
                                    "Downloading {demo_progress()}%"
                                } else {
                                    "Downloading..."
                                }
                            } else {
                                "Demo Images"
                            }
                        }
                    }
                }

                div {
                    class: "toolbar-group",

                    // Select all / Deselect all
                    if has_images {
                        button {
                            class: "btn btn-tertiary",
                            onclick: select_all,
                            title: if all_selected { "Deselect all" } else { "Select all" },
                            if all_selected {
                                "Deselect All"
                            } else {
                                "Select All"
                            }
                        }
                    }

                    // Delete selected
                    if has_selection {
                        button {
                            class: "btn btn-danger",
                            onclick: on_delete_selected,
                            title: "Delete selected images",
                            Icon { icon: BsTrash3 }
                            span { "Delete ({num_selected})" }
                        }
                    }

                    // Clear all
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

            // Image count info
            if has_images {
                div {
                    class: "images-info",
                    span { "{num_images} image(s)" }
                    if has_selection {
                        span { class: "highlight", "{num_selected} selected" }
                    }
                }
            }

            // Gallery
            if has_images {
                div {
                    class: "image-gallery",
                    for image_name in images() {
                        {
                            let image_url = format!("/api/projects/{}/images/{}", project_name, image_name);
                            let is_selected = selected_images().contains(&image_name);
                            let image_name2 = image_name.clone();
                            let image_name3 = image_name.clone();
                            let image_name4 = image_name.clone();

                            rsx! {
                                div {
                                    key: "{image_name}",
                                    class: if is_selected { "image-item selected" } else { "image-item" },

                                    // Checkbox overlay
                                    div {
                                        class: "image-checkbox",
                                        input {
                                            r#type: "checkbox",
                                            checked: is_selected,
                                            onchange: move |_| toggle_select(image_name.clone()),
                                        }
                                    }

                                    // Fullscreen button overlay (top-right)
                                    button {
                                        class: "image-fullscreen-btn",
                                        title: "View fullscreen",
                                        onclick: move |_| fullscreen_image.set(Some(image_name4.clone())),
                                        Icon { icon: BsArrowsFullscreen }
                                    }

                                    // Image
                                    img {
                                        src: image_url,
                                        alt: image_name2.clone(),
                                        title: image_name2.clone(),
                                        onclick: move |_| toggle_select(image_name2.clone()),
                                    }

                                    // Image name
                                    div {
                                        class: "image-name",
                                        title: image_name3.clone(),
                                        "{image_name}"
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
        }
    }
}

/// Read a web_sys File as bytes using the FileReader API
/// Only available for wasm32 target
#[cfg(target_arch = "wasm32")]
async fn read_file_as_bytes(file: &web_sys::File) -> Result<Vec<u8>, String> {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    // Create a FileReader
    let reader = web_sys::FileReader::new()
        .map_err(|_| "Failed to create FileReader".to_string())?;

    // Channel to signal when load is complete
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let tx = std::sync::Arc::new(tokio::sync::Mutex::new(tx));

    let onload_closure = Closure::once(move |_: web_sys::ProgressEvent| {
        let tx = tx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let mut tx_guard = tx.lock().await;
            let _ = tx_guard.send(()).await;
        });
    });

    reader.set_onload(Some(onload_closure.as_ref().unchecked_ref()));
    onload_closure.forget();

    // Start reading the file
    reader
        .read_as_array_buffer(file)
        .map_err(|_| "Failed to start file read".to_string())?;

    // Wait for the load event
    rx.recv()
        .await
        .ok_or_else(|| "File read was cancelled".to_string())?;

    // Get the ArrayBuffer result
    let result = reader
        .result()
        .map_err(|_| "Failed to get file content".to_string())?;

    // Convert ArrayBuffer to Vec<u8>
    let typed_array = js_sys::Uint8Array::new(&result);
    Ok(typed_array.to_vec())
}

/// Stub implementation for non-wasm targets
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
async fn read_file_as_bytes(_file: &web_sys::File) -> Result<Vec<u8>, String> {
    Err("File reading not supported on this platform".to_string())
}
