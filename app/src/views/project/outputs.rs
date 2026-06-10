use crate::mycomponents::{add_toast, remove_toast, update_toast, ToastType};
use crate::server::{
    clear_project_outputs, delete_project_output, get_project_output_bytes, list_project_outputs,
    write_project_output,
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
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use tracing::{debug, error, info};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

/// Escape a string for embedding inside a JS single-quoted string literal.
fn js_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Yield control back to the event loop so the UI stays responsive
/// during long-running synchronous work (e.g. ZIP compression).
#[cfg(target_arch = "wasm32")]
async fn yield_to_event_loop() {
    gloo_timers::future::TimeoutFuture::new(0).await;
}

/// Yield control back to the event loop (non-WASM, e.g. server-side tokio).
#[cfg(not(target_arch = "wasm32"))]
async fn yield_to_event_loop() {
    tokio::task::yield_now().await;
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

/// Build a map: directory path → list of (relative_path, name) for all files under it.
/// Used by the folder-download feature to know which files belong to a directory.
fn build_dir_files_map(files: &[OutputFile]) -> HashMap<String, Vec<(String, String)>> {
    let mut map = HashMap::<String, Vec<(String, String)>>::new();
    for f in files {
        // Every file belongs to the root.
        map.entry(String::new())
            .or_default()
            .push((f.relative_path.clone(), f.name.clone()));
        // … and to each ancestor directory along its path.
        if let Some(slash) = f.relative_path.rfind('/') {
            let mut cur = f.relative_path[..slash].to_string();
            map.entry(cur.clone())
                .or_default()
                .push((f.relative_path.clone(), f.name.clone()));
            while let Some(slash) = cur.rfind('/') {
                cur = cur[..slash].to_string();
                map.entry(cur.clone())
                    .or_default()
                    .push((f.relative_path.clone(), f.name.clone()));
            }
        }
    }
    map
}

/// Compute a zip-entry path relative to `folder_path`.
/// For the root folder (`""`) the full relative path is used unchanged;
/// for sub-folders the folder prefix is stripped.
fn zip_entry_path(relative_path: &str, folder_path: &str) -> String {
    if folder_path.is_empty() {
        relative_path.to_string()
    } else {
        let prefix = format!("{}/", folder_path);
        relative_path
            .strip_prefix(&prefix)
            .unwrap_or(relative_path)
            .to_string()
    }
}

// ---------------------------------------------------------------------------
// Async folder download / restore helpers (called from spawn blocks)
// ---------------------------------------------------------------------------

/// Create a Blob from the ZIP bytes and trigger a browser download.
///
/// On WASM targets this uses `web-sys` / `js-sys` to build the Blob directly
/// from a view into WASM linear memory, avoiding costly base64 serialization.
/// Falls back to base64 + eval on other targets.
#[cfg(target_arch = "wasm32")]
async fn trigger_zip_download(zip_bytes: Vec<u8>, zip_name: &str) {
    use js_sys::Array;

    // SAFETY: we create a short-lived view into WASM memory and immediately
    // hand it off to the Blob constructor (which copies the data). No
    // allocations or yielding happen between the view and the Blob call.
    let uint8_view = unsafe { js_sys::Uint8Array::view(&zip_bytes) };
    let arr = Array::new();
    arr.push(uint8_view.as_ref());

    let mut opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/zip");
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(arr.as_ref(), &opts)
        .expect("Failed to create Blob from ZIP bytes");
    let url =
        web_sys::Url::create_object_url_with_blob(&blob).expect("Failed to create object URL");

    // Minimal one-shot eval: just create an anchor and click it.
    let zip_name_esc = js_escape(zip_name);
    let js = format!(
        r#"const a = document.createElement('a');
a.href = '{url}';
a.download = '{zip_name_esc}';
document.body.appendChild(a);
a.click();
setTimeout(() => {{ document.body.removeChild(a); URL.revokeObjectURL(a.href); }}, 10000);"#
    );
    let _ = eval(&js).await;
}

#[cfg(not(target_arch = "wasm32"))]
async fn trigger_zip_download(zip_bytes: Vec<u8>, zip_name: &str) {
    // Fallback path (e.g. server-side tests): base64 + eval.
    let b64 = base64::engine::general_purpose::STANDARD.encode(&zip_bytes);
    let zip_name_esc = js_escape(zip_name);
    let js = format!(
        r#"const b64 = '{b64}';
const blob = new Blob([Uint8Array.from(atob(b64), c => c.charCodeAt(0))], {{type: 'application/zip'}});
const a = document.createElement('a');
a.href = URL.createObjectURL(blob);
a.download = '{zip_name_esc}';
document.body.appendChild(a);
a.click();
setTimeout(() => {{ document.body.removeChild(a); URL.revokeObjectURL(a.href); }}, 10000);"#
    );
    let _ = eval(&js).await;
}

/// Download every file under `folder_path` and save the user a ZIP archive.
///
/// On error, still exports a partial zip with whatever files were successfully
/// downloaded and reports the failures via the toast context.
async fn download_folder_zip(
    project_name: &str,
    folder_path: &str,
    zip_name: &str,
    entries: Vec<(String, String)>,
    mut toast_ctx: crate::mycomponents::ToastCtx,
) {
    if entries.is_empty() {
        add_toast(
            &mut toast_ctx,
            "Folder has no files".to_string(),
            ToastType::Error,
            None,
        );
        return;
    }

    let total = entries.len();
    let progress_id = add_toast(
        &mut toast_ctx,
        format!("Downloading {} files…", total),
        ToastType::Info,
        Some((0, total)),
    );

    // Download every file's bytes, collecting errors along the way.
    let mut file_data: Vec<(String, Vec<u8>)> = Vec::with_capacity(total);
    let mut errors: Vec<String> = Vec::new();
    for (done, (rel_path, _name)) in entries.iter().enumerate() {
        update_toast(
            &mut toast_ctx,
            &progress_id,
            Some(format!("Downloading {}", rel_path)),
            Some(Some((done, total))),
        );
        let pn = project_name.to_string();
        let rp = rel_path.clone();
        match get_project_output_bytes(pn, rp).await {
            Ok(mut stream) => {
                let mut bytes = Vec::new();
                let mut stream_error = false;
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(data) => bytes.extend_from_slice(&data),
                        Err(e) => {
                            errors.push(format!("Failed to read {}: {e}", rel_path));
                            stream_error = true;
                            break;
                        }
                    }
                }
                if !stream_error {
                    let zpath = zip_entry_path(rel_path, folder_path);
                    file_data.push((zpath, bytes));
                }
            }
            Err(e) => {
                errors.push(format!("Failed to download {}: {e}", rel_path));
            }
        }
    }

    // If nothing could be downloaded, show error(s) and bail.
    if file_data.is_empty() {
        remove_toast(&mut toast_ctx, &progress_id);
        let msg = if errors.len() == 1 {
            errors.into_iter().next().unwrap()
        } else {
            format!(
                "Download failed: {} errors — no files could be retrieved",
                errors.len()
            )
        };
        add_toast(&mut toast_ctx, msg, ToastType::Error, None);
        return;
    }

    // Create ZIP archive in memory using the Rust `zip` crate.
    let mut zip_buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip_writer = zip::ZipWriter::new(&mut zip_buf);
        for (zpath, bytes) in &file_data {
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            if let Err(e) = zip_writer.start_file(zpath, options) {
                error!("Failed to start ZIP entry {}: {e}", zpath);
            }
            if let Err(e) = zip_writer.write_all(bytes) {
                error!("Failed to write ZIP entry {}: {e}", zpath);
            }
            // Yield to the event loop so the UI stays responsive during
            // ZIP compression (which may block for large files).
            yield_to_event_loop().await;
        }
        if let Err(e) = zip_writer.finish() {
            error!("Failed to finish ZIP: {e}");
        }
    }
    let zip_bytes = zip_buf.into_inner();

    // Trigger browser download using the optimal path for the current target.
    trigger_zip_download(zip_bytes, zip_name).await;

    remove_toast(&mut toast_ctx, &progress_id);

    // Report success (and any partial failures).
    let ok_count = file_data.len();
    add_toast(
        &mut toast_ctx,
        if ok_count == 1 {
            format!("Downloaded {} file as {}", ok_count, zip_name)
        } else {
            format!("Downloaded {} files as {}", ok_count, zip_name)
        },
        ToastType::Info,
        None,
    );
    if !errors.is_empty() {
        add_toast(
            &mut toast_ctx,
            format!("Some files failed: {}", errors.join("; ")),
            ToastType::Error,
            None,
        );
    }
}

/// Present a file-picker for a ZIP archive, extract every entry and upload
/// each file to the server inside `folder_path`.
///
/// Progress is reported via the toast context.
/// When cancelled by the user, `restoring` is reset and an info toast is shown.
async fn restore_from_zip(
    project_name: &str,
    folder_path: &str,
    mut restoring: Signal<bool>,
    mut refresh_counter: Signal<u32>,
    mut toast_ctx: crate::mycomponents::ToastCtx,
) {
    // The eval JS opens a file picker, reads the selected file as bytes
    // (via native FileReader, no JSZip needed), and sends the raw ZIP bytes
    // as a base64 string back to Rust for extraction.
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
            add_toast(
                &mut toast_ctx,
                "Restore cancelled".to_string(),
                ToastType::Info,
                None,
            );
            restoring.set(false);
            return;
        }
        Ok(s) => s,
        Err(_) => {
            add_toast(
                &mut toast_ctx,
                "Restore cancelled".to_string(),
                ToastType::Info,
                None,
            );
            restoring.set(false);
            return;
        }
    };

    // Wait for the __done__ sentinel.
    let _ = eval_handle.recv::<String>().await;

    // Decode base64 into raw ZIP bytes.
    let zip_bytes = match base64::engine::general_purpose::STANDARD.decode(&b64_data) {
        Ok(b) => b,
        Err(e) => {
            add_toast(
                &mut toast_ctx,
                format!("Invalid ZIP data: {e}"),
                ToastType::Error,
                None,
            );
            restoring.set(false);
            return;
        }
    };

    // Parse the ZIP archive using the Rust `zip` crate.
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive = match zip::ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(e) => {
            add_toast(
                &mut toast_ctx,
                format!("Invalid ZIP file: {e}"),
                ToastType::Error,
                None,
            );
            restoring.set(false);
            return;
        }
    };

    let total = archive.len();
    let progress_id = add_toast(
        &mut toast_ctx,
        format!("Restoring {} files into '{}'…", total, folder_path),
        ToastType::Info,
        Some((0, total)),
    );

    let mut done = 0usize;
    for i in 0..total {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(_) => break,
        };

        if entry.is_dir() {
            continue;
        }

        let path = entry.name().to_string();

        let mut bytes = Vec::new();
        if entry.read_to_end(&mut bytes).is_err() {
            add_toast(
                &mut toast_ctx,
                format!("Failed to read {} from ZIP", path),
                ToastType::Error,
                None,
            );
            break;
        }

        // Prepend folder prefix.
        let target_path = if folder_path.is_empty() {
            path.clone()
        } else {
            format!("{}/{}", folder_path, path)
        };

        // Upload.
        let byte_stream =
            crate::fullstack_compat::ByteStream::new(futures::stream::once(async move {
                crate::fullstack_compat::body::Bytes::from(bytes)
            }));
        match write_project_output(project_name.to_string(), target_path, byte_stream).await {
            Ok(()) => {
                done += 1;
                update_toast(
                    &mut toast_ctx,
                    &progress_id,
                    Some(format!("Restoring {}", path)),
                    Some(Some((done, total))),
                );
            }
            Err(e) => {
                add_toast(
                    &mut toast_ctx,
                    format!("Failed to write {}: {e}", path),
                    ToastType::Error,
                    None,
                );
                break;
            }
        }
    }

    restoring.set(false);
    remove_toast(&mut toast_ctx, &progress_id);
    add_toast(
        &mut toast_ctx,
        format!("Restored {} files", done),
        ToastType::Info,
        None,
    );
    refresh_counter += 1;
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
    let restore_folder = use_signal(|| Option::<String>::None);

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
                            let mut tc2 = tc.clone();
                            spawn(async move {
                                let fl = match crate::server::list_project_outputs(pn2.clone()).await {
                                    Ok(f) => f,
                                    Err(e) => {
                                        add_toast(&mut tc2, format!("Failed to list outputs: {e}"), ToastType::Error, None);
                                        dl2.set(None);
                                        return;
                                    }
                                };
                                let entries = build_dir_files_map(&fl);
                                let root_entries = entries.get("").cloned().unwrap_or_default();
                                download_folder_zip(
                                    &pn2, "", &format!("{}-backup.zip", pn2),
                                    root_entries, tc2.clone(),
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
                            let rst2 = rst.clone();
                            let rc2 = rc.clone();
                            let tc2 = tc.clone();
                            spawn(async move {
                                restore_from_zip(&pn2, "", rst2, rc2, tc2).await;
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
                                                                let fl_clone = fl.clone();
                                                                move |_| {
                                                                    dl.set(Some(fp.clone()));
                                                                    let pn2 = pn.clone();
                                                                    let fp2 = fp.clone();
                                                                    let fn2 = fn_.clone();
                                                                    let mut dl2 = dl.clone();
                                                                    let tc2 = tc.clone();
                                                                    let fl2 = fl_clone.clone();
                                                                    spawn(async move {
                                                                        let entries = build_dir_files_map(&fl2);
                                                                        let file_entries = entries.get(&fp2).cloned().unwrap_or_default();
                                                                        let zip_name = format!("{}-{}.zip", pn2, fn2);
                                                                        download_folder_zip(
                                                                            &pn2, &fp2, &zip_name, file_entries, tc2.clone(),
                                                                        ).await;
                                                                        dl2.set(None);
                                                                    });
                                                                }
                                                            },
                                                            Icon { icon: BsDownload }
                                                            span { class: "btn-label", " Backup" }
                                                        }

                                                        // Folder restore (non-virtual dirs only)
                                                        if !is_virtual {
                                                            button {
                                                                class: "outputs-btn outputs-restore-btn",
                                                                disabled: restoring(),
                                                                title: "Restore folder contents from ZIP",
                                                                onclick: {
                                                                    let pn = project_name.clone();
                                                                    let fp = rel_path.clone();
                                                                    let mut rst = restoring;
                                                                    let rc = refresh_counter;
                                                                    let mut rst_f = restore_folder;
                                                                    let tc = toast_ctx;
                                                                    move |_| {
                                                                        rst_f.set(Some(fp.clone()));
                                                                        rst.set(true);
                                                                        let pn2 = pn.clone();
                                                                        let fp2 = fp.clone();
                                                                        let rst2 = rst.clone();
                                                                        let rc2 = rc.clone();
                                                                        let mut rst_f2 = rst_f.clone();
                                                                        let tc2 = tc.clone();
                                                                        spawn(async move {
                                                                            restore_from_zip(&pn2, &fp2, rst2, rc2, tc2).await;
                                                                            rst_f2.set(None);
                                                                        });
                                                                    }
                                                                },
                                                                Icon { icon: BsUpload }
                                                                span { class: "btn-label", " Restore" }
                                                            }
                                                        }
                                                    }

                                                    // Download (files only) — fetches bytes through the
                                                    // Dioxus server-function protocol, then triggers the
                                                    // browser save-as dialog via a native Blob URL.
                                                    if !is_dir {
                                                        button {
                                                            class: "outputs-btn outputs-download-link",
                                                            title: "Download {actual_name}",
                                                            onclick: move |_| {
                                                                let pn = pn_dl.clone();
                                                                let rp = rp_dl.clone();
                                                                let fname = fname_dl.clone();
                                                                let mut tc = tc_dl;
                                                                spawn(async move {
                                                                     // Download bytes, then trigger browser Save As via Blob URL.
                                                                     match get_project_output_bytes(pn, rp).await {
                                                                         Ok(stream) => {
                                                                             let mut bytes = Vec::new();
                                                                             let mut s = stream;
                                                                             while let Some(chunk) = s.next().await {
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
                                                                                 r#"const b64 = '{b64}';\nconst blob = new Blob([Uint8Array.from(atob(b64), c => c.charCodeAt(0))], {{type: 'application/octet-stream'}});\nconst a = document.createElement('a');\na.href = URL.createObjectURL(blob);\na.download = '{fname_esc}';\ndocument.body.appendChild(a);\na.click();\nsetTimeout(() => {{ document.body.removeChild(a); URL.revokeObjectURL(a.href); }}, 10000);"#
                                                                             );
                                                                             let _ = eval(&js).await;
                                                                             add_toast(&mut tc, format!("Downloaded {fname}"), ToastType::Info, None);
                                                                         }
                                                                         Err(e) => {
                                                                             error!(error = %e, "Download failed");
                                                                             add_toast(&mut tc, format!("Download failed: {e}"), ToastType::Error, None);
                                                                         }
                                                                     }
                                                                 });
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
        }
    }
}
