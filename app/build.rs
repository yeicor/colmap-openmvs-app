// build.rs — Download and vendor JS dependencies declared in /package.json.
//
// Runs automatically before every `cargo build` (or `dx build`/`dx serve`).
// Files go to `app/assets/lib/` and are served by Dioxus at their original
// paths (no hash). Only eruda is wrapped in asset!() in Rust code (it's a
// self-contained IIFE); three.js addons import from a fixed sibling path, so
// they intentionally bypass Dioxus asset hashing to keep the import URL stable.
//
// Versioning: a marker file `app/assets/lib/.versions` stores the currently
// downloaded versions. When package.json changes (cargo:rerun-if-changed),
// build.rs runs again, compares versions, and re-downloads any that changed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // package.json lives one level up (workspace root)
    let pkg_json_path = manifest_dir.join("..").join("package.json");

    // Trigger rebuild when versions change
    println!(
        "cargo:rerun-if-changed={}",
        pkg_json_path
            .canonicalize()
            .unwrap_or(pkg_json_path.clone())
            .display()
    );

    // Parse versions from package.json (no extra deps — plain string search)
    let pkg_json = fs::read_to_string(&pkg_json_path)
        .unwrap_or_else(|e| panic!("Failed to read package.json: {e}"));

    let eruda_version = extract_dep_version(&pkg_json, "eruda")
        .expect("\"eruda\" not found in package.json dependencies");
    let three_version = extract_dep_version(&pkg_json, "three")
        .expect("\"three\" not found in package.json dependencies");

    let lib_dir = manifest_dir.join("assets").join("lib");
    fs::create_dir_all(&lib_dir).expect("Failed to create assets/lib/");

    // ── Version marker ───────────────────────────────────────────────────────
    let profile = std::env::var("PROFILE").unwrap_or_default();
    let is_debug = profile != "release";

    let marker_path = lib_dir.join(".versions");
    // Include PROFILE so that a debug→release switch also triggers re-download
    // (eruda is only present in debug builds).
    let want_marker = format!("eruda={eruda_version}\nthree={three_version}\nprofile={profile}\n");
    let current_marker = fs::read_to_string(&marker_path).unwrap_or_default();
    let need_refresh = current_marker != want_marker;

    // If versions changed, remove old files so they get re-downloaded below
    if need_refresh {
        let _ = fs::remove_dir_all(lib_dir.join("eruda"));
        let _ = fs::remove_dir_all(lib_dir.join("three"));
    }

    // ── eruda (debug builds only) ────────────────────────────────────────────
    let eruda_dir = lib_dir.join("eruda");
    if is_debug {
        fs::create_dir_all(&eruda_dir).expect("Failed to create assets/lib/eruda/");
        download_if_absent(
            &format!("https://cdn.jsdelivr.net/npm/eruda@{eruda_version}/eruda.js"),
            &eruda_dir.join("eruda.js"),
        );
    }

    // ── three.js ─────────────────────────────────────────────────────────────
    let three_dir = lib_dir.join("three");
    fs::create_dir_all(&three_dir).expect("Failed to create assets/lib/three/");

    // Main ESM module (self-contained, no external imports)
    download_if_absent(
        &format!("https://cdn.jsdelivr.net/npm/three@{three_version}/build/three.module.js"),
        &three_dir.join("three.module.js"),
    );

    // ── three.js addon files: GLTFLoader + TrackballControls + their deps ─────
    // These files use bare `from 'three'` (resolved at runtime by the importmap
    // injected in App) and relative sibling imports. We MUST use
    // `with_minify(false)` when referencing them via asset!() so that dx skips
    // esbuild processing entirely (esbuild would fail on the bare 'three'
    // specifier and emit ERROR logs). Files are copied verbatim; the browser
    // resolves all imports at runtime using the importmap.

    let gltf_path = three_dir.join("GLTFLoader.js");
    download_if_absent(
        &format!(
            "https://cdn.jsdelivr.net/npm/three@{three_version}/examples/jsm/loaders/GLTFLoader.js"
        ),
        &gltf_path,
    );

    // TrackballControls — same treatment
    let trackball_path = three_dir.join("TrackballControls.js");
    download_if_absent(
        &format!(
            "https://cdn.jsdelivr.net/npm/three@{three_version}/examples/jsm/controls/TrackballControls.js"
        ),
        &trackball_path,
    );

    // Transitive dependencies of GLTFLoader:
    //   GLTFLoader.js → '../utils/BufferGeometryUtils.js'
    //   GLTFLoader.js → '../utils/SkeletonUtils.js'
    // After dx flattens assets, those relative paths resolve to
    // `/utils/BufferGeometryUtils.js` and `/utils/SkeletonUtils.js` in the
    // browser; the importmap in App.rs maps them to their (hashed) asset URLs.
    let utils_dir = lib_dir.join("utils");
    fs::create_dir_all(&utils_dir).expect("Failed to create assets/lib/utils/");
    download_if_absent(
        &format!(
            "https://cdn.jsdelivr.net/npm/three@{three_version}/examples/jsm/utils/BufferGeometryUtils.js"
        ),
        &utils_dir.join("BufferGeometryUtils.js"),
    );
    download_if_absent(
        &format!(
            "https://cdn.jsdelivr.net/npm/three@{three_version}/examples/jsm/utils/SkeletonUtils.js"
        ),
        &utils_dir.join("SkeletonUtils.js"),
    );

    // ── Write marker (only after all downloads succeed) ────────────────────────
    fs::write(&marker_path, &want_marker).expect("Failed to write .versions marker");
}

// ── Helpers ──────────────────────────────────────────────────────────────────────────────

/// Extract a package version from the `"dependencies"` block of a package.json.
/// Handles both `"pkg": "1.2.3"` and `"pkg": "^1.2.3"` (strips leading `^~`).
fn extract_dep_version(json: &str, pkg: &str) -> Option<String> {
    // Find the "dependencies" section first so we don't pick up dev-deps etc.
    let deps_start = json.find("\"dependencies\"")?;
    let section = &json[deps_start..];

    let key = format!("\"{pkg}\"");
    let key_pos = section.find(&key)?;
    let after_key = &section[key_pos + key.len()..];
    // Skip optional whitespace and colon
    let after_colon = after_key.trim_start().trim_start_matches(':').trim_start();
    if !after_colon.starts_with('"') {
        return None;
    }
    let inner = &after_colon[1..]; // skip opening quote
    let end = inner.find('"')?;
    let raw = &inner[..end];
    // Strip semver range prefixes like ^ or ~
    Some(raw.trim_start_matches(['^', '~']).to_owned())
}

/// Download `url` to `dest` using `curl`.
/// Skips if `dest` already exists (use the version marker to force refresh).
fn download_if_absent(url: &str, dest: &Path) {
    if dest.exists() {
        return;
    }
    eprintln!("cargo:warning=Downloading {url}");
    let status = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--output",
            dest.to_str().expect("non-UTF8 dest path"),
            url,
        ])
        .status()
        .unwrap_or_else(|e| panic!("curl exited with {e} while downloading {url}"));
    if !status.success() {
        panic!("curl exited with {status} while downloading {url}");
    }
}
