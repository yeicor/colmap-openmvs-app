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
