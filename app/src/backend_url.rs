//! Backend URL configuration.
//!
//! The backend URL is configurable and persisted on **all** platforms:
//!
//! * **WASM/web**: read from `?backend=` query param, then `#backend=` hash
//!   param, then `localStorage`.  Saving writes to `localStorage` and
//!   "restarting" means reloading the page.
//! * **Native (desktop/mobile/server)**: read from and written to a config
//!   file in the platform-appropriate config directory.  "Restarting" means
//!   calling `std::process::exit(0)`.

use std::sync::OnceLock;

/// Global slot holding the backend URL that was resolved at process start.
/// Set exactly once in `main()` before `dioxus::launch`, read thereafter.
pub static BACKEND_URL: OnceLock<String> = OnceLock::new();

/// Reads the backend URL to use for this session.
///
/// Priority on WASM:
/// 1. `?backend=` URL query parameter — persisted to `localStorage`.
/// 2. `#backend=` URL hash parameter  — persisted to `localStorage`.
/// 3. `backend_url` key in `localStorage` — from a previous session.
/// 4. Empty string (same-origin fallback).
///
/// On non-WASM: reads the config file produced by [`save_backend_url`], or
/// returns an empty string if no config file exists.
pub fn read_initial_backend_url() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        // 1. Query param ?backend=
        if let Some(url) = read_query_param() {
            save_to_local_storage(&url);
            return url;
        }
        // 2. Hash param #backend=
        if let Some(url) = read_hash_param() {
            save_to_local_storage(&url);
            return url;
        }
        // 3. localStorage
        load_from_local_storage().unwrap_or_default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        read_from_config_file().unwrap_or_default()
    }
}

/// Persists `url` to platform storage for use on next launch.
///
/// * WASM: writes (or removes) the `backend_url` key in `localStorage`.
/// * Non-WASM: writes (or removes) the config file.
pub fn save_backend_url(url: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if url.is_empty() {
            if let Some(storage) = local_storage() {
                let _ = storage.remove_item("backend_url");
            }
        } else {
            save_to_local_storage(url);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if url.is_empty() {
            let _ = std::fs::remove_file(config_file_path());
        } else {
            write_to_config_file(url);
        }
    }
}

/// Applies the new backend URL by reloading the page (WASM) or exiting the
/// process so the OS / launcher can restart it (non-WASM).
pub fn reload_or_exit() {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            let _ = window.location().reload();
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::process::exit(0);
    }
}

/// Returns a short, platform-appropriate message telling the user what will
/// happen when they confirm the backend URL change.
pub fn needs_restart_message() -> &'static str {
    #[cfg(target_arch = "wasm32")]
    {
        "The page will reload to apply the new backend URL."
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "The app must be restarted to apply the new backend URL."
    }
}

/// Whether runtime backend-URL configuration is supported on this platform.
///
/// Always `true` — all platforms support it now.
pub fn backend_url_configurable() -> bool {
    true
}

/// Validates a backend URL using the same parsing that Dioxus uses internally
/// when constructing server-function requests (`http::Uri::from_str`).
///
/// A valid backend URL must be parseable as an `http::Uri` and have a scheme
/// of `http` or `https`.
pub fn is_valid_backend_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return false;
    }
    match trimmed.parse::<http::Uri>() {
        Ok(u) => u.scheme_str().is_some_and(|s| s == "http" || s == "https"),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// WASM helpers
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
fn read_query_param() -> Option<String> {
    let window = web_sys::window()?;
    let search = window.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    let val = params.get("backend")?;
    if val.is_empty() {
        None
    } else {
        Some(val)
    }
}

#[cfg(target_arch = "wasm32")]
fn read_hash_param() -> Option<String> {
    let window = web_sys::window()?;
    let hash = window.location().hash().ok()?;
    let hash = hash.trim_start_matches('#');
    for part in hash.split('&') {
        let mut kv = part.splitn(2, '=');
        if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
            if k == "backend" && !v.is_empty() {
                // URL-decode the value (e.g. %3A → ':')
                let decoded = js_sys::decode_uri_component(v)
                    .ok()
                    .and_then(|s| s.as_string())?;
                return Some(decoded);
            }
        }
    }
    None
}

#[cfg(target_arch = "wasm32")]
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

#[cfg(target_arch = "wasm32")]
fn save_to_local_storage(url: &str) {
    if let Some(storage) = local_storage() {
        let _ = storage.set_item("backend_url", url);
    }
}

#[cfg(target_arch = "wasm32")]
fn load_from_local_storage() -> Option<String> {
    let val = local_storage()?.get_item("backend_url").ok()??;
    if val.is_empty() {
        None
    } else {
        Some(val)
    }
}

// ---------------------------------------------------------------------------
// Non-WASM helpers
// ---------------------------------------------------------------------------

/// Returns the platform-appropriate path for the backend-URL config file.
///
/// Resolution order:
/// 1. `$XDG_CONFIG_HOME/colmap-openmvs-app/backend_url`
/// 2. `$APPDATA/colmap-openmvs-app/backend_url`
/// 3. `$HOME/.config/colmap-openmvs-app/backend_url`
/// 4. `./backend_url` (fallback)
#[cfg(not(target_arch = "wasm32"))]
fn config_file_path() -> std::path::PathBuf {
    use std::path::PathBuf;
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg)
            .join("colmap-openmvs-app")
            .join("backend_url");
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata)
            .join("colmap-openmvs-app")
            .join("backend_url");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("colmap-openmvs-app")
            .join("backend_url");
    }
    PathBuf::from("./backend_url")
}

#[cfg(not(target_arch = "wasm32"))]
fn read_from_config_file() -> Option<String> {
    let content = std::fs::read_to_string(config_file_path()).ok()?;
    let trimmed = content.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn write_to_config_file(url: &str) {
    let path = config_file_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, url);
}
