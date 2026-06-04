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

/// Query the Android system UI mode via JNI and disable edge-to-edge.
///
/// Equivalent Java for dark-mode detection:
/// ```java
/// int uiMode = context.getResources().getConfiguration().uiMode;
/// boolean isDark =
///     (uiMode & Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES;
/// ```
///
/// Also disables the Android 15+ edge-to-edge default so the app's content
/// does not render behind system bars.
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

#[cfg(target_os = "android")]
pub fn disable_edge_to_edge() -> anyhow::Result<()> {
    use jni::objects::JValue;

    let (vm, context) = grab_vm_and_context()?;
    let mut env = vm.attach_current_thread()?;

    // ── Only API 35+ enforces edge-to-edge ───────────────────────────
    let sdk_class = env.find_class("android/os/Build$VERSION")?;
    let sdk_field = env.get_static_field(&sdk_class, "SDK_INT", "I")?;
    let sdk_int = sdk_field.i()?;

    if sdk_int < 35 {
        // Before Android 15 edge-to-edge was opt-in, not enforced.
        tracing::trace!("SDK {sdk_int} < 35 – edge-to-edge not enforced, skipping");
        return Ok(());
    }

    // ── Check that context is an Activity (needed for getWindow()) ────
    // ndk_context only guarantees a plain android.content.Context.
    // getWindow() is defined on Activity, so we must verify first.
    let activity_class = env.find_class("android/app/Activity")?;
    let is_activity = env.is_instance_of(&context, &activity_class)?;
    if !is_activity {
        anyhow::bail!(
            "Context is not an Activity, cannot call getWindow(). \
             Falling back to AndroidManifest theme opt-out if available."
        );
    }

    // ── Get the Window object ─────────────────────────────────────────
    let window = env
        .call_method(&context, "getWindow", "()Landroid/view/Window;", &[])
        .and_then(|w| w.l())
        .map_err(|e| anyhow::anyhow!("getWindow() failed: {e}"))?;

    // ── Opt out of edge-to-edge ───────────────────────────────────────
    // Window.setDecorFitsSystemWindows(true) tells the Window to draw
    // its own backgrounds behind system bars instead of letting content
    // draw there. This works on API 35+ (deprecated on 36+ but still
    // functional at least through Android 16).
    let _ = env
        .call_method(
            &window,
            "setDecorFitsSystemWindows",
            "(Z)V",
            &[JValue::Bool(true.into())],
        )
        .map_err(|e| anyhow::anyhow!("setDecorFitsSystemWindows() failed: {e}"))?;
    tracing::debug!("Disabled edge-to-edge via Window.setDecorFitsSystemWindows(true)");

    Ok(())
}
