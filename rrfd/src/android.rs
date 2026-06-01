//! Android file-picker/saver via JNI.
//!
//! Calls `MainActivity.startFilePicker(mimeType, multiple)` and
//! `MainActivity.startFileSaver(mimeType, suggestedName)` which trigger
//! Android intents.  Results are delivered back to Rust through the
//! `onFilesPickerResult` and `onSaveFileResult` JNI callbacks.
//!
//! The Kotlin counterpart is in `android/app/src/main/kotlin/dev/dioxus/main/MainActivity.kt`.

use crate::{Error, FileFilter, PickedFile};
use jni::objects::{GlobalRef, JClass, JObject, JObjectArray, JString, JValue};
use jni::sys::{jint, jintArray};
use jni::JNIEnv;
use std::os::fd::FromRawFd;
use std::sync::RwLock;
use tokio::sync::mpsc;
use tracing::warn;

// ── Global state ─────────────────────────────────────────────────────────────

static JAVA_VM: RwLock<Option<std::sync::Arc<jni::JavaVM>>> = RwLock::new(None);
static ACTIVITY_REF: RwLock<Option<GlobalRef>> = RwLock::new(None);

// Channel senders – at most one outstanding picker/saver at a time.
static PICK_TX: RwLock<Option<mpsc::Sender<Vec<PickedFile>>>> = RwLock::new(None);
static SAVE_TX: RwLock<Option<mpsc::Sender<Result<(), Error>>>> = RwLock::new(None);

/// Called once at app startup to cache the JVM and activity reference.
/// Invoke from the Rust main entry-point or JNI_OnLoad.
pub fn initialize(vm: std::sync::Arc<jni::JavaVM>, activity: GlobalRef) {
    *JAVA_VM.write().unwrap() = Some(vm);
    *ACTIVITY_REF.write().unwrap() = Some(activity);
}

// ── Public API ────────────────────────────────────────────────────────────────

pub async fn pick_files(filter: FileFilter, multiple: bool) -> Result<Vec<PickedFile>, Error> {
    let mime = extensions_to_mime(filter.extensions);
    let (tx, mut rx) = mpsc::channel(1);
    *PICK_TX.write().unwrap() = Some(tx);
    call_activity_start_file_picker(&mime, multiple)?;
    rx.recv()
        .await
        .ok_or_else(|| Error::Platform("picker channel closed".into()))
}

pub async fn save_file(default_name: &str, data: Vec<u8>) -> Result<(), Error> {
    let mime = "application/octet-stream";
    let (tx, mut rx) = mpsc::channel(1);

    // Store the data before kicking off the intent so the JNI callback can
    // pick it up immediately when the fd arrives.
    PENDING_SAVE_DATA.with(|cell| *cell.borrow_mut() = Some(data));

    *SAVE_TX.write().unwrap() = Some(tx);
    call_activity_start_file_saver(mime, default_name)?;

    rx.recv()
        .await
        .ok_or_else(|| Error::Platform("saver channel closed".into()))?
}

thread_local! {
    static PENDING_SAVE_DATA: std::cell::RefCell<Option<Vec<u8>>> = std::cell::RefCell::new(None);
}

// ── JNI call helpers ──────────────────────────────────────────────────────────

fn with_env<F, R>(f: F) -> Result<R, Error>
where
    F: FnOnce(&mut JNIEnv<'_>) -> Result<R, Error>,
{
    let vm = JAVA_VM
        .read()
        .unwrap()
        .clone()
        .ok_or_else(|| Error::Platform("JVM not initialised".into()))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| Error::Platform(format!("attach_current_thread: {e}")))?;
    f(&mut env)
}

fn activity_obj() -> Result<GlobalRef, Error> {
    ACTIVITY_REF
        .read()
        .unwrap()
        .clone()
        .ok_or_else(|| Error::Platform("activity not initialised".into()))
}

fn call_activity_start_file_picker(mime: &str, multiple: bool) -> Result<(), Error> {
    let activity = activity_obj()?;
    with_env(|env| {
        let jmime = env
            .new_string(mime)
            .map_err(|e| Error::Platform(format!("new_string: {e}")))?;
        env.call_method(
            activity.as_obj(),
            "startFilePicker",
            "(Ljava/lang/String;Z)V",
            &[JValue::Object(&jmime), JValue::Bool(multiple as u8)],
        )
        .map_err(|e| Error::Platform(format!("call startFilePicker: {e}")))?;
        Ok(())
    })
}

fn call_activity_start_file_saver(mime: &str, suggested_name: &str) -> Result<(), Error> {
    let activity = activity_obj()?;
    with_env(|env| {
        let jmime = env
            .new_string(mime)
            .map_err(|e| Error::Platform(format!("new_string mime: {e}")))?;
        let jname = env
            .new_string(suggested_name)
            .map_err(|e| Error::Platform(format!("new_string name: {e}")))?;
        env.call_method(
            activity.as_obj(),
            "startFileSaver",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[JValue::Object(&jmime), JValue::Object(&jname)],
        )
        .map_err(|e| Error::Platform(format!("call startFileSaver: {e}")))?;
        Ok(())
    })
}

// ── JNI callbacks from Kotlin ─────────────────────────────────────────────────

/// Called by `MainActivity.onFilesPickerResult(fds: IntArray, names: Array<String>)`.
#[unsafe(no_mangle)]
extern "system" fn Java_dev_dioxus_main_MainActivity_onFilesPickerResult(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    fds: jintArray,
    names: JObjectArray,
) {
    let result = collect_picked_files(&mut env, fds, names);
    if let Ok(ch) = PICK_TX.read() {
        if let Some(tx) = ch.as_ref() {
            let _ = tx.try_send(result.unwrap_or_default());
        }
    }
}

fn collect_picked_files(
    env: &mut JNIEnv<'_>,
    fds: jintArray,
    names: JObjectArray,
) -> Result<Vec<PickedFile>, Error> {
    let fd_array_obj = unsafe { jni::objects::JIntArray::from_raw(fds) };
    let len = env
        .get_array_length(&fd_array_obj)
        .map_err(|e| Error::Platform(format!("array length: {e}")))?;
    let mut fd_buf = vec![0i32; len as usize];
    env.get_int_array_region(&fd_array_obj, 0, &mut fd_buf)
        .map_err(|e| Error::Platform(format!("get fd array: {e}")))?;

    let mut files = Vec::with_capacity(len as usize);
    for (i, &raw_fd) in fd_buf.iter().enumerate() {
        if raw_fd < 0 {
            continue;
        }
        let name: String = {
            let jobj = env
                .get_object_array_element(&names, i as jni::sys::jsize)
                .map_err(|e| Error::Platform(format!("get name {i}: {e}")))?;
            let jstr = JString::from(jobj);
            env.get_string(&jstr)
                .map_err(|e| Error::Platform(format!("get string {i}: {e}")))?
                .into()
        };
        // SAFETY: Android gave us this fd; it is open and readable.
        let mut file = unsafe { std::fs::File::from_raw_fd(raw_fd) };
        let mut bytes = Vec::new();
        use std::io::Read;
        if let Err(e) = file.read_to_end(&mut bytes) {
            warn!(file = %name, error = %e, "rrfd: failed to read picked file");
            continue;
        }
        files.push(PickedFile { name, bytes });
    }
    Ok(files)
}

/// Called by `MainActivity.onSaveFileResult(fd: Int)`.
#[unsafe(no_mangle)]
extern "system" fn Java_dev_dioxus_main_MainActivity_onSaveFileResult(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    fd: jint,
) {
    let result: Result<(), Error> = if fd < 0 {
        Err(Error::NoFileSelected)
    } else {
        // SAFETY: Android gave us this fd; it is open and writable.
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        let data = PENDING_SAVE_DATA.with(|cell| cell.borrow_mut().take());
        if let Some(bytes) = data {
            use std::io::Write;
            file.write_all(&bytes).map_err(Error::Io)
        } else {
            Ok(())
        }
    };
    if let Ok(ch) = SAVE_TX.read() {
        if let Some(tx) = ch.as_ref() {
            let _ = tx.try_send(result);
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn extensions_to_mime(exts: &[&str]) -> String {
    if exts.is_empty() {
        return "*/*".to_string();
    }
    // Android accepts a single MIME type. Map common image extensions to image/*.
    if exts.iter().all(|e| {
        matches!(
            *e,
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tiff" | "heic"
        )
    }) {
        return "image/*".to_string();
    }
    "*/*".to_string()
}
