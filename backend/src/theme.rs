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

/// Check the global `Settings` for a user-configured theme override first.
///
/// * If `settings.theme_override` is `"light"` return `Some(false)`.
/// * If `settings.theme_override` is `"dark"`  return `Some(true)`.
/// * Otherwise fall through to the platform-specific detection (Android JNI)
///   or `None` so the browser's CSS media query decides.
async fn check_settings_override() -> Option<bool> {
    match crate::settings::get_settings().await {
        Ok(s) => match s.theme_override.as_deref() {
            Some("light") => Some(false),
            Some("dark") => Some(true),
            _ => None,
        },
        Err(_) => None,
    }
}

/// Returns the system dark-mode preference, if it can be determined
/// server-side.  See module docs for the `None` / `Some` semantics.
///
/// A `theme_override` stored in the application settings takes precedence
/// over platform detection.
pub async fn get_dark_mode() -> dioxus::Result<Option<bool>> {
    // 1. Check the user-configured override from settings.
    if let Some(forced) = check_settings_override().await {
        return Ok(Some(forced));
    }

    // 2. Platform-specific detection.
    #[cfg(target_os = "android")]
    {
        return Ok(Some(detect_android_dark_mode()?));
    }

    // On all other platforms let the browser/CSS media query take over.
    #[allow(unreachable_code)]
    Ok(None)
}

#[cfg(target_os = "android")]
fn grab_vm_and_context() -> anyhow::Result<(jni::JavaVM, jni::objects::JObject<'static>)> {
    use jni::JavaVM;

    let android_ctx = ndk_context::android_context();

    if android_ctx.vm().is_null() {
        anyhow::bail!("Android JavaVM pointer is null – ndk-context not initialised");
    }
    if android_ctx.context().is_null() {
        anyhow::bail!("Android context pointer is null – ndk-context not initialised");
    }

    // SAFETY: the pointer is non-null and was written by the Android runtime.
    let vm = unsafe { JavaVM::from_raw(android_ctx.vm().cast()) }?;
    let context = unsafe { jni::objects::JObject::from_raw(android_ctx.context().cast()) };

    Ok((vm, context))
}

/// Query the Android system UI mode via JNI
///
/// Equivalent Java for dark-mode detection:
/// ```java
/// int uiMode = context.getResources().getConfiguration().uiMode;
/// boolean isDark =
///     (uiMode & Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES;
/// ```
#[cfg(target_os = "android")]
fn detect_android_dark_mode() -> anyhow::Result<bool> {
    let (vm, context) = grab_vm_and_context()?;
    let mut env = vm.attach_current_thread()?;

    // ── Detect dark mode ────────────────────────────────────────────────
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

    let ui_mode = env
        .get_field(&configuration, "uiMode", "I")
        .map_err(|e| anyhow::anyhow!("uiMode field access failed: {e}"))?
        .i()
        .map_err(|e| anyhow::anyhow!("uiMode type error: {e}"))?;

    let is_dark = (ui_mode & 0x30) == 0x20;

    Ok(is_dark)
}
