use crate::server::{
    clear_project_outputs, delete_project_output, get_project_output_for_viewer,
    list_project_outputs,
};
use base64::Engine as _;
use colmap_openmvs_api::OutputFile;
use dioxus::document::eval;
use dioxus::prelude::*;
use std::collections::HashSet;
use tracing::{debug, error, info};
use urlencoding::encode as url_encode;

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
fn format_date(unix_secs: u64) -> String {
    if unix_secs == 0 {
        return String::new();
    }
    let now = chrono::Utc::now().timestamp() as u64;
    let diff = now.saturating_sub(unix_secs);
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
        viewable.sort_by_key(|f| &f.relative_path);
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
// Component
// ---------------------------------------------------------------------------

#[component]
pub fn OutputsTab(project_name: String) -> Element {
    debug!(project_name = %project_name, "Initializing outputs tab");

    let mut refresh_counter = use_signal(|| 0u32);
    let mut error_msg = use_signal(String::new);
    let mut viewing = use_signal(|| Option::<String>::None);
    let mut confirming_delete = use_signal(|| Option::<String>::None);
    let mut deleting_path = use_signal(|| Option::<String>::None);
    let mut collapsed = use_signal(HashSet::<String>::new);
    let mut confirming_clear_all = use_signal(|| false);
    let mut clearing_all = use_signal(|| false);

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
        document::Link { rel: "stylesheet", href: asset!("/assets/views/project/outputs.css") }
        div {
            class: "tab-content outputs-tab",

            // ── Toolbar ──────────────────────────────────────────────────
            div {
                class: "outputs-toolbar",
                span { class: "outputs-toolbar-title", "Outputs" }
                div { class: "outputs-toolbar-spacer" }

                button {
                    class: "outputs-btn",
                    title: "Expand all folders",
                    onclick: move |_| collapsed.write().clear(),
                    "⊞"
                    span { class: "btn-label", " Expand All" }
                }

                if clearing_all() {
                    span { class: "outputs-toolbar-status", "⏳" }
                } else if confirming_clear_all() {
                    button {
                        class: "outputs-btn outputs-confirm-del-btn",
                        title: "Confirm delete all",
                        onclick: {
                            let pn = project_name.clone();
                            move |_| {
                                confirming_clear_all.set(false);
                                clearing_all.set(true);
                                let pn = pn.clone();
                                spawn(async move {
                                    match clear_project_outputs(pn.clone()).await {
                                        Ok(()) => {
                                            info!(project_name = %pn, "All outputs cleared");
                                            collapsed.write().clear();
                                            refresh_counter += 1;
                                        }
                                        Err(e) => {
                                            error!(project_name = %pn, error = %e, "Failed to clear");
                                            error_msg.set(format!("Failed to clear all: {e}"));
                                        }
                                    }
                                    clearing_all.set(false);
                                });
                            }
                        },
                        "✓"
                        span { class: "btn-label", " Sure?" }
                    }
                    button {
                        class: "outputs-btn outputs-cancel-del-btn",
                        onclick: move |_| confirming_clear_all.set(false),
                        "✗"
                    }
                } else {
                    button {
                        class: "outputs-btn outputs-del-btn",
                        title: "Delete all outputs",
                        onclick: move |_| confirming_clear_all.set(true),
                        "🗑"
                        span { class: "btn-label", " Delete All" }
                    }
                }

                button {
                    class: "outputs-btn outputs-refresh-btn",
                    onclick: move |_| {
                        debug!("Refresh outputs");
                        refresh_counter += 1;
                    },
                    "↺"
                    span { class: "btn-label", " Refresh" }
                }
            }

            // ── Error banner ──────────────────────────────────────────────
            if !error_msg().is_empty() {
                div {
                    class: "outputs-error-banner",
                    span { "{error_msg}" }
                    button {
                        class: "outputs-error-dismiss",
                        onclick: move |_| error_msg.set(String::new()),
                        "✕"
                    }
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
                                            let is_viewing    = viewing()          == Some(rel_path.clone());
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
                                            let fn_view    = actual_name.clone();
                                            let pn_view    = project_name.clone();
                                            let pn_dl      = project_name.clone();
                                            let rp_dl      = actual_path.clone();
                                            let fname_dl   = actual_name.clone();

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
                                                            if is_collapsed { "▶" } else { "▼" }
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

                                                    // Download (files only)
                                                    if !is_dir {
                                                        button {
                                                            class: "outputs-btn outputs-download-link",
                                                            title: "Download {actual_name}",
                                                            onclick: move |_| {
                                                                let url = format!(
                                                                    "/projects/{}/outputs/file?relative_path={}",
                                                                    pn_dl, url_encode(&rp_dl)
                                                                );
                                                                let fname = fname_dl.clone();
                                                                spawn(async move {
                                                                    let js = format!(
                                                                        "var a=document.createElement('a');\
                                                                         a.href='{}';\
                                                                         a.download='{}';\
                                                                         a.style.display='none';\
                                                                         document.body.appendChild(a);\
                                                                         a.click();\
                                                                         setTimeout(function(){{document.body.removeChild(a);}},100);",
                                                                        js_escape(&url),
                                                                        js_escape(&fname)
                                                                    );
                                                                    let _ = eval(&js).await;
                                                                });
                                                            },
                                                            "⬇"
                                                            span { class: "btn-label", " Download" }
                                                        }
                                                    }

                                                    // 3D View (viewable files only)
                                                    if is_viewable {
                                                        button {
                                                            class: "outputs-btn outputs-view-3d-btn",
                                                            disabled: is_viewing,
                                                            onclick: move |_| {
                                                                let pn = pn_view.clone();
                                                                let rp = rp_view.clone();
                                                                let fn_ = fn_view.clone();
                                                                debug!(file = %fn_, "Opening 3D viewer");
                                                                viewing.set(Some(rel_path.clone()));
                                                                let mut err = error_msg;
                                                                spawn(async move {
                                                                    match get_project_output_for_viewer(pn.clone(), rp.clone()).await {
                                                                        Ok(bytes) => {
                                                                            info!(file = %fn_, bytes = bytes.len(), "Loaded for 3D viewer");
                                                                            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                                                                            let fname_safe = js_escape(&fn_);
                                                                            launch_glb_viewer(&b64, &fname_safe).await;
                                                                        }
                                                                        Err(e) => {
                                                                            error!(file = %fn_, error = %e, "Viewer load failed");
                                                                            err.set(format!("Failed to load viewer data: {e}"));
                                                                        }
                                                                    }
                                                                    viewing.set(None);
                                                                });
                                                            },
                                                            if is_viewing { "⏳" } else { "🔳" }
                                                            span {
                                                                class: "btn-label",
                                                                if is_viewing { " Loading…" } else { " View 3D" }
                                                            }
                                                        }
                                                    }

                                                    // Delete — hidden for virtual entries
                                                    if !is_virtual {
                                                        if is_deleting {
                                                            span { class: "outputs-entry-icon", "⏳" }
                                                        } else if is_confirming {
                                                            button {
                                                                class: "outputs-btn outputs-confirm-del-btn",
                                                                onclick: move |_| {
                                                                    let pn = pn_del.clone();
                                                                    let rp = rp_del.clone();
                                                                    deleting_path.set(Some(rp.clone()));
                                                                    confirming_delete.set(None);
                                                                    spawn(async move {
                                                                        match delete_project_output(pn.clone(), rp.clone()).await {
                                                                            Ok(()) => {
                                                                                info!(path = %rp, "Deleted");
                                                                                collapsed.write().remove(&rp);
                                                                                refresh_counter += 1;
                                                                            }
                                                                            Err(e) => {
                                                                                error!(path = %rp, error = %e, "Delete failed");
                                                                                error_msg.set(format!("Failed to delete: {e}"));
                                                                            }
                                                                        }
                                                                        deleting_path.set(None);
                                                                    });
                                                                },
                                                                "✓"
                                                                span {
                                                                    class: "btn-label",
                                                                    if is_dir { " Sure?" } else { " Sure?" }
                                                                }
                                                            }
                                                            button {
                                                                class: "outputs-btn outputs-cancel-del-btn",
                                                                onclick: move |_| confirming_delete.set(None),
                                                                "✗"
                                                            }
                                                        } else {
                                                            button {
                                                                class: "outputs-btn outputs-del-btn",
                                                                title: if is_dir { "Delete folder" } else { "Delete file" },
                                                                onclick: move |_| confirming_delete.set(Some(rp_confirm.clone())),
                                                                "🗑"
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

// ---------------------------------------------------------------------------
// 3-D Viewer (launched via eval'd JavaScript)
// ---------------------------------------------------------------------------

async fn launch_glb_viewer(b64: &str, fname_safe: &str) {
    info!(file_name = %fname_safe, "Launching 3D GLB viewer");
    let b64_esc = js_escape(b64);
    let fname_esc = js_escape(fname_safe);

    // asset!() with with_minify(false) → dx copies these files verbatim without
    // invoking esbuild (which would ERROR on the bare 'three' import). The
    // importmap injected in App resolves 'three' and the BufferGeometryUtils
    // relative import at runtime.
    let gltf_url = asset!(
        "/assets/lib/three/GLTFLoader.js",
        AssetOptions::js().with_minify(false)
    )
    .to_string();
    let trackball_url = asset!(
        "/assets/lib/three/TrackballControls.js",
        AssetOptions::js().with_minify(false)
    )
    .to_string();

    let js = format!(
        r#"(async () => {{
    console.log('[3D Viewer] Starting viewer setup...');
    try {{
        // three.js and its addons are vendored locally by build.rs.
        // 'three' is resolved to the local asset via the importmap in App.
        // GLTFLoader/TrackballControls are loaded from their asset!() URLs.
        const THREE = await import('three');
        const {{ GLTFLoader }} = await import('{gltf_url}');
        const {{ TrackballControls }} = await import('{trackball_url}');
        console.log('[3D Viewer] Libraries loaded');

        const b64 = '{}';
        const fname = '{}';

        // Decode GLB bytes and create a blob URL
        const binary = atob(b64);
        const arr = new Uint8Array(binary.length);
        for (let i = 0; i < binary.length; i++) arr[i] = binary.charCodeAt(i);
        const blob = new Blob([arr], {{type: 'model/gltf-binary'}});
        const blobUrl = URL.createObjectURL(blob);

        // Remove any existing overlay
        const existing = document.getElementById('ply-viewer-overlay');
        if (existing) existing.remove();

        // Overlay container
        const overlay = document.createElement('div');
        overlay.id = 'ply-viewer-overlay';
        overlay.style.cssText = 'position:fixed;inset:0;background:#0d1117;z-index:9999;display:flex;flex-direction:column;align-items:stretch;';
        document.body.appendChild(overlay);

        // Header bar
        const headerDiv = document.createElement('div');
        headerDiv.style.cssText = 'display:flex;align-items:center;padding:8px 16px;background:#161b22;border-bottom:1px solid #30363d;gap:12px;flex-shrink:0;';
        const titleSpan = document.createElement('span');
        titleSpan.style.cssText = 'color:#e6edf3;font-family:monospace;font-size:14px;flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;';
        titleSpan.textContent = '3D Viewer — ' + fname;
        const closeBtn = document.createElement('button');
        closeBtn.textContent = '✕ Close';
        closeBtn.style.cssText = 'padding:6px 14px;background:#21262d;color:#e6edf3;border:1px solid #30363d;border-radius:6px;cursor:pointer;font-size:13px;flex-shrink:0;';
        closeBtn.onclick = () => {{ URL.revokeObjectURL(blobUrl); overlay.remove(); }};
        headerDiv.appendChild(titleSpan);
        headerDiv.appendChild(closeBtn);
        overlay.appendChild(headerDiv);

        // Controls bar — reset button always present; dynamic slider added after PLY loads
        const controlsDiv = document.createElement('div');
        controlsDiv.style.cssText = 'display:flex;align-items:center;gap:12px;padding:8px 16px;background:#161b22;border-bottom:1px solid #30363d;flex-wrap:wrap;flex-shrink:0;';
        const resetBtn = document.createElement('button');
        resetBtn.textContent = 'Reset View';
        resetBtn.style.cssText = 'padding:4px 12px;background:#21262d;color:#e6edf3;border:1px solid #30363d;border-radius:4px;cursor:pointer;font-size:12px;';
        controlsDiv.appendChild(resetBtn);
        overlay.appendChild(controlsDiv);

        // Loading message
        const loadingDiv = document.createElement('div');
        loadingDiv.id = 'ply-viewer-overlay-loading';
        loadingDiv.style.cssText = 'color:#8b949e;font-family:monospace;font-size:13px;padding:24px;text-align:center;flex-shrink:0;';
        loadingDiv.textContent = 'Initializing 3D viewer...';
        overlay.appendChild(loadingDiv);

        // Canvas
        const canvas = document.createElement('canvas');
        canvas.id = 'ply-viewer-canvas';
        canvas.style.cssText = 'flex:1;display:block;min-height:0;width:100%;height:100%;';
        overlay.appendChild(canvas);

        await new Promise(r => setTimeout(r, 0));
        let w = canvas.clientWidth || window.innerWidth;
        let h = canvas.clientHeight || (window.innerHeight - 100);

        // Scene
        const scene = new THREE.Scene();
        scene.background = new THREE.Color(0x0d1117);
        const camera = new THREE.PerspectiveCamera(60, w / h, 0.001, 10000);
        camera.position.set(0, 0, 5);
        scene.add(camera);

        const renderer = new THREE.WebGLRenderer({{ canvas, antialias: true, preserveDrawingBuffer: true }});
        renderer.setSize(w, h, false);
        renderer.setPixelRatio(window.devicePixelRatio || 1);
        renderer.setClearColor(0x0d1117);
        if (THREE.SRGBColorSpace) renderer.outputColorSpace = THREE.SRGBColorSpace;

        const controls = new TrackballControls(camera, renderer.domElement);
        controls.rotateSpeed = 2.5; controls.zoomSpeed = 1.2; controls.panSpeed = 0.8;

        // Inertia
        let rotVel = new THREE.Vector3();
        let isRotating = false, lastX = 0, lastY = 0;
        renderer.domElement.addEventListener('mousedown', () => {{ isRotating = true; rotVel.set(0,0,0); }});
        renderer.domElement.addEventListener('mouseup',   () => {{ isRotating = false; }});
        renderer.domElement.addEventListener('mousemove', e => {{
            if (isRotating) {{ rotVel.x = (e.clientY-lastY)*0.001; rotVel.y = (e.clientX-lastX)*0.001; }}
            lastX = e.clientX; lastY = e.clientY;
        }});

        // Lighting: ambient + directional attached to camera
        scene.add(new THREE.AmbientLight(0xffffff, 0.4));
        const dl = new THREE.DirectionalLight(0xffffff, 1.0);
        dl.position.set(0, 0.5, 1);
        camera.add(dl);
        const dlTarget = new THREE.Object3D();
        dlTarget.position.set(0, 0, -1);
        camera.add(dlTarget);
        dl.target = dlTarget;

        loadingDiv.textContent = 'Loading 3D model...';
        const loader = new GLTFLoader();
        const initialCamPos = new THREE.Vector3();

        loader.load(blobUrl, (gltf) => {{
            try {{
                loadingDiv.remove();
                const model = gltf.scene;

                // Post-process meshes and detect point clouds
                let hasPoints = false;
                let hasMesh = false;
                model.traverse(child => {{
                    if (child.isPoints) {{
                        hasPoints = true;
                        // Override with a PointsMaterial that respects vertex colors
                        const hasVCol = !!child.geometry.attributes.color;
                        child.material = new THREE.PointsMaterial({{
                            vertexColors: hasVCol,
                            color: hasVCol ? 0xffffff : 0x4fc3f7,
                            size: 0.1,
                            sizeAttenuation: true,
                        }});
                    }}
                    if (child.isMesh) {{
                        hasMesh = true;
                        child.material.side = THREE.DoubleSide;
                    }}
                }});

                scene.add(model);
                console.log('[3D Viewer] Model added — hasPoints:', hasPoints, 'hasMesh:', hasMesh);

                // Fit camera to bounding box
                const box = new THREE.Box3().setFromObject(model);
                const center = box.getCenter(new THREE.Vector3());
                const size = box.getSize(new THREE.Vector3());
                const maxDim = Math.max(size.x, size.y, size.z) || 1;
                model.position.sub(center);
                camera.position.set(0, 0, maxDim * 2.5);
                initialCamPos.copy(camera.position);
                camera.near = maxDim * 0.0005;
                camera.far  = maxDim * 200;
                camera.updateProjectionMatrix();

                // Dynamic slider
                if (hasPoints) {{
                    const scaleLabel = document.createElement('label');
                    scaleLabel.style.cssText = 'color:#e6edf3;font-family:monospace;font-size:12px;display:flex;align-items:center;gap:8px;';
                    scaleLabel.textContent = 'Point Scale:';
                    const scaleSlider = document.createElement('input');
                    scaleSlider.type='range'; scaleSlider.min='0.1'; scaleSlider.max='5'; scaleSlider.step='0.1'; scaleSlider.value='1';
                    scaleSlider.style.cssText = 'width:120px;cursor:pointer;';
                    const scaleVal = document.createElement('span');
                    scaleVal.style.cssText = 'color:#8b949e;font-family:monospace;font-size:12px;min-width:30px;';
                    scaleVal.textContent = '1.0x';
                    scaleSlider.oninput = e => {{
                        const s = parseFloat(e.target.value);
                        scaleVal.textContent = s.toFixed(1)+'x';
                        model.traverse(c => {{ if (c.isPoints) c.material.size = 0.1 * s; }});
                    }};
                    scaleLabel.appendChild(scaleSlider);
                    scaleLabel.appendChild(scaleVal);
                    controlsDiv.insertBefore(scaleLabel, resetBtn);
                }} else {{
                    const yawLabel = document.createElement('label');
                    yawLabel.style.cssText = 'color:#e6edf3;font-family:monospace;font-size:12px;display:flex;align-items:center;gap:8px;';
                    yawLabel.textContent = 'Light Angle:';
                    const yawSlider = document.createElement('input');
                    yawSlider.type='range'; yawSlider.min='-180'; yawSlider.max='180'; yawSlider.step='5'; yawSlider.value='0';
                    yawSlider.style.cssText = 'width:120px;cursor:pointer;';
                    const yawVal = document.createElement('span');
                    yawVal.style.cssText = 'color:#8b949e;font-family:monospace;font-size:12px;min-width:36px;';
                    yawVal.textContent = '0\u00b0';
                    yawSlider.oninput = e => {{
                        const yaw = parseFloat(e.target.value) * Math.PI / 180;
                        yawVal.textContent = e.target.value + '\u00b0';
                        dl.position.set(Math.sin(yaw), 0.5, Math.cos(yaw));
                    }};
                    yawLabel.appendChild(yawSlider);
                    yawLabel.appendChild(yawVal);
                    controlsDiv.insertBefore(yawLabel, resetBtn);
                }}

                resetBtn.onclick = () => {{
                    camera.position.copy(initialCamPos);
                    controls.reset();
                    rotVel.set(0,0,0);
                }};

                const ro = new ResizeObserver(() => {{
                    const nw = canvas.clientWidth, nh = canvas.clientHeight;
                    if (nw > 0 && nh > 0) {{
                        camera.aspect = nw / nh;
                        camera.updateProjectionMatrix();
                        renderer.setSize(nw, nh, false);
                    }}
                }});
                ro.observe(canvas);

                const origClose = closeBtn.onclick;
                closeBtn.onclick = () => {{ URL.revokeObjectURL(blobUrl); ro.disconnect(); renderer.dispose(); origClose(); }};

                const animate = () => {{
                    if (!document.contains(overlay)) {{ URL.revokeObjectURL(blobUrl); ro.disconnect(); renderer.dispose(); return; }}
                    requestAnimationFrame(animate);
                    if (!isRotating && (Math.abs(rotVel.x) > 0.001 || Math.abs(rotVel.y) > 0.001)) {{
                        const qy = new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(0,1,0), rotVel.y);
                        const qx = new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(1,0,0), rotVel.x);
                        camera.position.applyQuaternion(new THREE.Quaternion().multiplyQuaternions(qy, qx));
                        rotVel.multiplyScalar(0.95);
                    }}
                    controls.update();
                    renderer.render(scene, camera);
                }};
                animate();
            }} catch (innerErr) {{
                console.error('[3D Viewer] Error processing model:', innerErr);
                const ld = document.getElementById('ply-viewer-overlay-loading');
                if (ld) {{ ld.style.color='#f85149'; ld.textContent='Error: '+(innerErr.message||'Failed to process model'); }}
            }}
        }}, undefined, err => {{
            console.error('[3D Viewer] Error loading GLB:', err);
            const ld = document.getElementById('ply-viewer-overlay-loading');
            if (ld) {{ ld.style.color='#f85149'; ld.textContent='Error: '+(err.message||'Failed to load model'); }}
        }});

    }} catch (err) {{
        console.error('[3D Viewer] Fatal error:', err.stack || err);
        const ld = document.getElementById('ply-viewer-overlay-loading');
        if (ld) {{ ld.style.color='#f85149'; ld.textContent='Error: '+(err.message||'Failed to initialize'); }}
    }}
}})();"#,
        b64_esc, fname_esc
    );

    if let Err(e) = eval(&js).await {
        let err_msg = format!("Failed to execute 3D viewer: {:?}", e);
        let _ = eval(&format!(
            "console.error('{}');",
            err_msg.replace('"', "\\\"")
        ))
        .await;
    }
}
