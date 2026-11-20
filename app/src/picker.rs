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

/// Result of a sequential pick-and-process operation.
#[derive(Debug, Default)]
pub struct PickResult {
    /// Number of files the user selected.
    pub selected: usize,
    /// Files that were read and processed successfully.
    pub succeeded: Vec<String>,
    /// Human-readable errors for files that could not be read or processed.
    pub errors: Vec<String>,
}

/// Decode an `__error__:<url-encoded-name>:<message>` payload sent by the JS
/// picker when a file could not be read.
fn parse_picker_error(rest: &str) -> (String, String) {
    let mut parts = rest.splitn(2, ':');
    let name = parts
        .next()
        .and_then(|n| urlencoding::decode(n).ok())
        .map(|n| n.into_owned())
        .unwrap_or_else(|| "unknown file".to_string());
    let message = parts.next().unwrap_or("read failed").to_string();
    (name, message)
}

/// Open a native file-picker and process each selected file **one at a time**.
///
/// Unlike [`pick_files`], this never holds more than one file in memory at a
/// time: the JavaScript side waits for an acknowledgement from Rust before it
/// reads and sends the next file.  This is essential on Android, where reading
/// several high-resolution photos into the WebView at once exhausts the
/// renderer's memory (and can make later reads fail with `NotReadableError`).
///
/// Files are also encoded with `FileReader.readAsDataURL`, which performs the
/// base64 conversion natively instead of building a giant JavaScript string one
/// character at a time.
///
/// The `on_file` callback receives `(filename, bytes)` and returns `Ok(())` on
/// success or an error message to report.  A failure for one file does not stop
/// the remaining files from being processed.
pub async fn pick_files_sequential<F, Fut>(
    accept: Option<&str>,
    multiple: bool,
    mut on_file: F,
) -> PickResult
where
    F: FnMut(String, Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    // NOTE: `document::eval` treats the script as the *body* of an async
    // function (Dioxus wraps it and closes the channel when the returned
    // promise resolves).  Keep this as a plain body: wrapping it in an
    // immediately-invoked async function would make the eval resolve right
    // away, after which `Eval::send` (our acknowledgements) fails because the
    // JS side of the channel is already closed.
    const JS_TEMPLATE: &str = r#"
    const acceptFilter = '__ACCEPT__';
    const multipleFlag = __MULTIPLE__;

    function fileToBase64(file) {
        return new Promise(function(resolve, reject) {
            const reader = new FileReader();
            reader.onload = function() {
                const s = String(reader.result);
                const comma = s.indexOf(',');
                resolve(comma >= 0 ? s.slice(comma + 1) : '');
            };
            reader.onerror = function() {
                reject(reader.error || new Error('file read failed'));
            };
            reader.readAsDataURL(file);
        });
    }

    const input = document.createElement('input');
    input.type = 'file';
    input.multiple = multipleFlag;
    if (acceptFilter) input.accept = acceptFilter;
    input.style.display = 'none';
    document.body.appendChild(input);

    const picked = await new Promise(function(resolve) {
        input.addEventListener('change', function() { resolve(input.files); });
        input.addEventListener('cancel', function() { resolve(null); });
        input.click();
    });
    document.body.removeChild(input);

    if (!picked || picked.length === 0) {
        dioxus.send('__done__');
        return;
    }

    dioxus.send(String(picked.length));

    for (let i = 0; i < picked.length; i++) {
        const file = picked[i];
        let b64;
        try {
            b64 = await fileToBase64(file);
        } catch (e) {
            const msg = (e && e.message) ? e.message : String(e);
            dioxus.send('__error__:' + encodeURIComponent(file.name) + ':' + msg.slice(0, 200));
            await dioxus.recv();
            continue;
        }
        dioxus.send('__name__:' + encodeURIComponent(file.name));
        dioxus.send(b64);
        await dioxus.recv();
    }

    dioxus.send('__done__');
"#;

    let js = JS_TEMPLATE
        .replace("__ACCEPT__", &js_escape(accept.unwrap_or("")))
        .replace("__MULTIPLE__", if multiple { "true" } else { "false" });

    let mut eval_handle = eval(&js);

    let first = match eval_handle.recv::<String>().await {
        Ok(s) => s,
        Err(_) => return PickResult::default(),
    };

    if first == "__done__" {
        return PickResult::default();
    }

    let selected: usize = match first.parse() {
        Ok(n) => n,
        Err(_) => return PickResult::default(),
    };

    let mut result = PickResult {
        selected,
        succeeded: Vec::with_capacity(selected),
        errors: Vec::new(),
    };

    for _ in 0..selected {
        let msg = match eval_handle.recv::<String>().await {
            Ok(m) => m,
            Err(e) => {
                result.errors.push(format!(
                    "Upload channel closed unexpectedly ({} pending)",
                    selected - result.succeeded.len() - result.errors.len()
                ));
                tracing::error!("Eval channel error during file pick: {:?}", e);
                break;
            }
        };

        if msg == "__done__" {
            break;
        }

        if let Some(rest) = msg.strip_prefix("__error__:") {
            let (name, message) = parse_picker_error(rest);
            tracing::error!("Failed to read picked file '{}': {}", name, message);
            result.errors.push(format!("{name}: {message}"));
            let _ = eval_handle.send("__ack__");
            continue;
        }

        let name = match msg.strip_prefix("__name__:") {
            Some(encoded) => urlencoding::decode(encoded)
                .map(|n| n.into_owned())
                .unwrap_or_else(|_| encoded.to_string()),
            None => {
                result
                    .errors
                    .push(format!("Unexpected picker message: {msg}"));
                let _ = eval_handle.send("__ack__");
                break;
            }
        };
        let b64 = match eval_handle.recv::<String>().await {
            Ok(b) => b,
            Err(e) => {
                result
                    .errors
                    .push(format!("{name}: upload channel closed unexpectedly: {e}"));
                break;
            }
        };

        match base64::engine::general_purpose::STANDARD.decode(&b64) {
            Ok(bytes) => match on_file(name.clone(), bytes).await {
                Ok(()) => result.succeeded.push(name),
                Err(e) => {
                    tracing::error!("Failed to import picked file '{}': {}", name, e);
                    result.errors.push(format!("{name}: {e}"));
                }
            },
            Err(e) => {
                tracing::error!("Failed to decode picked file '{}': {}", name, e);
                result
                    .errors
                    .push(format!("{name}: base64 decode failed: {e}"));
            }
        }

        let _ = eval_handle.send("__ack__");
    }

    result
}

/// Open a native file-picker and stream each selected file to `on_file` in
/// chunks.
///
/// This is the native (desktop / Android) counterpart to
/// [`upload_files_direct`]: there is no TCP server to stream a `fetch()` body
/// to, so the bytes have to travel through the Dioxus eval channel.  Sending a
/// whole video at once can be hundreds of megabytes of base64; instead, the
/// JavaScript side reads fixed-size slices and waits for an acknowledgement
/// after each one.  The Rust side feeds the chunks into the [`ByteStream`]
/// passed to `on_file`, which can be handed straight to a server function.
///
/// `on_file` receives `(filename, expected_size, body)`.  The expected size
/// lets the receiver detect a truncated upload if a chunk cannot be read.
#[cfg(not(target_arch = "wasm32"))]
pub async fn pick_files_chunked<F, Fut>(
    accept: Option<&str>,
    multiple: bool,
    mut on_file: F,
) -> PickResult
where
    F: FnMut(String, u64, crate::fullstack_compat::ByteStream) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    use futures::SinkExt as _;

    // NOTE: `document::eval` treats the script as the *body* of an async
    // function (Dioxus wraps it and closes the channel when the returned
    // promise resolves).  Keep this as a plain body: wrapping it in an
    // immediately-invoked async function would make the eval resolve right
    // away, after which `Eval::send` (our acknowledgements) fails because the
    // JS side of the channel is already closed.
    const JS_TEMPLATE: &str = r#"
    const acceptFilter = '__ACCEPT__';
    const multipleFlag = __MULTIPLE__;
    const CHUNK_SIZE = 512 * 1024;

    function blobToBase64(blob) {
        return new Promise(function(resolve, reject) {
            const reader = new FileReader();
            reader.onload = function() {
                const s = String(reader.result);
                const comma = s.indexOf(',');
                resolve(comma >= 0 ? s.slice(comma + 1) : '');
            };
            reader.onerror = function() {
                reject(reader.error || new Error('file read failed'));
            };
            reader.readAsDataURL(blob);
        });
    }

    const input = document.createElement('input');
    input.type = 'file';
    input.multiple = multipleFlag;
    if (acceptFilter) input.accept = acceptFilter;
    input.style.display = 'none';
    document.body.appendChild(input);

    const picked = await new Promise(function(resolve) {
        input.addEventListener('change', function() { resolve(input.files); });
        input.addEventListener('cancel', function() { resolve(null); });
        input.click();
    });
    document.body.removeChild(input);

    if (!picked || picked.length === 0) {
        dioxus.send('__done__');
        return;
    }

    dioxus.send(String(picked.length));

    for (let i = 0; i < picked.length; i++) {
        const file = picked[i];
        dioxus.send('__file__');
        dioxus.send(file.name);
        dioxus.send(String(file.size));

        let failed = false;
        for (let offset = 0; offset < file.size; offset += CHUNK_SIZE) {
            let b64;
            try {
                const end = Math.min(offset + CHUNK_SIZE, file.size);
                b64 = await blobToBase64(file.slice(offset, end));
            } catch (e) {
                const msg = (e && e.message) ? e.message : String(e);
                dioxus.send('__error__:' + encodeURIComponent(file.name) + ':' + msg.slice(0, 200));
                failed = true;
                break;
            }
            dioxus.send(b64);
            await dioxus.recv();
        }

        if (!failed) dioxus.send('__eof__');
        await dioxus.recv();
    }

    dioxus.send('__done__');
"#;

    let js = JS_TEMPLATE
        .replace("__ACCEPT__", &js_escape(accept.unwrap_or("")))
        .replace("__MULTIPLE__", if multiple { "true" } else { "false" });

    let mut eval_handle = eval(&js);

    let first = match eval_handle.recv::<String>().await {
        Ok(s) => s,
        Err(_) => return PickResult::default(),
    };

    if first == "__done__" {
        return PickResult::default();
    }

    let selected: usize = match first.parse() {
        Ok(n) => n,
        Err(_) => return PickResult::default(),
    };

    let mut result = PickResult {
        selected,
        succeeded: Vec::with_capacity(selected),
        errors: Vec::new(),
    };

    for _ in 0..selected {
        let marker = match eval_handle.recv::<String>().await {
            Ok(m) => m,
            Err(e) => {
                result
                    .errors
                    .push(format!("Upload channel closed unexpectedly: {e}"));
                break;
            }
        };

        if marker == "__done__" {
            break;
        }

        if marker != "__file__" {
            result
                .errors
                .push(format!("Unexpected picker message: {marker}"));
            break;
        }

        let name = match eval_handle.recv::<String>().await {
            Ok(n) => n,
            Err(e) => {
                result
                    .errors
                    .push(format!("Upload channel closed unexpectedly: {e}"));
                break;
            }
        };

        let expected_size = match eval_handle.recv::<String>().await {
            Ok(s) => match s.parse::<u64>() {
                Ok(n) => n,
                Err(e) => {
                    result
                        .errors
                        .push(format!("{name}: invalid file size '{s}': {e}"));
                    break;
                }
            },
            Err(e) => {
                result
                    .errors
                    .push(format!("{name}: upload channel closed unexpectedly: {e}"));
                break;
            }
        };

        let (mut tx, rx) =
            futures::channel::mpsc::channel::<crate::fullstack_compat::body::Bytes>(4);
        let upload = on_file(
            name.clone(),
            expected_size,
            crate::fullstack_compat::ByteStream::new(rx),
        );

        let pump = async {
            let mut file_error: Option<String> = None;
            loop {
                let msg = match eval_handle.recv::<String>().await {
                    Ok(m) => m,
                    Err(e) => {
                        file_error = Some(format!("upload channel closed unexpectedly: {e}"));
                        break;
                    }
                };

                if msg == "__eof__" {
                    break;
                }

                if let Some(rest) = msg.strip_prefix("__error__:") {
                    let (_, message) = parse_picker_error(rest);
                    file_error = Some(message);
                    break;
                }

                match base64::engine::general_purpose::STANDARD.decode(&msg) {
                    Ok(bytes) => {
                        // If the upload future has already failed, keep
                        // draining the file so the JS side does not stall on
                        // an acknowledgement that will never come.
                        let _ = tx
                            .send(crate::fullstack_compat::body::Bytes::from(bytes))
                            .await;
                    }
                    Err(e) => {
                        file_error = Some(format!("base64 decode failed: {e}"));
                        break;
                    }
                }

                let _ = eval_handle.send("__ack__");
            }
            drop(tx);
            file_error
        };

        let (upload_result, file_error) = futures::future::join(upload, pump).await;

        // Acknowledge the end of this file so the picker moves on to the next.
        let _ = eval_handle.send("__ack__");

        if let Some(err) = file_error {
            result.errors.push(format!("{name}: {err}"));
        } else if let Err(err) = upload_result {
            result.errors.push(format!("{name}: {err}"));
        } else {
            result.succeeded.push(name);
        }
    }

    result
}

/// Escape a string for embedding in a JS single-quoted string literal.
fn js_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
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
/// Heartbeat messages are sent periodically during upload to keep the Dioxus
/// eval WebSocket/channel alive, preventing timeouts during large file uploads.
///
/// * `accept` — optional comma-separated MIME types or file extensions.
/// * `multiple` — if `true`, the user can select multiple files.
/// * `upload_url_prefix` — the base URL for the upload endpoint,
///   e.g. `"/api/projects/myproject/videos/"`.
///
/// Returns `(successful_names, error_messages)`.
pub async fn upload_files_direct(
    accept: Option<&str>,
    multiple: bool,
    upload_url_prefix: &str,
) -> (Vec<String>, Vec<String>) {
    let accept_js = accept.unwrap_or("");
    let multiple_js = if multiple { "true" } else { "false" };
    let url_prefix_escaped = js_escape(upload_url_prefix);

    let js = format!(
        r#"
(async function() {{
    const acceptFilter = '{accept_js}';
    const multipleFlag = {multiple_js};
    const urlPrefix = '{url_prefix_escaped}';

    // ── Open file picker ────────────────────────────────────────────
    const input = document.createElement('input');
    input.type = 'file';
    input.multiple = multipleFlag;
    if (acceptFilter) input.accept = acceptFilter;
    input.style.display = 'none';
    document.body.appendChild(input);

    const picked = await new Promise((resolve) => {{
        input.addEventListener('change', () => resolve(input.files));
        input.addEventListener('cancel', () => resolve(null));
        input.click();
    }});
    document.body.removeChild(input);

    if (!picked || picked.length === 0) {{
        dioxus.send('__done__');
        return;
    }}

    dioxus.send(String(picked.length));

    // ── Upload each file ─────────────────────────────────────────────
    for (let i = 0; i < picked.length; i++) {{
        const file = picked[i];
        const url = urlPrefix + encodeURIComponent(file.name);

        // Start a heartbeat interval to keep the eval channel alive.
        const heartbeat = setInterval(() => {{
            dioxus.send('__heartbeat__');
        }}, 15000); // every 15 seconds

        let success = false;
        try {{
            const resp = await fetch(url, {{
                method: 'POST',
                headers: {{ 'Content-Type': 'application/octet-stream' }},
                body: file,
            }});
            const name64 = btoa(unescape(encodeURIComponent(file.name)));
            if (resp.ok) {{
                dioxus.send(file.name);
                success = true;
            }} else {{
                const body = await resp.text().catch(() => '');
                dioxus.send('__fail__:' + name64 + ':' + resp.status + ':' + body.slice(0, 200));
            }}
        }} catch(e) {{
            clearInterval(heartbeat);
            const name64 = btoa(unescape(encodeURIComponent(file.name)));
            dioxus.send('__fail__:' + name64 + ':0:' + e.message.slice(0, 200));
        }}
        clearInterval(heartbeat);
    }}

    dioxus.send('__done__');
}})();
"#
    );

    let mut eval_handle = eval(&js);

    let first = match eval_handle.recv::<String>().await {
        Ok(s) => s,
        Err(_) => return (vec![], vec![]),
    };

    if first == "__done__" {
        return (vec![], vec![]);
    }

    let count: usize = match first.parse() {
        Ok(n) => n,
        Err(_) => return (vec![], vec![]),
    };

    let mut uploaded = Vec::with_capacity(count);
    let mut errors = Vec::new();
    let mut remaining = count;

    while remaining > 0 {
        let msg = match eval_handle.recv::<String>().await {
            Ok(m) => m,
            Err(e) => {
                tracing::error!("Eval channel error during upload: {:?}", e);
                // Eval channel died — add any remaining files as errors
                if remaining > 0 {
                    errors.push(format!(
                        "Upload channel closed unexpectedly ({} pending)",
                        remaining
                    ));
                }
                break;
            }
        };

        // Handle heartbeat messages (keep-alive pings from JS)
        if msg == "__heartbeat__" {
            continue;
        }

        if msg == "__done__" {
            break;
        }

        if let Some(rest) = msg.strip_prefix("__fail__:") {
            // rest is "<base64-name>:<status>:<message>"
            let err_parts: Vec<&str> = rest.splitn(3, ':').collect();
            let name_b64 = err_parts.first().unwrap_or(&"");
            let decoded =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, name_b64)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or_else(|| format!("<base64:{}", name_b64));
            let status = err_parts.get(1).copied().unwrap_or("?");
            let message = err_parts.get(2).copied().unwrap_or("unknown error");
            tracing::error!(
                "Direct upload failed for '{}': status={}, msg={}",
                decoded,
                status,
                message
            );
            errors.push(format!("{}: {}", decoded, message));
            remaining -= 1;
        } else {
            tracing::info!("Direct upload succeeded: {}", msg);
            uploaded.push(msg);
            remaining -= 1;
        }
    }

    (uploaded, errors)
}
