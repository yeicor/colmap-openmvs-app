//! System color-scheme / dark-mode detection.
//!
//! Returns the preferred color scheme as seen by the *server* process:
//!
//! * `None`         – no preference known; let the browser CSS media query
//!   (`prefers-color-scheme`) decide.
//! * `Some(false)`  – force light mode.
//! * `Some(true)`   – force dark mode.
//!
//! On Android the WebView does not always propagate `prefers-color-scheme`
//! reliably, so we detect the system UI mode via JNI.  Other platforms return
//! `None` so the browser's own media-query handling is used.

/// Returns the system dark-mode preference, if it can be determined
/// server-side.  See module docs for the `None` / `Some` semantics.
pub async fn get_dark_mode() -> dioxus::Result<Option<bool>> {
    #[cfg(target_os = "android")]
    {
        return Ok(Some(detect_android_dark_mode()?));
    }

    // On all other platforms let the browser/CSS media query take over.
    #[allow(unreachable_code)]
    Ok(None)
}

/// Query the Android system UI mode via JNI.
///
/// Equivalent Java:
/// ```java
/// int uiMode = context.getResources().getConfiguration().uiMode;
/// boolean isDark =
///     (uiMode & Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES;
/// ```
#[cfg(target_os = "android")]
fn detect_android_dark_mode() -> anyhow::Result<bool> {
    use jni::{objects::JObject, JavaVM};

    // ndk-context holds the JavaVM* and jobject* set up by the Dioxus/Activity runtime.
    let android_ctx = ndk_context::android_context();

    // Bail early on null pointers rather than triggering UB.
    if android_ctx.vm().is_null() {
        anyhow::bail!("Android JavaVM pointer is null – ndk-context not initialised");
    }
    if android_ctx.context().is_null() {
        anyhow::bail!("Android Context pointer is null – ndk-context not initialised");
    }

    // SAFETY: both pointers are non-null and were written by the Android runtime.
    let vm = unsafe { JavaVM::from_raw(android_ctx.vm().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let context = unsafe { JObject::from_raw(android_ctx.context().cast()) };

    // context.getResources()
    let resources = env
        .call_method(
            &context,
            "getResources",
            "()Landroid/content/res/Resources;",
            &[],
        )
        .map_err(|e| anyhow::anyhow!("getResources() failed: {e}"))?
        .l()
        .map_err(|e| anyhow::anyhow!("getResources() return type error: {e}"))?;

    // resources.getConfiguration()
    let configuration = env
        .call_method(
            &resources,
            "getConfiguration",
            "()Landroid/content/res/Configuration;",
            &[],
        )
        .map_err(|e| anyhow::anyhow!("getConfiguration() failed: {e}"))?
        .l()
        .map_err(|e| anyhow::anyhow!("getConfiguration() return type error: {e}"))?;

    // int Configuration.uiMode
    let ui_mode = env
        .get_field(&configuration, "uiMode", "I")
        .map_err(|e| anyhow::anyhow!("uiMode field access failed: {e}"))?
        .i()
        .map_err(|e| anyhow::anyhow!("uiMode type error: {e}"))?;

    // Configuration.UI_MODE_NIGHT_MASK = 0x30
    // Configuration.UI_MODE_NIGHT_YES  = 0x20
    Ok((ui_mode & 0x30) == 0x20)
}
