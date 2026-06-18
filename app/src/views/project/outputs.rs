use crate::mycomponents::{add_toast, remove_toast, update_toast, ToastType};
use crate::server::{
    clear_project_outputs, delete_project_output, download_project_outputs_zip,
    get_project_output_bytes, get_project_output_glb, list_project_outputs,
    restore_project_outputs,
};
use base64::Engine as _;
use colmap_openmvs_api::OutputFile;
use dioxus::document::eval;
use dioxus::prelude::*;
use dioxus_free_icons::icons::bs_icons::{
    BsArrowRepeat, BsCheck2, BsChevronDown, BsChevronRight, BsDownload, BsEye, BsHourglass,
    BsTrash3, BsUpload, BsX,
};
use dioxus_free_icons::Icon;
use std::collections::HashSet;
use tracing::{debug, error, info};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Try to save `data` to a known public download directory (SD card on Android)
/// and return the path where it was saved.
///
/// On non-Android targets this always returns `None` so callers fall through
/// to the base64 + eval `<a>`-tag download path.
fn save_bytes_to_downloads(data: &[u8], filename: &str) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "android")]
    {
        let candidates = [
            "/storage/emulated/0/Download",
            "/sdcard/Download",
            "/storage/emulated/0",
            "/sdcard",
        ];
        for base in &candidates {
            let dir = std::path::Path::new(base);
            if dir.exists() && dir.is_dir() {
                let path = dir.join(filename);
                // Avoid overwriting by appending a suffix if the file exists.
                let path = if path.exists() {
                    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    let mut counter = 1u32;
                    loop {
                        let candidate = if ext.is_empty() {
                            dir.join(format!("{}_{}", stem, counter))
                        } else {
                            dir.join(format!("{}_{}.{}", stem, counter, ext))
                        };
                        if !candidate.exists() {
                            break candidate;
                        }
                        counter += 1;
                    }
                } else {
                    path
                };
                match std::fs::write(&path, data) {
                    Ok(()) => {
                        tracing::info!(saved_to = %path.display(), "Download saved to SD card");
                        return Some(path);
                    }
                    Err(e) => {
                        tracing::debug!(
                            path = %path.display(),
                            error = %e,
                            "Cannot write to candidate download dir, trying next"
                        );
                    }
                }
            }
        }
        tracing::warn!("No writable download directory found on Android");
        None
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (data, filename);
        None
    }
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Returns a short human-readable relative age string, or empty string if unknown.
fn format_date(unix_millis: u64) -> String {
    if unix_millis == 0 {
        return String::new();
    }
    let now = chrono::Utc::now().timestamp() as u64;
    let diff = now.saturating_sub(unix_millis / 1000);
    if diff < 60 {
        "just now".to_string()
    } else if diff < 3_600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3_600)
    } else if diff < 86_400 * 30 {
        format!("{}d ago", diff / 86_400)
    } else if diff < 86_400 * 365 {
        format!("{}mo ago", diff / (86_400 * 30))
    } else {
        format!("{}y ago", diff / (86_400 * 365))
    }
}

/// Trigger a browser download of a file by fetching from a server URL directly.
///
/// On non-demo builds this avoids copying large files through the Rust ↔ JS
/// bridge: JS fetches the bytes directly as a Blob and triggers the save-as
/// dialog. On demo builds we fall back to the old base64 approach since no
/// real HTTP endpoint exists.
#[cfg(not(feature = "demo"))]
async fn trigger_download_from_url(
    project_name: &str,
    relative_path: &str,
    filename: &str,
    mime_type: &str,
    api_endpoint: &str, // e.g. "/api/projects/{name}/outputs/bytes" or "glb"
    mut toast_ctx: crate::mycomponents::ToastCtx,
) {
    use dioxus::document::eval as eval_fn;
    let name_esc = js_escape(filename);
    let project_esc = js_escape(project_name);
    let path_esc = js_escape(relative_path);

    // Show progress toast while the download is being prepared
    let progress_id = add_toast(
        &mut toast_ctx,
        format!("Downloading {filename}"),
        ToastType::Info,
        None,
    );

    // Use get_server_url() as base so JS fetches via the correct server
    // origin — works in web mode (equals window.location.origin) and
    // desktop mode (embedded server on localhost:port).
    // On Android/Tauri where get_server_url() may be empty and
    // window.location.origin is a non-HTTP protocol (dioxus://, tauri://),
    // fall back to the default localhost address.
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

    let js = format!(
        r#"(async function() {{
    const baseUrl = '{server_url_esc}' || (/^https?:/.test(window.location.origin) ? window.location.origin : 'http://localhost:8080');
    const url = baseUrl + '{api_endpoint}'.replace('{{name}}', encodeURIComponent('{project_esc}')) + '?relative_path=' + encodeURIComponent('{path_esc}');
    try {{
        const resp = await fetch(url);
        if (!resp.ok) throw new Error('HTTP ' + resp.status);
        const blob = await resp.blob();
        const a = document.createElement('a');
        a.href = URL.createObjectURL(blob);
        a.download = '{name_esc}';
        document.body.appendChild(a);
        a.click();
        setTimeout(() => {{ document.body.removeChild(a); URL.revokeObjectURL(a.href); }}, 10000);
        dioxus.send('ok');
    }} catch(e) {{
        dioxus.send('error:' + e.message);
    }}
}})();"#
    );

    let mut handle = eval_fn(&js);
    if let Ok(msg) = handle.recv::<String>().await {
        if let Some(err) = msg.strip_prefix("error:") {
            tracing::warn!(
                "JS HTTP download failed ({}), falling back to Rust server function — performance may be reduced",
                err
            );
            // Fallback: fetch via Rust, base64-encode, and trigger download via eval.
            let stream_result = if api_endpoint.contains("/outputs/glb") {
                get_project_output_glb(project_name.to_string(), relative_path.to_string()).await
            } else {
                get_project_output_bytes(project_name.to_string(), relative_path.to_string()).await
            };

            match stream_result {
                Ok(mut stream) => {
                    let mut bytes = Vec::new();
                    while let Some(chunk) = stream.next().await {
                        match chunk {
                            Ok(data) => bytes.extend_from_slice(&data),
                            Err(e) => {
                                error!("Fallback download stream error: {e}");
                                remove_toast(&mut toast_ctx, &progress_id);
                                add_toast(
                                    &mut toast_ctx,
                                    format!("Download failed: {e}"),
                                    ToastType::Error,
                                    None,
                                );
                                return;
                            }
                        }
                    }
                    // On Android, write to the SD card Download folder directly
                    // (the `<a>` download attribute does not work in WebViews).
                    // On other platforms, use the base64 + eval approach.
                    if let Some(saved_path) = save_bytes_to_downloads(&bytes, filename) {
                        tracing::info!(saved_to = %saved_path.display(), "Download saved locally");
                        add_toast(
                            &mut toast_ctx,
                            format!("Downloaded {filename} to {}", saved_path.display()),
                            ToastType::Info,
                            None,
                        );
                        remove_toast(&mut toast_ctx, &progress_id);
                        return;
                    } else {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        let b64_esc = js_escape(&b64);
                        let mime_esc = js_escape(mime_type);
                        let js_fb = format!(
                            r#"const b64 = '{b64_esc}';
const blob = new Blob([Uint8Array.from(atob(b64), c => c.charCodeAt(0))], {{type: '{mime_esc}'}});
const a = document.createElement('a');
a.href = URL.createObjectURL(blob);
a.download = '{name_esc}';
document.body.appendChild(a);
a.click();
setTimeout(() => {{ document.body.removeChild(a); URL.revokeObjectURL(a.href); }}, 10000);"#
                        );
                        let _ = eval_fn(&js_fb).await;
                    }
                }
                Err(e) => {
                    error!("Fallback server function failed: {e}");
                    remove_toast(&mut toast_ctx, &progress_id);
                    add_toast(
                        &mut toast_ctx,
                        format!("Download failed: {e}"),
                        ToastType::Error,
                        None,
                    );
                    return;
                }
            }
        }
    }
    // Success: replace progress toast with completion notification
    remove_toast(&mut toast_ctx, &progress_id);
    add_toast(
        &mut toast_ctx,
        format!("Downloaded {filename}"),
        ToastType::Info,
        None,
    );
}

/// Escape a string for embedding inside a JS single-quoted string literal.
fn js_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Pick an emoji icon based on file extension.
fn file_icon(name: &str, is_dir: bool, is_virtual_dir: bool) -> &'static str {
    if is_dir {
        return if is_virtual_dir { "🔷" } else { "📁" };
    }
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "ply" | "obj" | "stl" | "fbx" | "glb" | "gltf" => "🔷",
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tiff" | "gif" => "🖼️",
        "db" | "sqlite" | "sqlite3" => "🗄️",
        "bin" | "dat" => "💾",
        "txt" | "log" | "md" | "rst" => "📝",
        "sh" | "bash" | "zsh" | "fish" => "⚙️",
        "py" | "js" | "ts" | "rs" | "toml" | "json" | "yaml" | "yml" | "xml" => "📋",
        "zip" | "tar" | "gz" | "bz2" | "7z" | "xz" | "rar" => "📦",
        "pdf" => "📑",
        "mp4" | "avi" | "mov" | "mkv" | "webm" => "🎬",
        "csv" | "tsv" => "📊",
        _ => "📄",
    }
}

// ---------------------------------------------------------------------------
// Tree data structures + builder
// ---------------------------------------------------------------------------

struct DirNode {
    name: String,
    path: String,
    subdirs: Vec<DirNode>,
    files: Vec<OutputFile>,
}

/// A flattened display entry (both real and virtual).
#[derive(Clone, PartialEq)]
struct DisplayEntry {
    /// Display name (full relative path for virtual <models> file entries).
    name: String,
    /// Logical path used for collapse tracking and RSX key.
    relative_path: String,
    is_dir: bool,
    /// True for the synthetic <models> folder and its children.
    is_virtual: bool,
    depth: usize,
    /// `Some(…)` for file entries, always holds the *real* OutputFile.
    file: Option<OutputFile>,
    /// Recursive file count — directories only.
    file_count: usize,
    /// Recursive total size — directories; direct size — files.
    total_size: u64,
}

fn insert_recursive(node: &mut DirNode, parts: &[&str], file: OutputFile) {
    if parts.len() == 1 {
        node.files.push(file);
    } else {
        let dir_name = parts[0];
        let dir_path = if node.path.is_empty() {
            dir_name.to_string()
        } else {
            format!("{}/{}", node.path, dir_name)
        };
        if let Some(sub) = node.subdirs.iter_mut().find(|d| d.name == dir_name) {
            insert_recursive(sub, &parts[1..], file);
        } else {
            let mut new_dir = DirNode {
                name: dir_name.to_string(),
                path: dir_path,
                subdirs: vec![],
                files: vec![],
            };
            insert_recursive(&mut new_dir, &parts[1..], file);
            node.subdirs.push(new_dir);
        }
    }
}

fn count_files_in(node: &DirNode) -> usize {
    node.files.len() + node.subdirs.iter().map(count_files_in).sum::<usize>()
}

fn total_size_in(node: &DirNode) -> u64 {
    node.files.iter().map(|f| f.size).sum::<u64>()
        + node.subdirs.iter().map(total_size_in).sum::<u64>()
}

fn flatten_node(node: &DirNode, depth: usize, result: &mut Vec<DisplayEntry>) {
    let mut subdirs: Vec<&DirNode> = node.subdirs.iter().collect();
    subdirs.sort_by_key(|d| &d.name);
    for sub in subdirs {
        result.push(DisplayEntry {
            name: sub.name.clone(),
            relative_path: sub.path.clone(),
            is_dir: true,
            is_virtual: false,
            depth,
            file: None,
            file_count: count_files_in(sub),
            total_size: total_size_in(sub),
        });
        flatten_node(sub, depth + 1, result);
    }
    let mut files: Vec<&OutputFile> = node.files.iter().collect();
    files.sort_by_key(|f| &f.name);
    for f in files {
        result.push(DisplayEntry {
            name: f.name.clone(),
            relative_path: f.relative_path.clone(),
            is_dir: false,
            is_virtual: false,
            depth,
            file: Some(f.clone()),
            file_count: 0,
            total_size: f.size,
        });
    }
}

fn build_display_list(files: &[OutputFile]) -> Vec<DisplayEntry> {
    let mut result = Vec::new();

    // ── Virtual <models> folder: all viewable files, sorted by path ──────
    let mut viewable: Vec<&OutputFile> = files.iter().filter(|f| f.is_viewable).collect();
    if !viewable.is_empty() {
        viewable.sort_by(|a, b| {
            b.modified_at
                .cmp(&a.modified_at)
                .then_with(|| a.relative_path.cmp(&b.relative_path))
        });
        let models_size: u64 = viewable.iter().map(|f| f.size).sum();
        result.push(DisplayEntry {
            name: "<models>".to_string(),
            relative_path: "<models>".to_string(),
            is_dir: true,
            is_virtual: true,
            depth: 0,
            file: None,
            file_count: viewable.len(),
            total_size: models_size,
        });
        for f in &viewable {
            result.push(DisplayEntry {
                // Full path as name so the user knows where the file lives
                name: f.relative_path.clone(),
                // Virtual path; "<models>" prefix is what collapse-tracking checks
                relative_path: format!("<models>/{}", f.relative_path),
                is_dir: false,
                is_virtual: true,
                depth: 1,
                file: Some((*f).clone()),
                file_count: 0,
                total_size: f.size,
            });
        }
    }

    // ── Real file tree ───────────────────────────────────────────────────
    let mut root = DirNode {
        name: String::new(),
        path: String::new(),
        subdirs: vec![],
        files: vec![],
    };
    for file in files {
        let parts: Vec<&str> = file.relative_path.split('/').collect();
        insert_recursive(&mut root, &parts, file.clone());
    }
    flatten_node(&root, 0, &mut result);
    result
}

/// Returns `true` when no ancestor directory of `relative_path` is in `collapsed`.
fn is_entry_visible(relative_path: &str, collapsed: &HashSet<String>) -> bool {
    let parts: Vec<&str> = relative_path.split('/').collect();
    for i in 1..parts.len() {
        let ancestor = parts[..i].join("/");
        if collapsed.contains(&ancestor) {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Server-side download / restore helpers (called from spawn blocks)
// ---------------------------------------------------------------------------

/// Download the ZIP archive of `folder_path` from the server and trigger a
/// browser save-as dialog via base64 + eval.
async fn trigger_download_zip(
    project_name: &str,
    folder_path: &str,
    filename: &str,
    mut toast_ctx: crate::mycomponents::ToastCtx,
) {
    let pn = project_name.to_string();
    let fp = folder_path.to_string();
    let fn_ = filename.to_string();

    // Show persistent progress toast while the ZIP is being prepared
    let progress_id = add_toast(
        &mut toast_ctx,
        format!("Downloading {fn_}…"),
        ToastType::Info,
        Some((0, 0)),
    );

    match download_project_outputs_zip(pn, fp).await {
        Ok(mut stream) => {
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(data) => bytes.extend_from_slice(&data),
                    Err(e) => {
                        error!(error = %e, "ZIP download stream error");
                        remove_toast(&mut toast_ctx, &progress_id);
                        add_toast(
                            &mut toast_ctx,
                            format!("Download failed: {e}"),
                            ToastType::Error,
                            None,
                        );
                        return;
                    }
                }
            }
            // Remove progress toast before triggering download
            remove_toast(&mut toast_ctx, &progress_id);

            // On Android, write to the SD card Download folder directly
            // (the `<a>` download attribute does not work in WebViews).
            // On other platforms, use the base64 + eval approach.
            if let Some(saved_path) = save_bytes_to_downloads(&bytes, &fn_) {
                tracing::info!(saved_to = %saved_path.display(), "ZIP backup saved locally");
                add_toast(
                    &mut toast_ctx,
                    format!("Downloaded {fn_} to {}", saved_path.display()),
                    ToastType::Info,
                    None,
                );
                remove_toast(&mut toast_ctx, &progress_id);
                return;
            } else {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let name_esc = js_escape(&fn_);
                let js = format!(
                    r#"const b64 = '{b64}';
const blob = new Blob([Uint8Array.from(atob(b64), c => c.charCodeAt(0))], {{type: 'application/zip'}});
const a = document.createElement('a');
a.href = URL.createObjectURL(blob);
a.download = '{name_esc}';
document.body.appendChild(a);
a.click();
setTimeout(() => {{ document.body.removeChild(a); URL.revokeObjectURL(a.href); }}, 10000);"#
                );
                let _ = eval(&js).await;
            }
            add_toast(
                &mut toast_ctx,
                format!("Downloaded {fn_}"),
                ToastType::Info,
                None,
            );
        }
        Err(e) => {
            error!(error = %e, "Failed to create ZIP archive");
            remove_toast(&mut toast_ctx, &progress_id);
            add_toast(
                &mut toast_ctx,
                format!("Failed to create ZIP: {e}"),
                ToastType::Error,
                None,
            );
        }
    }
}

/// Present a file-picker for a ZIP archive and send its raw bytes to the
/// server for restoration under `folder_path`.
///
/// Progress and errors are reported via the toast context.
async fn restore_files(
    project_name: &str,
    folder_path: &str,
    mut refresh_counter: Signal<u32>,
    mut toast_ctx: crate::mycomponents::ToastCtx,
) {
    // Show a persistent toast indicating the file picker is open
    let progress_id = add_toast(
        &mut toast_ctx,
        "Select a ZIP file to restore…".to_string(),
        ToastType::Info,
        Some((0, 0)),
    );

    // The eval JS opens a file picker, reads the selected file as bytes,
    // and sends the raw ZIP bytes as a base64 string back to Rust.
    let js = r#"
(async function() {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = '.zip';
    input.style.display = 'none';
    document.body.appendChild(input);

    const file = await new Promise((resolve) => {
        input.addEventListener('change', () => resolve(input.files[0]));
        input.click();
    });
    document.body.removeChild(input);

    if (!file) { dioxus.send(''); return; }

    const bytes = await new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(new Uint8Array(reader.result));
        reader.onerror = () => reject(reader.error);
        reader.readAsArrayBuffer(file);
    });

    // Convert Uint8Array to base64 string (chunked to avoid stack overflow).
    const chunkSize = 65536;
    let binary = '';
    for (let i = 0; i < bytes.length; i += chunkSize) {
        const chunk = bytes.subarray(i, Math.min(i + chunkSize, bytes.length));
        binary += String.fromCharCode.apply(null, chunk);
    }
    const b64 = btoa(binary);
    dioxus.send(b64);
    dioxus.send('__done__');
})();
"#;

    let mut eval_handle = document::eval(js);

    // Receive the base64-encoded ZIP data.
    let b64_data = match eval_handle.recv::<String>().await {
        Ok(s) if s.is_empty() => {
            // User cancelled the file picker.
            remove_toast(&mut toast_ctx, &progress_id);
            add_toast(
                &mut toast_ctx,
                "Restore cancelled".to_string(),
                ToastType::Info,
                None,
            );
            return;
        }
        Ok(s) => s,
        Err(_) => {
            remove_toast(&mut toast_ctx, &progress_id);
            add_toast(
                &mut toast_ctx,
                "Restore cancelled".to_string(),
                ToastType::Info,
                None,
            );
            return;
        }
    };

    // Wait for the __done__ sentinel.
    let _ = eval_handle.recv::<String>().await;

    // Decode base64 into raw ZIP bytes.
    let zip_bytes = match base64::engine::general_purpose::STANDARD.decode(&b64_data) {
        Ok(b) => b,
        Err(e) => {
            remove_toast(&mut toast_ctx, &progress_id);
            add_toast(
                &mut toast_ctx,
                format!("Invalid ZIP data: {e}"),
                ToastType::Error,
                None,
            );
            return;
        }
    };

    // Update progress toast to indicate restoration is in progress
    update_toast(
        &mut toast_ctx,
        &progress_id,
        Some("Restoring files…".to_string()),
        Some(Some((0, 1))),
    );

    // Send the raw ZIP bytes to the server for restoration.
    let byte_stream = crate::fullstack_compat::ByteStream::new(futures::stream::once(async move {
        crate::fullstack_compat::body::Bytes::from(zip_bytes)
    }));

    match restore_project_outputs(
        project_name.to_string(),
        folder_path.to_string(),
        byte_stream,
    )
    .await
    {
        Ok(()) => {
            remove_toast(&mut toast_ctx, &progress_id);
            add_toast(
                &mut toast_ctx,
                "Files restored successfully".to_string(),
                ToastType::Info,
                None,
            );
            refresh_counter += 1;
        }
        Err(e) => {
            error!(error = %e, "Restore failed");
            remove_toast(&mut toast_ctx, &progress_id);
            add_toast(
                &mut toast_ctx,
                format!("Failed to restore: {e}"),
                ToastType::Error,
                None,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

#[component]
pub fn OutputsTab(project_name: String) -> Element {
    debug!(project_name = %project_name, "Initializing outputs tab");

    let toast_ctx = crate::mycomponents::use_toast_ctx();
    let mut refresh_counter = use_signal(|| 0u32);
    let mut confirming_delete = use_signal(|| Option::<String>::None);
    let mut deleting_path = use_signal(|| Option::<String>::None);
    let mut collapsed = use_signal(HashSet::<String>::new);
    let mut confirming_clear_all = use_signal(|| false);
    let mut clearing_all = use_signal(|| false);
    let restoring = use_signal(|| false);
    let downloading_folder = use_signal(|| Option::<String>::None);
    let mut download_modal = use_signal(|| Option::<(String, String, bool)>::None);

    let project_name_res = project_name.clone();
    let files = use_resource(move || {
        let pn = project_name_res.clone();
        async move {
            let _ = refresh_counter();
            debug!(project_name = %pn, "Fetching output files");
            match list_project_outputs(pn.clone()).await {
                Ok(f) => {
                    info!(project_name = %pn, count = f.len(), "Loaded output files");
                    Ok(f)
                }
                Err(e) => {
                    error!(project_name = %pn, error = %e, "Failed to load output files");
                    Err(e)
                }
            }
        }
    });

    rsx! {
        div {
            class: "tab-content outputs-tab",

            // ── Toolbar ──────────────────────────────────────────────
            div {
                class: "outputs-toolbar",
                // ── Backup (root download as ZIP) ────────────────
                button {
                    class: "action-btn action-btn-primary",
                    disabled: downloading_folder().as_deref() == Some(""),
                    title: "Download all outputs as ZIP",
                    onclick: {
                        let pn = project_name.clone();
                        let mut dl = downloading_folder;
                        let tc = toast_ctx;
                        move |_| {
                            dl.set(Some(String::new()));
                            let pn2 = pn.clone();
                            let mut dl2 = dl.clone();
                            let tc2 = tc.clone();
                            spawn(async move {
                                trigger_download_zip(
                                    &pn2, "", &format!("{}-backup.zip", pn2), tc2,
                                ).await;
                                dl2.set(None);
                            });
                        }
                    },
                    Icon { icon: BsDownload }
                    span { class: "btn-label", " Backup" }
                }

                // ── Restore (upload ZIP to root) ─────────────────
                button {
                    class: "action-btn action-btn-success",
                    disabled: restoring(),
                    title: "Restore outputs from a ZIP archive",
                    onclick: {
                        let pn = project_name.clone();
                        let mut rst = restoring;
                        let rc = refresh_counter;
                        let tc = toast_ctx;
                        move |_| {
                            rst.set(true);
                            let pn2 = pn.clone();
                            let mut rst2 = rst.clone();
                            let rc2 = rc.clone();
                            let tc2 = tc.clone();
                            spawn(async move {
                                restore_files(&pn2, "", rc2, tc2).await;
                                rst2.set(false);
                            });
                        }
                    },
                    Icon { icon: BsUpload }
                    span { class: "btn-label", " Restore" }
                }

                if clearing_all() {
                    span { class: "outputs-toolbar-status", Icon { icon: BsHourglass } }
                } else if confirming_clear_all() {
                    button {
                        class: "action-btn action-btn-success",
                        title: "Confirm delete all",
                        onclick: {
                            let pn = project_name.clone();
                            let tc = toast_ctx;
                            move |_| {
                                confirming_clear_all.set(false);
                                clearing_all.set(true);
                                let pn = pn.clone();
                                let mut tc = tc.clone();
                                spawn(async move {
                                    match clear_project_outputs(pn.clone()).await {
                                        Ok(()) => {
                                            info!(project_name = %pn, "All outputs cleared");
                                            add_toast(&mut tc, "All outputs cleared".to_string(), ToastType::Info, None);
                                            collapsed.write().clear();
                                            refresh_counter += 1;
                                        }
                                        Err(e) => {
                                            error!(project_name = %pn, error = %e, "Failed to clear");
                                            add_toast(&mut tc, format!("Failed to clear all: {e}"), ToastType::Error, None);
                                        }
                                    }
                                    clearing_all.set(false);
                                });
                            }
                        },
                        Icon { icon: BsCheck2 }
                        span { class: "btn-label", " Sure?" }
                    }
                    button {
                        class: "action-btn",
                        onclick: move |_| confirming_clear_all.set(false),
                        Icon { icon: BsX }
                    }
                } else {
                    button {
                        class: "action-btn action-btn-danger",
                        title: "Delete all outputs",
                        onclick: move |_| confirming_clear_all.set(true),
                        Icon { icon: BsTrash3 }
                        span { class: "btn-label", " Delete All" }
                    }
                }

                button {
                    class: "action-btn action-btn-warning",
                    onclick: move |_| {
                        debug!("Refresh outputs");
                        refresh_counter += 1;
                    },
                    Icon { icon: BsArrowRepeat }
                    span { class: "btn-label", " Refresh" }
                }
            }

            // ── File tree ─────────────────────────────────────────────────
            {
                let coll = collapsed();
                match files() {
                    None => rsx! {
                        div { class: "outputs-placeholder", "Loading…" }
                    },
                    Some(Err(e)) => rsx! {
                        div { class: "outputs-placeholder error", "{e}" }
                    },
                    Some(Ok(fl)) if fl.is_empty() => rsx! {
                        div { class: "outputs-placeholder",
                            "No output files yet. Run the pipeline to generate results."
                        }
                    },
                    Some(Ok(fl)) => {
                        let display_entries = build_display_list(&fl);
                        rsx! {
                            div {
                                class: "outputs-file-list",
                                for entry in display_entries {
                                    if is_entry_visible(&entry.relative_path, &coll) {
                                        {
                                            let rel_path   = entry.relative_path.clone();
                                            let entry_name = entry.name.clone();
                                            let is_dir     = entry.is_dir;
                                            let is_virtual = entry.is_virtual;
                                            let depth      = entry.depth;
                                            let icon       = file_icon(&entry_name, is_dir, is_dir && is_virtual);

                                            let is_collapsed  = is_dir && coll.contains(&rel_path);
                                            let is_confirming = confirming_delete() == Some(rel_path.clone());
                                            let is_deleting   = deleting_path()    == Some(rel_path.clone());
                                            let is_viewable   = entry.file.as_ref().map(|f| f.is_viewable).unwrap_or(false);
                                            let has_glb       = entry.file.as_ref().map(|f| f.glb_available).unwrap_or(false);

                                            // For download/view use the *real* file path (handles virtual entries)
                                            let actual_path = entry.file.as_ref()
                                                .map(|f| f.relative_path.clone())
                                                .unwrap_or_else(|| rel_path.clone());
                                            let actual_name = entry.file.as_ref()
                                                .map(|f| f.name.clone())
                                                .unwrap_or_else(|| entry_name.clone());
                                            let modified_at = entry.file.as_ref().map(|f| f.modified_at).unwrap_or(0);

                                            let size_text = format_size(entry.total_size);
                                            let date_text = if is_dir { String::new() } else { format_date(modified_at) };
                                            let dir_meta  = if is_dir {
                                                format!("{} files · {}", entry.file_count, format_size(entry.total_size))
                                            } else {
                                                String::new()
                                            };

                                            // Closure captures
                                            let rp_toggle  = rel_path.clone();
                                            let rp_confirm = rel_path.clone();
                                            let rp_del     = rel_path.clone();
                                            let pn_del     = project_name.clone();
                                            let rp_view    = actual_path.clone();
                                            let pn_view    = project_name.clone();
                                            let pn_dl      = project_name.clone();
                                            let rp_dl      = actual_path.clone();
                                            let fname_dl   = actual_name.clone();
                                            let tc_dl      = toast_ctx;
                                            let tc_dl3     = toast_ctx;

                                            rsx! {
                                                div {
                                                    key: "{rel_path}",
                                                    class: if is_virtual && is_dir {
                                                        "outputs-entry outputs-entry-virtual-dir"
                                                    } else if is_virtual {
                                                        "outputs-entry outputs-entry-virtual-file"
                                                    } else if is_dir {
                                                        "outputs-entry outputs-entry-dir"
                                                    } else {
                                                        "outputs-entry outputs-entry-file"
                                                    },
                                                    style: "--depth: {depth};",

                                                    // Toggle (dirs) / spacer (files)
                                                    if is_dir {
                                                        button {
                                                            class: "outputs-expand-btn",
                                                            title: if is_collapsed { "Expand" } else { "Collapse" },
                                                            onclick: move |_| {
                                                                let mut c = collapsed.write();
                                                                if c.contains(&rp_toggle) {
                                                                    c.remove(&rp_toggle);
                                                                } else {
                                                                    c.insert(rp_toggle.clone());
                                                                }
                                                            },
                                                            if is_collapsed { Icon { icon: BsChevronRight } } else { Icon { icon: BsChevronDown } }
                                                        }
                                                    } else {
                                                        span { class: "outputs-expand-spacer" }
                                                    }

                                                    // Icon
                                                    span { class: "outputs-entry-icon", "{icon}" }

                                                    // Info column
                                                    div { class: "outputs-file-info-wrapper",
                                                        div { class: "outputs-file-name",
                                                            title: "{entry_name}",
                                                            "{entry_name}"
                                                        }
                                                        if is_dir {
                                                            div { class: "outputs-file-meta",
                                                                span { class: "meta-size", "{dir_meta}" }
                                                            }
                                                        } else {
                                                            div { class: "outputs-file-meta",
                                                                span { class: "meta-size", "{size_text}" }
                                                                if !date_text.is_empty() {
                                                                    span { class: "meta-sep", " · " }
                                                                    span { class: "meta-date", "{date_text}" }
                                                                }
                                                            }
                                                        }
                                                    }

                                                    // Folder download (exclude virtual <models> directory)
                                                    if is_dir && !is_virtual {
                                                        button {
                                                            class: "outputs-btn outputs-dir-download-btn",
                                                            disabled: downloading_folder().as_deref() == Some(&rel_path),
                                                            title: "Download folder as ZIP",
                                                            onclick: {
                                                                let pn = project_name.clone();
                                                                let fp = rel_path.clone();
                                                                let fn_ = entry_name.clone();
                                                                let mut dl = downloading_folder;
                                                                let tc = toast_ctx;
                                                                move |_| {
                                                                    dl.set(Some(fp.clone()));
                                                                    let pn2 = pn.clone();
                                                                    let fp2 = fp.clone();
                                                                    let fn2 = fn_.clone();
                                                                    let mut dl2 = dl.clone();
                                                                    let tc2 = tc.clone();
                                                                    spawn(async move {
                                                                        let zip_name = format!("{}-{}.zip", pn2, fn2);
                                                                        trigger_download_zip(
                                                                            &pn2, &fp2, &zip_name, tc2,
                                                                        ).await;
                                                                        dl2.set(None);
                                                                    });
                                                                }
                                                            },
                                                            Icon { icon: BsDownload }
                                                            span { class: "btn-label", " Backup" }
                                                        }
                                                    }

                                                    // Download (files only) — viewable files with GLB show a
                                                    // modal offering Raw / GLB choices; everything else downloads
                                                    // raw bytes directly.
                                                    if !is_dir {
                                                        button {
                                                            class: "outputs-btn outputs-download-link",
                                                            title: "Download {actual_name}",
                                                            onclick: move |_| {
                                                                let pn = pn_dl.clone();
                                                                let rp = rp_dl.clone();
                                                                let fname = fname_dl.clone();
                                                                let tc = tc_dl;
                                                                if is_viewable && has_glb {
                                                                    download_modal.set(Some((rp, fname, true)));
                                                                } else {
                                                                    spawn(async move {
                                                                        #[cfg(not(feature = "demo"))]
                                                                        {
                                                                            trigger_download_from_url(
                                                                                &pn, &rp, &fname, "application/octet-stream",
                                                                                "/api/projects/{name}/outputs/bytes", tc,
                                                                            ).await;
                                                                        }
                                                                        #[cfg(feature = "demo")]
                                                                        {
                                                                            let mut tc = tc;
                                                                            match get_project_output_bytes(pn, rp).await {
                                                                                Ok(mut stream) => {
                                                                                    let mut bytes = Vec::new();
                                                                                    while let Some(chunk) = stream.next().await {
                                                                                        match chunk {
                                                                                            Ok(data) => bytes.extend_from_slice(&data),
                                                                                            Err(e) => {
                                                                                                error!(error = %e, "Download stream error");
                                                                                                add_toast(&mut tc, format!("Download failed: {e}"), ToastType::Error, None);
                                                                                                return;
                                                                                            }
                                                                                        }
                                                                                    }
                                                                                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                                                                                    let fname_esc = js_escape(&fname);
                                                                                    let js = format!(
                                                                                        r#"const b64 = '{b64}';
	const blob = new Blob([Uint8Array.from(atob(b64), c => c.charCodeAt(0))], {{type: 'application/octet-stream'}});
	const a = document.createElement('a');
	a.href = URL.createObjectURL(blob);
	a.download = '{fname_esc}';
	document.body.appendChild(a);
	a.click();
	setTimeout(() => {{ document.body.removeChild(a); URL.revokeObjectURL(a.href); }}, 10000);"#
                                                                                        );
                                                                                        let _ = eval(&js).await;
                                                                                        add_toast(&mut tc, format!("Downloaded {fname}"), ToastType::Info, None);
                                                                                    }
                                                                                    Err(e) => {
                                                                                        error!(error = %e, "Download failed");
                                                                                        add_toast(&mut tc, format!("Download failed: {e}"), ToastType::Error, None);
                                                                                    }
                                                                                }
                                                                            }
                                                                        });
                                                                    }
                                                                },
                                                                Icon { icon: BsDownload }
                                                                span { class: "btn-label", " Backup" }
                                                            }
                                                        }

                                                    // 3D View (viewable files only) — navigate to the viewer route
                                                    if is_viewable {
                                                        button {
                                                            class: "outputs-btn outputs-view-3d-btn",
                                                            onclick: move |_| {
                                                                let pn = pn_view.clone();
                                                                let rp = rp_view.clone();
                                                                debug!(file = %rp, "Navigating to viewer route");

                                                                // Navigate to the viewer.  The file path is base64-url-safe
                                                                // encoded so it does NOT contain `/` or `%` which would be
                                                                // decoded by the Dioxus router before route matching and
                                                                // break the path segments.
                                                                let encoded = base64::engine::general_purpose::URL_SAFE
                                                                    .encode(rp.as_bytes());
                                                                navigator().push(crate::Route::Viewer {
                                                                    name: pn,
                                                                    file_encoded: encoded,
                                                                    cfg: "-".to_string(),
                                                                });
                                                            },
                                                            Icon { icon: BsEye }
                                                            span { class: "btn-label", " View 3D" }
                                                        }
                                                    }

                                                    // Delete — hidden for virtual entries
                                                    if !is_virtual {
                                                        if is_deleting {
                                                            span { class: "outputs-entry-icon", Icon { icon: BsHourglass } }
                                                        } else if is_confirming {
                                                            button {
                                                                class: "outputs-btn outputs-confirm-del-btn",
                                                                onclick: move |_| {
                                                                    let pn = pn_del.clone();
                                                                    let rp = rp_del.clone();
                                                                    deleting_path.set(Some(rp.clone()));
                                                                    confirming_delete.set(None);
                                                                    let mut tc3 = tc_dl3;
                                                                    spawn(async move {
                                                                                match delete_project_output(pn.clone(), rp.clone()).await {
                                                                                    Ok(()) => {
                                                                                        info!(path = %rp, "Deleted");
                                                                                        collapsed.write().remove(&rp);
                                                                                        refresh_counter += 1;
                                                                                    }
                                                                                    Err(e) => {
                                                                                        error!(path = %rp, error = %e, "Delete failed");
                                                                                        add_toast(&mut tc3, format!("Failed to delete: {e}"), ToastType::Error, None);
                                                                                    }
                                                                                }
                                                                                deleting_path.set(None);
                                                                            });
                                                                },
                                                                Icon { icon: BsCheck2 }
                                                                span { class: "btn-label", " Sure?" }
                                                            }
                                                            button {
                                                                class: "outputs-btn outputs-cancel-del-btn",
                                                                onclick: move |_| confirming_delete.set(None),
                                                                Icon { icon: BsX }
                                                            }
                                                        } else {
                                                            button {
                                                                class: "outputs-btn outputs-del-btn",
                                                                title: if is_dir { "Delete folder" } else { "Delete file" },
                                                                onclick: move |_| confirming_delete.set(Some(rp_confirm.clone())),
                                                                Icon { icon: BsTrash3 }
                                                                span {
                                                                    class: "btn-label",
                                                                    if is_dir { " Del Folder" } else { " Delete" }
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
                        }
                    }
                }
            }

            // ── Download modal overlay ─────────────────────────────────────
            {
                let modal = download_modal();
                if let Some((modal_path, modal_name, modal_has_glb)) = modal {
                    let modal_path_clone = modal_path.clone();
                    let modal_name_clone = modal_name.clone();
                    let pn = project_name.clone();

                    rsx! {
                        div {
                            class: "modal-overlay",
                            onclick: move |_| download_modal.set(None),
                            div {
                                class: "modal-content",
                                onclick: move |e| e.stop_propagation(),
                                h3 { "Download options" }
                                p { class: "download-hint", "Choose a format for {modal_name_clone}" }
                                div {
                                    class: "download-options-vertical",
                                    div {
                                        class: "download-option",
                                        button {
                                            class: "download-option-btn",
                                            onclick: {
                                                let pn = pn.clone();
                                                let rp = modal_path_clone.clone();
                                                let fname = modal_name_clone.clone();
                                                let mut dm = download_modal;
                                                let tc = toast_ctx;
                                                move |_| {
                                                    dm.set(None);
                                                    let pn2 = pn.clone();
                                                    let rp2 = rp.clone();
                                                    let fname2 = fname.clone();
                                                    let tc2 = tc.clone();
                                                    spawn(async move {
                                                        #[cfg(not(feature = "demo"))]
                                                        {
                                                            trigger_download_from_url(
                                                                &pn2, &rp2, &fname2, "application/octet-stream",
                                                                "/api/projects/{name}/outputs/bytes", tc2,
                                                            ).await;
                                                        }
                                                        #[cfg(feature = "demo")]
                                                        {
                                                            let mut tc2 = tc2;
                                                            match get_project_output_bytes(pn2, rp2).await {
                                                                Ok(mut stream) => {
                                                                    let mut bytes = Vec::new();
                                                                    while let Some(chunk) = stream.next().await {
                                                                        match chunk {
                                                                            Ok(data) => bytes.extend_from_slice(&data),
                                                                            Err(e) => {
                                                                                error!(error = %e, "Raw download stream error");
                                                                                return;
                                                                            }
                                                                        }
                                                                    }
                                                                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                                                                    let fname_esc = js_escape(&fname2);
                                                                    let js = format!(
                                                                        r#"const b64 = '{b64}';
const blob = new Blob([Uint8Array.from(atob(b64), c => c.charCodeAt(0))], {{type: 'application/octet-stream'}});
const a = document.createElement('a');
a.href = URL.createObjectURL(blob);
a.download = '{fname_esc}';
document.body.appendChild(a);
a.click();
setTimeout(() => {{ document.body.removeChild(a); URL.revokeObjectURL(a.href); }}, 10000);"#
                                                                    );
                                                                    let _ = eval(&js).await;
                                                                    add_toast(
                                                                        &mut tc2,
                                                                        format!("Downloaded {fname2}"),
                                                                        ToastType::Info,
                                                                        None,
                                                                    );
                                                                }
                                                                Err(e) => {
                                                                    error!(error = %e, "Raw download failed");
                                                                }
                                                            }
                                                        }
                                                    });
                                                }
                                            },
                                            div { class: "download-option-title", "Download Raw" }
                                            div { class: "download-option-desc", "Original file; may need other supporting files to open." }
                                        }
                                    }
                                    if modal_has_glb {
                                        div {
                                            class: "download-option",
                                            button {
                                                class: "download-option-btn",
                                                onclick: {
                                                    let pn = pn.clone();
                                                    let rp = modal_path_clone.clone();
                                                    let fname = modal_name_clone.clone();
                                                    let mut dm = download_modal;
                                                    let tc = toast_ctx;
                                                    move |_| {
                                                        dm.set(None);
                                                        let pn2 = pn.clone();
                                                        let rp2 = rp.clone();
                                                        let fname2 = fname.clone();
                                                        let tc2 = tc.clone();
                                                        // Derive a .glb filename from the original name
                                                        let glb_name = if let Some((base, _)) = fname2.rsplit_once('.') {
                                                            format!("{base}.glb")
                                                        } else {
                                                            format!("{fname2}.glb")
                                                        };
                                                        spawn(async move {
                                                            #[cfg(not(feature = "demo"))]
                                                            {
                                                                trigger_download_from_url(
                                                                    &pn2, &rp2, &glb_name, "model/gltf-binary",
                                                                    "/api/projects/{name}/outputs/glb", tc2,
                                                                ).await;
                                                            }
                                                            #[cfg(feature = "demo")]
                                                            {
                                                                let mut tc2 = tc2;
                                                                match get_project_output_glb(pn2, rp2).await {
                                                                    Ok(mut stream) => {
                                                                        let mut bytes = Vec::new();
                                                                        while let Some(chunk) = stream.next().await {
                                                                            match chunk {
                                                                                Ok(data) => bytes.extend_from_slice(&data),
                                                                                Err(e) => {
                                                                                    error!(error = %e, "GLB download stream error");
                                                                                    return;
                                                                                }
                                                                            }
                                                                        }
                                                                        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                                                                        let fname_esc = js_escape(&glb_name);
                                                                        let js = format!(
                                                                            r#"const b64 = '{b64}';
const blob = new Blob([Uint8Array.from(atob(b64), c => c.charCodeAt(0))], {{type: 'model/gltf-binary'}});
const a = document.createElement('a');
a.href = URL.createObjectURL(blob);
a.download = '{fname_esc}';
document.body.appendChild(a);
a.click();
setTimeout(() => {{ document.body.removeChild(a); URL.revokeObjectURL(a.href); }}, 10000);"#
                                                                        );
                                                                        let _ = eval(&js).await;
                                                                        add_toast(
                                                                            &mut tc2,
                                                                            format!("Downloaded {glb_name}"),
                                                                            ToastType::Info,
                                                                            None,
                                                                        );
                                                                    }
                                                                    Err(e) => {
                                                                        error!(error = %e, "GLB download failed");
                                                                    }
                                                                }
                                                            }
                                                        });
                                                    }
                                                },
                                                div { class: "download-option-title", "Download GLB" }
                                                div { class: "download-option-desc", "Modern, more compatible format; packs all supporting files into a single file." }
                                            }
                                        }
                                    }
                                    button {
                                        class: "modal-cancel-btn",
                                        onclick: move |_| download_modal.set(None),
                                        "Cancel"
                                    }
                                }
                            }
                        }
                    }
                } else {
                    rsx! {}
                }
            }
        }
    }
}
