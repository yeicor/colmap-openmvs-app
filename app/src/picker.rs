//! Client-side file/folder pickers using the browser's native `<input type="file">` element.
//!
//! These replace the old server-side `rfd` dialogs with in-browser file selection,
//! making the UI consistent across all deployment targets (web, desktop, mobile).

use base64::Engine;
use dioxus::document::eval;

/// Open a native file-picker and return the selected files as `(filename, bytes)` tuples.
///
/// * `accept` — optional comma-separated MIME types or file extensions (e.g. `"image/*"`, `".json"`).
/// * `multiple` — if `true`, the user can select multiple files.
///
/// Returns `Ok(vec![])` if the user cancelled.
pub async fn pick_files(accept: Option<&str>, multiple: bool) -> Vec<(String, Vec<u8>)> {
    let accept_js = accept.unwrap_or("");
    let multiple_js = if multiple { "true" } else { "false" };

    let js = format!(
        r#"
(async function() {{
    const input = document.createElement('input');
    input.type = 'file';
    input.multiple = {multiple_js};
    input.accept = '{accept_js}';
    input.style.display = 'none';
    document.body.appendChild(input);

    const files = await new Promise((resolve) => {{
        input.addEventListener('change', () => resolve(input.files));
        input.addEventListener('cancel', () => resolve(null));
        input.click();
    }});
    document.body.removeChild(input);

    if (!files || files.length === 0) {{
        dioxus.send('__done__');
        return;
    }}

    dioxus.send(String(files.length));

    for (let i = 0; i < files.length; i++) {{
        const f = files[i];
        const buf = await f.arrayBuffer();
        const bytes = new Uint8Array(buf);
        let binary = '';
        for (let j = 0; j < bytes.length; j++) {{
            binary += String.fromCharCode(bytes[j]);
        }}
        dioxus.send(f.name);
        dioxus.send(btoa(binary));
    }}

    dioxus.send('__done__');
}})();
"#
    );

    let mut eval_handle = eval(&js);

    // First message: total count, or "__done__" if cancelled.
    let first = match eval_handle.recv::<String>().await {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    if first == "__done__" {
        return vec![];
    }

    let count: usize = match first.parse() {
        Ok(n) => n,
        Err(_) => return vec![],
    };

    let mut results = Vec::with_capacity(count);
    for _ in 0..count {
        let name = match eval_handle.recv::<String>().await {
            Ok(n) => n,
            Err(_) => break,
        };
        let b64 = match eval_handle.recv::<String>().await {
            Ok(d) => d,
            Err(_) => break,
        };

        match base64::engine::general_purpose::STANDARD.decode(&b64) {
            Ok(bytes) => results.push((name, bytes)),
            Err(_) => continue,
        }
    }

    results
}

/// Open a native file-picker for a single file.
///
/// * `accept` — optional MIME type / extension filter.
///
/// Returns `Ok(None)` if the user cancelled.
pub async fn pick_file(accept: Option<&str>) -> Option<(String, Vec<u8>)> {
    let mut files = pick_files(accept, false).await;
    if files.is_empty() {
        None
    } else {
        Some(files.swap_remove(0))
    }
}

/// Open a native file-picker and upload each selected file **directly** to the
/// server via `fetch()`, bypassing the Dioxus `eval` channel entirely.
///
/// This is **significantly** faster and more memory-efficient than
/// [`pick_files`] + calling the server function, because:
///
/// 1. The file is **never read into WASM memory** — the browser streams it
///    directly from disk via `fetch()`.
/// 2. There is **no base64 encoding/decoding** (no 33% size overhead).
/// 3. The Dioxus `eval` channel is only used for control messages (filenames),
///    not for binary payloads.
/// 4. On supported browsers the file is streamed, avoiding OOM for files
///    larger than available WASM heap.
///
/// * `accept` — optional comma-separated MIME types or file extensions.
/// * `multiple` — if `true`, the user can select multiple files.
/// * `upload_url_prefix` — the base URL for the upload endpoint,
///   e.g. `"/api/projects/myproject/videos/"`.
///
/// Returns the list of filenames that were **successfully** uploaded.
pub async fn upload_files_direct(
    accept: Option<&str>,
    multiple: bool,
    upload_url_prefix: &str,
) -> Vec<String> {
    let accept_js = accept.unwrap_or("");
    let multiple_js = if multiple { "true" } else { "false" };
    let url_prefix = upload_url_prefix;

    let js = format!(
        r#"
(async function() {{
    const input = document.createElement('input');
    input.type = 'file';
    input.multiple = {multiple_js};
    input.accept = '{accept_js}';
    input.style.display = 'none';
    document.body.appendChild(input);

    const files = await new Promise((resolve) => {{
        input.addEventListener('change', () => resolve(input.files));
        input.addEventListener('cancel', () => resolve(null));
        input.click();
    }});
    document.body.removeChild(input);

    if (!files || files.length === 0) {{
        dioxus.send('__done__');
        return;
    }}

    dioxus.send(String(files.length));

    for (let i = 0; i < files.length; i++) {{
        const f = files[i];
        const url = '{url_prefix}' + encodeURIComponent(f.name);

        try {{
            const resp = await fetch(url, {{
                method: 'POST',
                headers: {{ 'Content-Type': 'application/octet-stream' }},
                body: f,
            }});
            const name64 = btoa(unescape(encodeURIComponent(f.name)));
            if (resp.ok) {{
                dioxus.send(f.name);
            }} else {{
                const body = await resp.text().catch(() => '');
                dioxus.send('__fail__:' + name64 + ':' + resp.status + ':' + body.slice(0, 200));
            }}
        }} catch(e) {{
            const name64 = btoa(unescape(encodeURIComponent(f.name)));
            dioxus.send('__fail__:' + name64 + ':0:' + e.message.slice(0, 200));
        }}
    }}

    dioxus.send('__done__');
}})();
"#
    );

    let mut eval_handle = eval(&js);

    let first = match eval_handle.recv::<String>().await {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    if first == "__done__" {
        return vec![];
    }

    let count: usize = match first.parse() {
        Ok(n) => n,
        Err(_) => return vec![],
    };

    let mut uploaded = Vec::with_capacity(count);
    for _ in 0..count {
        let msg = match eval_handle.recv::<String>().await {
            Ok(m) => m,
            Err(_) => break,
        };

        if let Some(rest) = msg.strip_prefix("__fail__:") {
            // rest is "<base64-name>:<status>:<message>"
            let err_parts: Vec<&str> = rest.splitn(3, ':').collect();
            let name_b64 = err_parts.first().unwrap_or(&"");
            let decoded =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, name_b64)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or_else(|| format!("<base64:{}", name_b64));
            tracing::error!("Direct upload failed for '{}': {:?}", decoded, err_parts);
        } else {
            tracing::info!("Direct upload succeeded: {}", msg);
            uploaded.push(msg);
        }
    }

    uploaded
}
