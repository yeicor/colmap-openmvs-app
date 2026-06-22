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
use dioxus::core::use_drop;
use dioxus::document::eval;
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{
    BsArrowsFullscreen, BsBoxArrowUpRight, BsCameraVideo, BsCheckAll, BsCloudDownload, BsGrid,
    BsImage, BsStar, BsTextareaResize, BsTrash3, BsUpload, BsViewList, BsXCircle,
};
use dioxus_free_icons::Icon;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use tracing::{debug, error, info};

// ---------------------------------------------------------------------------
// Cancellable image-fetch helpers
//
// On WASM (web browsers) we use direct `fetch` requests with an `AbortController`
// so every in-flight HTTP request is torn down the instant the user navigates
// away — this keeps the browser's connection‑pool free for other server‑function
// calls.  On desktop / Android the backend is localhost, so we simply use the
// normal Dioxus server function and rely on Dioxus 0.7 dropping all spawned
// scoped tasks on unmount.
// ---------------------------------------------------------------------------

/// Maximum concurrent image byte fetches.
/// Keeps some room in the browser connection pool for other requests.
const MAX_CONCURRENT_FETCHES: usize = 6;

/// Helper: escape a string for embedding in a JS single-quoted string literal.
#[cfg(not(target_arch = "wasm32"))]
fn js_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Fetch media size via HEAD request using JS eval (non-WASM).
/// Supports both images and videos via the `endpoint_type` parameter.
#[cfg(all(not(feature = "demo"), not(target_arch = "wasm32")))]
async fn fetch_media_size_eval(
    project_name: &str,
    media_name: &str,
    endpoint_type: &str, // "images" or "videos"
) -> Result<u64, String> {
    let server_url = {
        #[cfg(feature = "fullstack")]
        {
            dioxus::fullstack::get_server_url()
        }
        #[cfg(not(feature = "fullstack"))]
        {
            String::new()
        }
    };
    let server_url_esc = js_escape(&server_url);
    let project_esc = js_escape(project_name);
    let media_esc = js_escape(media_name);

    let js = format!(
        r#"(async function() {{
    const baseUrl = '{server_url_esc}' || (/^https?:/.test(window.location.origin) ? window.location.origin : 'http://localhost:8080');
    const url = baseUrl + '/api/projects/' + encodeURIComponent('{project_esc}') + '/{endpoint_type}/' + encodeURIComponent('{media_esc}') + '/bytes';
    try {{
        const resp = await fetch(url, {{method: 'HEAD'}});
        if (!resp.ok) throw new Error('HTTP ' + resp.status);
        const len = resp.headers.get('Content-Length');
        const result = (len !== null) ? len : '0';
        dioxus.send(result);
    }} catch(e) {{
        dioxus.send('error:' + e.message);
    }}
}})();"#
    );

    let mut eval_handle = eval(&js);
    let result = eval_handle
        .recv::<String>()
        .await
        .map_err(|e| format!("eval error: {e}"))?;

    if let Some(err) = result.strip_prefix("error:") {
        tracing::warn!(
            "JS HTTP HEAD for {} size failed ({}), falling back to Rust server function — performance may be reduced",
            endpoint_type, err
        );
        // Only images have a Rust fallback (videos just return error)
        if endpoint_type == "images" {
            fetch_image_size_rust_fallback(project_name, media_name).await
        } else {
            Err(err.to_string())
        }
    } else {
        result
            .parse::<u64>()
            .map_err(|e| format!("Invalid size: {e}"))
    }
}

/// Fetch image size via HEAD request using JS eval (non-WASM).
/// Uses the HTTP endpoint with `get_server_url()` for the correct base URL.
#[cfg(all(not(feature = "demo"), not(target_arch = "wasm32")))]
async fn fetch_image_size_eval(project_name: &str, image_name: &str) -> Result<u64, String> {
    fetch_media_size_eval(project_name, image_name, "images").await
}

/// Fallback: get image size by fetching bytes via the Rust server function.
/// Used when the JS HTTP HEAD request fails (e.g. on Android with no TCP server).
#[cfg(all(not(feature = "demo"), not(target_arch = "wasm32")))]
async fn fetch_image_size_rust_fallback(
    project_name: &str,
    image_name: &str,
) -> Result<u64, String> {
    let bytes = fetch_image_bytes_rust_fallback(project_name, image_name).await?;
    Ok(bytes.len() as u64)
}

/// Fetch image bytes via HTTP GET using JS eval (non-WASM).
/// Uses the HTTP endpoint with `get_server_url()` for the correct base URL.
/// Falls back to calling the Rust server function directly if the JS fetch fails
/// (e.g. on Android where no TCP server is running).
#[cfg(not(target_arch = "wasm32"))]
async fn fetch_image_bytes_eval(project_name: &str, image_name: &str) -> Result<Vec<u8>, String> {
    let server_url = {
        #[cfg(feature = "fullstack")]
        {
            dioxus::fullstack::get_server_url()
        }
        #[cfg(not(feature = "fullstack"))]
        {
            String::new()
        }
    };
    let server_url_esc = js_escape(&server_url);
    let project_esc = js_escape(project_name);
    let image_esc = js_escape(image_name);

    let js = format!(
        r#"(async function() {{
    const baseUrl = '{server_url_esc}' || (/^https?:/.test(window.location.origin) ? window.location.origin : 'http://localhost:8080');
    const url = baseUrl + '/api/projects/' + encodeURIComponent('{project_esc}') + '/images/' + encodeURIComponent('{image_esc}') + '/bytes';
    try {{
        const resp = await fetch(url);
        if (!resp.ok) throw new Error('HTTP ' + resp.status);
        const blob = await resp.blob();
        const reader = new FileReader();
        reader.onload = function() {{
            const b64 = reader.result.split(',')[1];
            dioxus.send(b64);
            dioxus.send('__done__');
        }};
        reader.readAsDataURL(blob);
    }} catch(e) {{
        dioxus.send('__error__:' + e.message);
    }}
}})();"#
    );

    let mut eval_handle = eval(&js);
    let b64_data = eval_handle
        .recv::<String>()
        .await
        .map_err(|e| format!("eval error: {e}"))?;

    if let Some(err) = b64_data.strip_prefix("__error__:") {
        tracing::warn!(
            "JS HTTP fetch for image bytes failed ({}), falling back to Rust server function — performance may be reduced",
            err
        );
        return fetch_image_bytes_rust_fallback(project_name, image_name).await;
    }

    // Wait for the __done__ sentinel
    let _ = eval_handle.recv::<String>().await;

    base64::engine::general_purpose::STANDARD
        .decode(&b64_data)
        .map_err(|e| format!("Base64 decode error: {e}"))
}

/// Fallback: collect image bytes via the Rust server function directly.
/// Used when the JS HTTP fetch fails (e.g. on Android with no TCP server).
#[cfg(not(target_arch = "wasm32"))]
async fn fetch_image_bytes_rust_fallback(
    project_name: &str,
    image_name: &str,
) -> Result<Vec<u8>, String> {
    let stream = crate::server::get_project_image_bytes_stream(
        project_name.to_string(),
        image_name.to_string(),
    )
    .await
    .map_err(|e| format!("Rust server function failed: {e}"))?;
    crate::fullstack_compat::collect_bytes_from_stream(stream).await
}

/// Platform‑specific fetch for a single image's bytes.
///
/// In demo mode, embedded byte data is returned directly — no HTTP request
/// is made (no server exists in the static web build).
/// `signal` is an `AbortSignal` on WASM (passed from the component‑level
/// `AbortController` that gets aborted on unmount) and unused on other targets.
#[cfg(target_arch = "wasm32")]
async fn fetch_image_bytes_impl(
    project_name: &str,
    image_name: &str,
    signal: &web_sys::AbortSignal,
) -> Result<Vec<u8>, String> {
    // In demo mode the embedded bytes are available at compile time — use
    // them directly instead of making (doomed) HTTP requests to a static site.
    #[cfg(feature = "demo")]
    if let Some(bytes) = crate::demo::demo_image_bytes(image_name) {
        return Ok(bytes.to_vec());
    }
    use js_sys::{Promise, Uint8Array};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::wasm_bindgen::JsCast;

    let window = web_sys::window().ok_or("No window available")?;
    let prefix = crate::backend_url::BACKEND_URL
        .get()
        .map(|s| s.as_str())
        .unwrap_or("");
    let encoded_project = urlencoding::encode(project_name);
    let encoded_image = urlencoding::encode(image_name);
    let url = format!("{prefix}/api/projects/{encoded_project}/images/{encoded_image}/bytes");

    let opts = web_sys::RequestInit::new();
    opts.set_method("GET");
    opts.set_signal(Some(signal));

    let request = web_sys::Request::new_with_str_and_init(&url, &opts)
        .map_err(|e| format!("Failed to create request: {e:?}"))?;

    let response_promise: Promise = window.fetch_with_request(&request);
    let response_val = JsFuture::from(response_promise)
        .await
        .map_err(|e| format!("Fetch failed: {e:?}"))?;

    let response: web_sys::Response = response_val
        .dyn_into()
        .map_err(|_| "Response is not a Response object".to_string())?;

    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }

    let array_buffer = response
        .array_buffer()
        .map_err(|e| format!("Failed to get array buffer: {e:?}"))?;
    let buffer_val = JsFuture::from(array_buffer)
        .await
        .map_err(|e| format!("Failed to read array buffer: {e:?}"))?;

    let uint8 = Uint8Array::new(&buffer_val);
    let mut bytes = vec![0u8; uint8.length() as usize];
    uint8.copy_to(&mut bytes);
    Ok(bytes)
}

/// Non‑WASM fallback – fetches image bytes via HTTP from JS eval.
/// Uses `get_server_url()` so it works correctly in desktop mode.
#[cfg(not(target_arch = "wasm32"))]
async fn fetch_image_bytes_impl(project_name: &str, image_name: &str) -> Result<Vec<u8>, String> {
    #[cfg(feature = "demo")]
    if let Some(bytes) = crate::demo::demo_image_bytes(image_name) {
        return Ok(bytes.to_vec());
    }
    fetch_image_bytes_eval(project_name, image_name).await
}

/// Fetch the `Content-Length` of a media file via a HEAD request (WASM only).
/// This avoids downloading any bytes just to show a file size in list mode.
#[cfg(target_arch = "wasm32")]
async fn fetch_media_size_wasm(
    project_name: &str,
    media_name: &str,
    endpoint_type: &str, // "images" or "videos"
) -> Result<u64, String> {
    #[cfg(feature = "demo")]
    if endpoint_type == "images" {
        if let Some(bytes) = crate::demo::demo_image_bytes(media_name) {
            return Ok(bytes.len() as u64);
        }
    }
    use js_sys::Promise;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::wasm_bindgen::JsCast;

    let window = web_sys::window().ok_or("No window available")?;
    let prefix = crate::backend_url::BACKEND_URL
        .get()
        .map(|s| s.as_str())
        .unwrap_or("");
    let encoded_project = urlencoding::encode(project_name);
    let encoded_media = urlencoding::encode(media_name);
    let url =
        format!("{prefix}/api/projects/{encoded_project}/{endpoint_type}/{encoded_media}/bytes");

    let opts = web_sys::RequestInit::new();
    opts.set_method("HEAD");

    let request = web_sys::Request::new_with_str_and_init(&url, &opts)
        .map_err(|e| format!("Failed to create HEAD request: {e:?}"))?;

    let response_promise: Promise = window.fetch_with_request(&request);
    let response_val = JsFuture::from(response_promise)
        .await
        .map_err(|e| format!("HEAD request failed: {e:?}"))?;

    let response: web_sys::Response = response_val
        .dyn_into()
        .map_err(|_| "Response is not a Response object".to_string())?;

    if !response.ok() {
        return Err(format!("HEAD returned HTTP {}", response.status()));
    }

    response
        .headers()
        .get("Content-Length")
        .map_err(|e| format!("Failed to get Content-Length header: {e:?}"))?
        .ok_or_else(|| "No Content-Length header in response".to_string())?
        .parse::<u64>()
        .map_err(|e| format!("Invalid Content-Length: {e}"))
}

/// Fetch the `Content-Length` of an image via a HEAD request (WASM only).
/// This avoids downloading any image bytes just to show a file size in list mode.
#[cfg(target_arch = "wasm32")]
async fn fetch_image_size_wasm(project_name: &str, image_name: &str) -> Result<u64, String> {
    // In demo mode, return the embedded byte length directly.
    #[cfg(feature = "demo")]
    if let Some(bytes) = crate::demo::demo_image_bytes(image_name) {
        return Ok(bytes.len() as u64);
    }
    use js_sys::Promise;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::wasm_bindgen::JsCast;

    let window = web_sys::window().ok_or("No window available")?;
    let prefix = crate::backend_url::BACKEND_URL
        .get()
        .map(|s| s.as_str())
        .unwrap_or("");
    let encoded_project = urlencoding::encode(project_name);
    let encoded_image = urlencoding::encode(image_name);
    let url = format!("{prefix}/api/projects/{encoded_project}/images/{encoded_image}/bytes");

    let opts = web_sys::RequestInit::new();
    opts.set_method("HEAD");

    let request = web_sys::Request::new_with_str_and_init(&url, &opts)
        .map_err(|e| format!("Failed to create HEAD request: {e:?}"))?;

    let response_promise: Promise = window.fetch_with_request(&request);
    let response_val = JsFuture::from(response_promise)
        .await
        .map_err(|e| format!("HEAD request failed: {e:?}"))?;

    let response: web_sys::Response = response_val
        .dyn_into()
        .map_err(|_| "Response is not a Response object".to_string())?;

    if !response.ok() {
        return Err(format!("HEAD returned HTTP {}", response.status()));
    }

    response
        .headers()
        .get("Content-Length")
        .map_err(|e| format!("Failed to get Content-Length header: {e:?}"))?
        .ok_or_else(|| "No Content-Length header in response".to_string())?
        .parse::<u64>()
        .map_err(|e| format!("Invalid Content-Length: {e}"))
}

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

// ---------------------------------------------------------------------------
// Async fetch helpers — extracted so the blob‑cache `use_effect` can branch
// cleanly between gallery mode (full bytes) and list mode (sizes only).
// ---------------------------------------------------------------------------

/// Fetch full image bytes with bounded concurrency, populating `img_cache`.
/// This is the gallery‑mode code path (the original behaviour).
async fn fetch_all_bytes(
    needs_fetch: Vec<String>,
    project: String,
    version: u64,
    cancelled: Signal<bool>,
    list_version: Signal<u64>,
    mut img_cache: Signal<HashMap<String, (String, usize)>>,
    #[cfg(target_arch = "wasm32")] abort_signal: Option<web_sys::AbortSignal>,
) {
    use futures::stream::FuturesUnordered;
    use futures::StreamExt as _;

    let mut results: Vec<(String, Result<Vec<u8>, String>)> = Vec::with_capacity(needs_fetch.len());
    let mut in_flight: FuturesUnordered<
        Pin<Box<dyn Future<Output = (String, Result<Vec<u8>, String>)>>>,
    > = FuturesUnordered::new();
    let mut iter = needs_fetch.into_iter();

    #[cfg(target_arch = "wasm32")]
    let signal = abort_signal;

    macro_rules! push_fetch {
        ($in_flight:expr, $project:expr, $name:expr $(,)?) => {{
            let p = $project;
            let n = $name;
            #[cfg(target_arch = "wasm32")]
            let sig = signal.clone();
            $in_flight.push(Box::pin(async move {
                let result = {
                    #[cfg(target_arch = "wasm32")]
                    {
                        fetch_image_bytes_impl(
                            &p,
                            &n,
                            sig.as_ref().expect("AbortSignal must exist on WASM"),
                        )
                        .await
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        fetch_image_bytes_impl(&p, &n).await
                    }
                };
                (n, result)
            }) as _)
        }};
    }

    for _ in 0..MAX_CONCURRENT_FETCHES {
        if let Some(name) = iter.next() {
            push_fetch!(in_flight, project.clone(), name);
        }
    }

    while let Some((name, result)) = in_flight.next().await {
        if cancelled() || list_version() != version {
            return;
        }
        results.push((name, result));
        if let Some(name) = iter.next() {
            push_fetch!(in_flight, project.clone(), name);
        }
    }

    for (name, result) in results {
        if cancelled() || list_version() != version {
            break;
        }
        match result {
            Ok(bytes) => {
                let size = bytes.len();
                let url = bytes_to_display_url(&bytes);
                img_cache.write().insert(name, (url, size));
            }
            Err(e) => {
                error!(image_name = %name, error = %e, "Failed to load image bytes");
            }
        }
    }
}

/// Fetch only file sizes via HEAD requests (list mode).
/// On WASM this issues cheap HEAD requests that return Content‑Length without
/// any image body data.  On non‑WASM (desktop / Android) we use the full
/// server function and discard the body — the data is local so bandwidth
/// is not a concern.
async fn fetch_all_sizes(
    needs_fetch: Vec<String>,
    _project: String,
    version: u64,
    cancelled: Signal<bool>,
    list_version: Signal<u64>,
    mut file_sizes: Signal<HashMap<String, u64>>,
) {
    for name in &needs_fetch {
        if cancelled() || list_version() != version {
            break;
        }

        let size = {
            #[cfg(target_arch = "wasm32")]
            {
                // HEAD request — downloads headers only, zero body bytes.
                fetch_image_size_wasm(&_project, name).await.ok()
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let size = {
                    #[cfg(feature = "demo")]
                    {
                        // Demo mode (non-WASM): use embedded bytes directly — no HTTP server.
                        crate::demo::demo_image_bytes(name).map(|b| b.len() as u64)
                    }
                    #[cfg(not(feature = "demo"))]
                    {
                        // HEAD request via JS eval — uses the HTTP endpoint with
                        // `get_server_url()`, avoiding the download of full bytes.
                        match fetch_image_size_eval(&_project, name).await {
                            Ok(size) => Some(size),
                            Err(e) => {
                                tracing::debug!(error = %e, "Failed to fetch image size via HEAD");
                                None
                            }
                        }
                    }
                };
                size
            }
        };

        if let Some(s) = size {
            file_sizes.write().insert(name.clone(), s);
        }
    }
}

/// Fetch video file sizes via HEAD requests (list mode).
/// Same approach as `fetch_all_sizes` but uses the video bytes endpoint.
async fn fetch_all_video_sizes(
    video_names: Vec<String>,
    _project: String,
    version: u64,
    cancelled: Signal<bool>,
    list_version: Signal<u64>,
    mut file_sizes: Signal<HashMap<String, u64>>,
) {
    for name in &video_names {
        if cancelled() || list_version() != version {
            break;
        }

        let size = {
            #[cfg(target_arch = "wasm32")]
            {
                fetch_media_size_wasm(&_project, name, "videos").await.ok()
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let size = {
                    #[cfg(feature = "demo")]
                    {
                        // Demo mode: no videos, return None.
                        None::<u64>
                    }
                    #[cfg(not(feature = "demo"))]
                    {
                        match fetch_media_size_eval(&_project, name, "videos").await {
                            Ok(size) => Some(size),
                            Err(e) => {
                                tracing::debug!(error = %e, "Failed to fetch video size via HEAD");
                                None
                            }
                        }
                    }
                };
                size
            }
        };

        if let Some(s) = size {
            file_sizes.write().insert(name.clone(), s);
        }
    }
}

/// Fetch a single image's bytes and store them in `img_cache`.
/// Used by the fullscreen viewer when the image isn't cached yet
/// (e.g. in list mode where we only fetch sizes).
async fn fetch_and_cache_single(
    name: String,
    project: String,
    mut img_cache: Signal<HashMap<String, (String, usize)>>,
) {
    let result = {
        #[cfg(target_arch = "wasm32")]
        {
            // On WASM we use the direct-fetch helper (no AbortSignal — the
            // user consciously opened fullscreen so cancellation isn't needed).
            fetch_image_bytes_wasm_fallback(&project, &name).await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Fetch via HTTP from JS eval with `get_server_url()`.
            fetch_image_bytes_eval(&project, &name).await
        }
    };

    if let Ok(data) = result {
        let size = data.len();
        let url = bytes_to_display_url(&data);
        img_cache.write().insert(name, (url, size));
    }
}

/// WASM-only fallback: fetch image bytes via direct `fetch` without an
/// AbortController (used by the fullscreen on‑demand fetch where
/// cancellation does not apply).
#[cfg(target_arch = "wasm32")]
async fn fetch_image_bytes_wasm_fallback(
    project_name: &str,
    image_name: &str,
) -> Result<Vec<u8>, String> {
    // On WASM we bypass the Dioxus server function and issue a direct
    // GET request so we don't need `backend` (which doesn't exist on WASM).

    let window = web_sys::window().ok_or("No window available")?;
    let prefix = crate::backend_url::BACKEND_URL
        .get()
        .map(|s| s.as_str())
        .unwrap_or("");
    let encoded_project = urlencoding::encode(project_name);
    let encoded_image = urlencoding::encode(image_name);
    let url = format!("{prefix}/api/projects/{encoded_project}/images/{encoded_image}/bytes");

    let opts = web_sys::RequestInit::new();
    opts.set_method("GET");

    let request = web_sys::Request::new_with_str_and_init(&url, &opts)
        .map_err(|e| format!("Failed to create request: {e:?}"))?;

    let response_val = wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("Fetch failed: {e:?}"))?;

    let response: web_sys::Response = response_val.into();

    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }

    let buffer_val = wasm_bindgen_futures::JsFuture::from(
        response
            .array_buffer()
            .map_err(|e| format!("Failed to get array buffer: {e:?}"))?,
    )
    .await
    .map_err(|e| format!("Failed to read array buffer: {e:?}"))?;

    let uint8 = js_sys::Uint8Array::new(&buffer_val);
    let mut bytes = vec![0u8; uint8.length() as usize];
    uint8.copy_to(&mut bytes);
    Ok(bytes)
}

#[component]
pub fn GalleryTab(project_name: String) -> Element {
    let project_name_clone = project_name.clone();
    use_effect(move || debug!(project_name = %project_name_clone, "Initializing images tab"));
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
    let mut selected_videos = use_signal(Vec::<String>::new);
    let mut demo_loading = use_signal(|| false);
    let mut demo_dialog_open = use_signal(|| false);
    let mut resize_loading = use_signal(|| false);
    let mut resize_dialog_open = use_signal(|| false);
    let mut resize_max_dimension = use_signal(|| 1024u32);
    let mut error_message = use_signal::<Option<String>>(|| None);
    let mut info_message = use_signal::<Option<String>>(|| None);
    let mut fullscreen_image = use_signal::<Option<String>>(|| None);
    let mut uploading = use_signal(|| false);
    let mut show_images = use_signal(|| cfg!(feature = "demo"));

    // Video state
    let mut video_paths = use_signal(Vec::<String>::new);
    let mut video_uploading = use_signal(|| false);

    // Size-only cache for list mode (populated via HEAD requests on WASM
    // so we never download full image bytes just to show a file size).
    let file_sizes: Signal<HashMap<String, u64>> = use_signal(HashMap::new);

    // ── Route-change cancellation ──────────────────────────────────────────
    // Set to `true` when the component unmounts (user navigates away).  All
    // spawned tasks check this flag before starting new work or processing
    // results, preventing stale signal writes after the UI is gone.
    let cancelled = use_signal(|| false);

    // WASM: `AbortController` that aborts every in-flight `fetch` for image
    // bytes the instant the component unmounts.  This keeps the browser's
    // connection pool free for other server-function calls.
    #[cfg(target_arch = "wasm32")]
    let mut abort_controller: Signal<Option<web_sys::AbortController>> = use_signal(|| None);

    // Track task IDs so we can cancel demo downloads / resize tasks on unmount.
    let demo_task_id: Signal<Option<String>> = use_signal(|| None);
    let resize_task_id: Signal<Option<String>> = use_signal(|| None);

    // ── Unmount cleanup (runs when component is removed from the tree) ───────
    // `use_drop` is a Dioxus 0.7 hook that schedules a closure to run when
    // the component unmounts (route change away from the Images tab).
    {
        let mut cancelled = cancelled;
        #[cfg(target_arch = "wasm32")]
        let mut ac = abort_controller;
        let mut demo_id = demo_task_id;
        let mut resize_id = resize_task_id;
        use_drop(move || {
            // 1. Prevent any further processing in spawned tasks.
            cancelled.set(true);

            // 2. Abort all in-flight HTTP requests for image bytes.
            #[cfg(target_arch = "wasm32")]
            if let Some(ctrl) = ac.write().take() {
                tracing::debug!("ImagesTab unmount: aborting in-flight image byte fetches");
                ctrl.abort();
            }

            // 3. Cancel any running server-side tasks (demo download, resize).
            if let Some(id) = demo_id.write().take() {
                tracing::debug!(task_id = %id, "ImagesTab unmount: cancelling demo download");
                spawn(async move {
                    let _ = crate::server::cancel_task(id).await;
                });
            }
            if let Some(id) = resize_id.write().take() {
                tracing::debug!(task_id = %id, "ImagesTab unmount: cancelling resize task");
                spawn(async move {
                    let _ = crate::server::cancel_task(id).await;
                });
            }
        });
    }

    // ── Load video list on mount ─────────────────────────────────────
    let project_name_videos = project_name.clone();
    use_effect(move || {
        let project_name = project_name_videos.clone();
        spawn(async move {
            match crate::server::get_project_videos(project_name.clone()).await {
                Ok(vids) => {
                    debug!(project_name = %project_name, video_count = vids.len(), "Loaded project videos");
                    video_paths.set(vids);
                }
                Err(e) => {
                    warn!(project_name = %project_name, error = %e, "Failed to load project videos");
                }
            }
        });
    });

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

    // ── Blob-URL cache management (with cancellation support) ─────────────
    // Incrementally updates the cache: removes entries for deleted images,
    // fetches display URLs only for newly added ones, and leaves existing
    // entries untouched.  Callers that modify byte content (e.g. resize)
    // must clear `img_cache` before bumping `image_paths` to force a re-fetch.
    let project_name_cache = project_name.clone();
    use_effect(move || {
        // ── 1. Skip if unmounted ────────────────────────────────────────
        if cancelled() {
            return;
        }

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

        // ── 2. Create a fresh AbortController for this batch (WASM only) ─
        #[cfg(target_arch = "wasm32")]
        let abort_signal: Option<web_sys::AbortSignal> = {
            // Abort any controller from a previous batch.
            if let Some(old) = abort_controller.write().take() {
                old.abort();
            }
            let ctrl = web_sys::AbortController::new().ok();
            let sig = ctrl.as_ref().map(|c| c.signal());
            *abort_controller.write() = ctrl;
            sig
        };

        // ── 3. Check view mode ────────────────────────────────────────
        // In gallery mode we fetch full image bytes as before.  In list mode
        // we only fetch file sizes (via HEAD on WASM) so we never download
        // multi-GB image data just to show a size column.
        if show_images() {
            // ── Gallery mode: fetch full image bytes ────────────────────
            if !needs_fetch.is_empty() {
                let project = project_name_cache.clone();
                spawn(fetch_all_bytes(
                    needs_fetch,
                    project,
                    version,
                    cancelled,
                    list_version,
                    img_cache,
                    #[cfg(target_arch = "wasm32")]
                    abort_signal,
                ));
            }
        } else {
            // ── List mode: fetch sizes for images and videos ────────────
            let project = project_name_cache.clone();
            if !needs_fetch.is_empty() {
                spawn(fetch_all_sizes(
                    needs_fetch,
                    project.clone(),
                    version,
                    cancelled,
                    list_version,
                    file_sizes,
                ));
            }
            let vids = video_paths();
            if !vids.is_empty() {
                spawn(fetch_all_video_sizes(
                    vids,
                    project,
                    version,
                    cancelled,
                    list_version,
                    file_sizes,
                ));
            }
        }
    });

    let on_delete_selected = {
        let project_name = project_name.clone();
        move |_| {
            let project_name = project_name.clone();
            let to_delete_images: Vec<String> = selected_images().clone();
            let to_delete_videos: Vec<String> = selected_videos().clone();
            spawn(async move {
                let mut deleted_any = false;
                for image_name in to_delete_images {
                    if crate::server::delete_project_image(project_name.clone(), image_name)
                        .await
                        .is_ok()
                    {
                        deleted_any = true;
                    }
                }
                for video_name in to_delete_videos {
                    if crate::server::delete_project_video(project_name.clone(), video_name)
                        .await
                        .is_ok()
                    {
                        deleted_any = true;
                    }
                }
                if deleted_any {
                    match crate::server::get_project_images(project_name.clone()).await {
                        Ok(imgs) => {
                            image_paths.set(imgs);
                            list_version += 1;
                        }
                        Err(e) => {
                            error_message.set(Some(format!("Failed to reload images: {}", e)));
                        }
                    }
                    match crate::server::get_project_videos(project_name.clone()).await {
                        Ok(vids) => {
                            video_paths.set(vids);
                        }
                        Err(e) => {
                            warn!(project_name = %project_name, error = %e, "Failed to reload videos after delete");
                        }
                    }
                    info_message.set(Some("Selected items deleted successfully.".to_string()));
                }
            });
            selected_images.set(Vec::new());
            selected_videos.set(Vec::new());
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
            let mut demo_id = demo_task_id;
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
                // Store so we can cancel this task on unmount.
                demo_id.set(Some(task_id.clone()));
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
            let mut resize_id = resize_task_id;
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
                // Store so we can cancel this task on unmount.
                resize_id.set(Some(task_id.clone()));
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

    let mut selected_videos2 = selected_videos;
    let mut toggle_select_video = move |video_name: String| {
        let mut selected = selected_videos();
        if selected.contains(&video_name) {
            selected.retain(|x| x != &video_name);
        } else {
            selected.push(video_name);
        }
        selected_videos2.set(selected);
    };

    let select_all = move |_| {
        let img_paths = image_paths();
        let vid_paths = video_paths();
        let all_paths: Vec<String> = img_paths.iter().chain(vid_paths.iter()).cloned().collect();
        let total_count = all_paths.len();
        let selected_count = selected_images().len() + selected_videos().len();
        if selected_count == total_count && total_count > 0 {
            selected_images.set(Vec::new());
            selected_videos.set(Vec::new());
        } else {
            selected_images.set(img_paths);
            selected_videos.set(vid_paths);
        }
    };

    let has_images = !image_paths().is_empty();
    let has_videos = !video_paths().is_empty();
    let has_media = has_images || has_videos;
    let has_selection = !selected_images().is_empty() || !selected_videos().is_empty();
    let all_selected = (selected_images().len() + selected_videos().len())
        == (image_paths().len() + video_paths().len())
        && has_media;
    let num_images = image_paths().len();
    let num_selected = selected_images().len() + selected_videos().len();
    let num_videos = video_paths().len();

    // Pre-compute video streaming URLs for the gallery.
    let video_urls: HashMap<String, String> = if has_videos {
        let base = crate::backend_url::BACKEND_URL
            .get()
            .map(|s| s.trim_end_matches('/'))
            .unwrap_or("");
        video_paths()
            .iter()
            .map(|name| {
                let url = format!(
                    "{}/api/projects/{}/videos/{}/bytes",
                    base,
                    urlencoding::encode(&project_name),
                    urlencoding::encode(name),
                );
                (name.clone(), url)
            })
            .collect()
    } else {
        HashMap::new()
    };

    // Merged sorted list of all media (images + videos) for unified gallery display.
    let all_media: Vec<(String, bool)> = {
        let imgs = image_paths();
        let vids = video_paths();
        let mut all: Vec<(String, bool)> = imgs.into_iter().map(|n| (n, false)).collect();
        all.extend(vids.into_iter().map(|n| (n, true)));
        all.sort_by(|a, b| a.0.cmp(&b.0));
        all
    };

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
                                        class: "action-btn",
                                        Icon { icon: BsBoxArrowUpRight }
                                        "Source"
                                    }
                                    button {
                                        class: "action-btn action-btn-primary",
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
                                        class: "action-btn",
                                        Icon { icon: BsBoxArrowUpRight }
                                        "Source"
                                    }
                                    button {
                                        class: "action-btn action-btn-primary",
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

            // ── Fullscreen media viewer (on‑demand fetch) ──────────────────
            // Supports both images and videos.
            {
                if let Some(fullscreen_name) = fullscreen_image() {
                    let is_video = video_paths().contains(&fullscreen_name);

                    if is_video {
                        // ── Video fullscreen ────────────────────────────────
                        let video_url = video_urls.get(&fullscreen_name).map(|s| s.as_str()).unwrap_or("");
                        let size = file_sizes().get(&fullscreen_name).copied().unwrap_or(0);
                        let size_mb = size as f64 / 1024.0 / 1024.0;
                        let cap = if size > 0 {
                            format!("{} \u{00b7} {:.2} MB", &fullscreen_name, size_mb)
                        } else {
                            fullscreen_name.clone()
                        };
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
                                        "\u{00d7}"
                                    }

                                    div {
                                        class: "fullscreen-caption",
                                        "{cap}"
                                    }
                                    video {
                                        src: "{video_url}",
                                        preload: "auto",
                                        controls: true,
                                        class: "fullscreen-video",
                                        autoplay: true,
                                    }
                                }
                            }
                        }
                    } else {
                        // ── Image fullscreen (on‑demand fetch) ──────────────
                        // When the user opens fullscreen for an image that isn't cached
                        // (e.g. in list mode where we only fetch sizes), we fetch its bytes
                        // on demand and store them in `img_cache`.
                        let cache_entry = img_cache().get(&fullscreen_name).cloned();
                        let (full_image_url, size_bytes) = cache_entry.unwrap_or_else(|| (String::new(), 0));

                        let img_id = format!("fullscreen-img-{}", fullscreen_name);
                        let metadata_id = format!("metadata-fullscreen-{}", fullscreen_name);
                        let img_id_onload = img_id.clone();
                        let metadata_id_onload = metadata_id.clone();
                        let fname_onload = fullscreen_name.clone();
                        let cached_size = size_bytes;

                        // Trigger fetch for uncached images.
                        let fullscreen_img_cache = img_cache;
                        let fullscreen_fetch_project = project_name.clone();
                        let fname_for_fetch = fullscreen_name.clone();
                        let _ = use_resource(move || {
                            let name = fname_for_fetch.clone();
                            let project = fullscreen_fetch_project.clone();
                            async move {
                                if !fullscreen_img_cache.read().contains_key(&name) {
                                    fetch_and_cache_single(
                                        name.clone(),
                                        project.clone(),
                                        fullscreen_img_cache,
                                    )
                                    .await;
                                }
                            }
                        });

                        let size_mb = cached_size as f64 / 1024.0 / 1024.0;
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
                    }
                } else {
                    rsx! {}
                }
            }

            div {
                class: "images-toolbar",

                // ── First group: upload and demo actions ────────────────
                div {
                    class: "toolbar-group",

                    button {
                        class: "action-btn action-btn-primary",
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
                            class: "btn-label",
                            if uploading() { "Uploading..." } else { "Upload Image" }
                        }
                    }

                    // ── Upload Video (direct stream, avoids WASM memory) ──
                    button {
                        class: "action-btn action-btn-primary",
                        title: if video_uploading() { "Uploading..." } else { "Upload videos from disk" },
                        disabled: video_uploading() || uploading() || demo_loading(),
                        onclick: {
                            let project_name = project_name.clone();
                            move |_| {
                                video_uploading.set(true);
                                error_message.set(None);
                                let pn = project_name.clone();
                                spawn(async move {
                                    let base = crate::backend_url::BACKEND_URL
                                        .get()
                                        .map(|s| s.trim_end_matches('/'))
                                        .unwrap_or("");
                                    let upload_prefix =
                                        format!("{}/api/projects/{}/videos/",
                                            base,
                                            urlencoding::encode(&pn));
                                    let (uploaded, upload_errors) = crate::picker::upload_files_direct(
                                        Some("video/*,.mp4,.webm,.mkv,.avi,.mov,.m4v,.mpg,.mpeg,.wmv,.flv,.3gp"),
                                        true,
                                        &upload_prefix,
                                    ).await;

                                    let count = uploaded.len();
                                    if count > 0 {
                                        let mut msg = format!("Uploaded {} video(s). Videos are kept in the videos/ folder and will be automatically processed when the pipeline runs.", count);
                                        if !upload_errors.is_empty() {
                                            msg.push_str(&format!(" {} upload(s) failed.", upload_errors.len()));
                                            error_message.set(Some(upload_errors.join("; ")));
                                        }
                                        info_message.set(Some(msg));
                                        if let Ok(vids) = crate::server::get_project_videos(pn).await {
                                            video_paths.set(vids);
                                        }
                                    } else {
                                        if !upload_errors.is_empty() {
                                            error_message.set(Some(format!("Video upload failed: {}", upload_errors.join("; "))));
                                        } else {
                                            info_message.set(Some("No videos were uploaded.".to_string()));
                                        }
                                    }

                                    video_uploading.set(false);
                                });
                            }
                        },
                        Icon { icon: BsCameraVideo }
                        span {
                            class: "btn-label",
                            if video_uploading() { "Uploading..." } else { "Upload Video" }
                        }
                    }

                    button {
                        class: "action-btn action-btn-primary",
                        onclick: move |_| demo_dialog_open.set(true),
                        disabled: demo_loading() || uploading(),
                        title: "Choose and download demo images from bundler_sfm examples",
                        Icon { icon: BsStar }
                        span {
                            class: "btn-label",
                            if demo_loading() {
                                "Downloading…"
                            } else {
                                "Demo Images"
                            }
                        }
                    }

                    if has_images {
                        button {
                            class: "action-btn",
                            onclick: on_open_resize_dialog,
                            disabled: resize_loading() || uploading() || demo_loading(),
                            title: "Resize ALL images by resizing to a maximum dimension",
                            Icon { icon: BsTextareaResize }
                            span {
                                class: "btn-label",
                                if resize_loading() {
                                    "Resizing..."
                                } else {
                                    "Resize Images"
                                }
                            }
                        }
                    }
                }

                // ── Second group: video info and management ──────────────
                div {
                    class: "toolbar-group",

                    if has_videos {
                        div {
                            class: "action-btn images-info",
                            "{num_videos} " Icon { icon: BsCameraVideo }
                        }
                    }

                    if has_videos {
                        button {
                            class: "action-btn action-btn-danger",
                            onclick: {
                                let project_name = project_name.clone();
                                move |_| {
                                    let project_name = project_name.clone();
                                    spawn(async move {
                                        match crate::server::clear_project_videos(project_name).await {
                                            Ok(_) => {
                                                video_paths.set(Vec::new());
                                                info_message.set(Some("All videos cleared successfully".to_string()));
                                            }
                                            Err(e) => {
                                                error_message.set(Some(format!("Failed to clear videos: {}", e)));
                                            }
                                        }
                                    });
                                }
                            },
                            title: "Delete all videos",
                            Icon { icon: BsXCircle }
                            span { class: "btn-label", "Clear Videos" }
                        }
                    }
                }

                // ── Third group: view toggle and selection actions ───────
                div {
                    class: "toolbar-group",

                    button {
                        class: "action-btn",
                        onclick: move |_| show_images.set(!show_images()),
                        title: if show_images() { "Switch to compact list view" } else { "Switch to image gallery" },
                        if show_images() {
                            Icon { icon: BsViewList }
                            span { class: "btn-label", "List" }
                        } else {
                            Icon { icon: BsGrid }
                            span { class: "btn-label", "Gallery" }
                        }
                    }

                    if has_images || has_videos {
                        button {
                            class: "action-btn",
                            onclick: select_all,
                            title: if all_selected { "Deselect all" } else { "Select all" },
                            Icon { icon: BsCheckAll }
                            span { class: "btn-label",
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
                            class: "action-btn images-info",
                            "{num_images} " Icon { icon: BsImage }
                        }
                    }

                    if has_selection {
                        button {
                            class: "action-btn action-btn-danger",
                            onclick: on_delete_selected,
                            title: "Delete selected items",
                            Icon { icon: BsTrash3 }
                            span { class: "btn-label", "Delete ({num_selected})" }
                        }
                    }

                    if has_images {
                        button {
                            class: "action-btn action-btn-danger",
                            onclick: on_clear_all,
                            title: "Delete all images",
                            Icon { icon: BsXCircle }
                            span { class: "btn-label", "Clear All" }
                        }
                    }
                }
            }

            if has_media {
                if show_images() {
                    div {
                        class: "image-gallery",
                        {
                            let selected = selected_images();
                            let cache = img_cache();
                            let urls = &video_urls;
                            let mut elements = Vec::new();
                            for (media_name, is_video) in all_media.iter() {
                                if *is_video {
                                    // ── Video item ────────────────────────────
                                    let url = urls.get(media_name).map(|s| s.as_str()).unwrap_or("");
                                    let vname_clone = media_name.clone();
                                    let vname_checkbox = media_name.clone();
                                    let is_vid_selected = selected_videos().contains(media_name);
                                    elements.push(rsx! {
                                        div {
                                            key: "{media_name}",
                                            class: if is_vid_selected { "image-item selected" } else { "image-item" },

                                            div {
                                                class: "image-checkbox",
                                                input {
                                                    r#type: "checkbox",
                                                    checked: is_vid_selected,
                                                    onchange: move |_| toggle_select_video(vname_checkbox.clone()),
                                                    id: format!("video-checkbox-{}", urlencoding::encode(media_name)),
                                                }
                                            }

                                            button {
                                                class: "image-fullscreen-btn",
                                                title: "View fullscreen",
                                                onclick: move |_| fullscreen_image.set(Some(vname_clone.clone())),
                                                Icon { icon: BsArrowsFullscreen }
                                            }

                                            video {
                                                src: "{url}",
                                                preload: "metadata",
                                                controls: true,
                                                class: "thumbnail video-player",
                                            }

                                            div {
                                                class: "image-name",
                                                "{media_name}"
                                            }
                                        }
                                    });
                                } else {
                                    // ── Image item ────────────────────────────
                                    let is_selected = selected.contains(media_name);
                                    let Some((image_url, size_bytes)) = cache.get(media_name).cloned()
                                        else { continue; };
                                    let size_mb = size_bytes as f64 / 1024.0 / 1024.0;
                                    let safe_image_name = urlencoding::encode(media_name);
                                    let img_id = format!("thumbnail-{}", safe_image_name);
                                    let metadata_id = format!("metadata-{}", safe_image_name);
                                    let image_name_for_checkbox = media_name.clone();
                                    let image_name_for_fullscreen = media_name.clone();
                                    let image_name_for_img = media_name.clone();
                                    elements.push(rsx! {
                                        div {
                                            key: "{media_name}",
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
                                                    "{media_name}"
                                                }
                                            }

                                            img {
                                                src: image_url.clone(),
                                                alt: media_name.clone(),
                                                id: img_id.clone(),
                                                onclick: move |_| toggle_select(image_name_for_img.clone()),
                                                class: "thumbnail",
                                                onload: move |_| {
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
                            }
                            rsx! { for element in elements { {element} } }
                        }
                    }
                } else {
                    div {
                        class: "image-list-compact",
                        for (media_name, is_video) in all_media.iter().map(|(n, v)| (n.clone(), *v)) {
                            if is_video {
                                div {
                                    key: "{media_name}",
                                    class: if selected_videos().contains(&media_name) { "image-list-item selected" } else { "image-list-item" },
                                    input {
                                        r#type: "checkbox",
                                        checked: selected_videos().contains(&media_name),
                                        onchange: {
                                            let n = media_name.clone();
                                            move |_| toggle_select_video(n.clone())
                                        },
                                    }
                                    span {
                                        class: "item-name video-name",
                                        Icon { icon: BsCameraVideo }
                                        " {media_name}"
                                    }
                                    button {
                                        class: "list-fullscreen-btn",
                                        title: "View fullscreen",
                                        onclick: {
                                            let n = media_name.clone();
                                            move |_| fullscreen_image.set(Some(n.clone()))
                                        },
                                        Icon { icon: BsArrowsFullscreen }
                                    }
                                    span { class: "item-size",
                                        {
                                            let sz = file_sizes().get(&media_name).copied();
                                            match sz {
                                                Some(s) => format!("{:.2} MB", s as f64 / 1048576.0),
                                                None => String::new(),
                                            }
                                        }
                                    }
                                }
                            } else {
                                div {
                                    key: "{media_name}",
                                    class: if selected_images().contains(&media_name) { "image-list-item selected" } else { "image-list-item" },
                                    input {
                                        r#type: "checkbox",
                                        checked: selected_images().contains(&media_name),
                                        onchange: {
                                            let n = media_name.clone();
                                            move |_| toggle_select(n.clone())
                                        },
                                    }
                                    span {
                                        class: "item-name",
                                        onclick: {
                                            let n = media_name.clone();
                                            move |_| fullscreen_image.set(Some(n.clone()))
                                        },
                                        "{media_name}"
                                    }
                                    button {
                                        class: "list-fullscreen-btn",
                                        title: "View fullscreen",
                                        onclick: {
                                            let n = media_name.clone();
                                            move |_| fullscreen_image.set(Some(n.clone()))
                                        },
                                        Icon { icon: BsArrowsFullscreen }
                                    }
                                    span { class: "item-size",
                                        {
                                            let sz_from_size_cache =
                                                file_sizes().get(&media_name).copied();
                                            let sz_from_img_cache =
                                                img_cache().get(&media_name).map(|(_, s)| *s as u64);
                                            let sz = sz_from_size_cache.or(sz_from_img_cache);
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
                }

            } else {
                div {
                    class: "empty-gallery",
                    Icon { icon: BsImage, class: "empty-icon" }
                    p { "No images in this project" }
                    p {
                        class: "hint",
                        "Upload images, capture photos, upload videos, or download demo images to get started."
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
