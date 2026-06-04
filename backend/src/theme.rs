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
        // Hacky side effect to disable Android 15+ edge-to-edge default so content doesn't render behind system bars.
        if let Err(err) = disable_edge_to_edge() {
            tracing::warn!(error = %err, "Failed to disable Android edge-to-edge mode; content may render partially behind system bars");
        }
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
fn disable_edge_to_edge() -> anyhow::Result<()> {
    use jni::objects::JValue;

    let (vm, context) = grab_vm_and_context()?;
    let mut env = vm.attach_current_thread()?;

    // ── Disable edge-to-edge ───────────────────────────────────────────
    // Android 15+ renders app content behind system bars by default.
    // We opt out via Window.setDecorFitsSystemWindows(true).
    if let Ok(sdk_class) = env.find_class("android/os/Build$VERSION") {
        if let Ok(sdk_field) = env.get_static_field(&sdk_class, "SDK_INT", "I") {
            if let Ok(sdk_int) = sdk_field.i() {
                // Try to get the Window object from the Activity context.
                if let Ok(window_result) =
                    env.call_method(&context, "getWindow", "()Landroid/view/Window;", &[])
                {
                    if let Ok(window) = window_result.l() {
                        if sdk_int >= 35 {
                            // API 35+: Window.setDecorFitsSystemWindows(true)
                            let _ = env.call_method(
                                &window,
                                "setDecorFitsSystemWindows",
                                "(Z)V",
                                &[JValue::Bool(true.into())],
                            );
                            return Ok(());
                        } else {
                            // Older: Window.addFlags(FLAG_DRAWS_SYSTEM_BAR_BACKGROUNDS)
                            if let Ok(lp_class) =
                                env.find_class("android/view/WindowManager$LayoutParams")
                            {
                                if let Ok(flag_field) = env.get_static_field(
                                    &lp_class,
                                    "FLAG_DRAWS_SYSTEM_BAR_BACKGROUNDS",
                                    "I",
                                ) {
                                    if let Ok(flag) = flag_field.i() {
                                        let _ = env.call_method(
                                            &window,
                                            "addFlags",
                                            "(I)V",
                                            &[JValue::Int(flag)],
                                        );
                                    }
                                }
                            }
                            return Ok(());
                        }
                    }
                    return Err(anyhow::anyhow!(
                        "getWindow() succeeded but return type error: expected Window object"
                    ));
                }
                return Err(anyhow::anyhow!(
                    "Failed to call getWindow() on context to disable edge-to-edge"
                ));
            }
            return Err(anyhow::anyhow!(
                "SDK_INT field access failed or type error: expected int"
            ));
        }
        return Err(anyhow::anyhow!(
            "SDK_INT field not found in Build.VERSION class"
        ));
    }
    return Err(anyhow::anyhow!(
        "Build.VERSION class not found for SDK_INT access"
    ));
}
