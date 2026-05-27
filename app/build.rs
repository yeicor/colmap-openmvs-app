// build.rs — Pre-bundle JS assets before dx copies them.
//
// Strategy:
//   1. Run `npm install` (workspace root) when package.json changes.
//      Uses a content-hash marker so it is skipped on subsequent builds.
//   2. Run esbuild to bundle app/js/viewer3d.js → app/assets/viewer3d.bundle.js
//      when the source changes.  Uses the same hash-file approach.
//   3. Copy eruda.js from node_modules into app/assets/lib/eruda/ (debug only).
//
// cargo:rerun-if-changed is set ONLY on the two source inputs.  Output files
// are never listed, which prevents the "output changes → cargo reruns build.rs
// → output changes" loop that plagued the previous implementation.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .join("..")
        .canonicalize()
        .expect("Failed to resolve workspace root");

    // ── Rebuild triggers ─────────────────────────────────────────────────────
    // SOURCE inputs: trigger rebuild when they change.
    let pkg_json_path = workspace_root.join("package.json");
    let viewer_src = manifest_dir.join("js").join("viewer3d.js");
    println!("cargo:rerun-if-changed={}", pkg_json_path.display());
    println!("cargo:rerun-if-changed={}", viewer_src.display());

    // OUTPUT files: if they are MISSING cargo will unconditionally re-run
    // build.rs so they get regenerated.  Once they exist and are unchanged,
    // cargo skips this script — no loop.
    let bundle_out = manifest_dir.join("assets").join("viewer3d.bundle.js");
    println!("cargo:rerun-if-changed={}", bundle_out.display());

    let profile = std::env::var("PROFILE").unwrap_or_default();
    if profile != "release" {
        let eruda_out = manifest_dir
            .join("assets")
            .join("lib")
            .join("eruda")
            .join("eruda.js");
        println!("cargo:rerun-if-changed={}", eruda_out.display());
    }

    // ── 1. npm install (cached by package.json content hash) ─────────────────
    npm_install_if_needed(&workspace_root, &pkg_json_path);

    // ── 2. Bundle viewer3d.js → assets/viewer3d.bundle.js ───────────────────
    bundle_viewer(&workspace_root, &viewer_src, &bundle_out);

    // ── 3. Copy eruda (debug builds only) ────────────────────────────────────
    if profile != "release" {
        copy_eruda(&workspace_root, &manifest_dir);
    }
}

// ── npm install ───────────────────────────────────────────────────────────────

fn npm_install_if_needed(workspace_root: &Path, pkg_json_path: &Path) {
    let marker = workspace_root.join("node_modules").join(".build-rs-hash");
    let pkg_bytes =
        fs::read(pkg_json_path).unwrap_or_else(|e| panic!("Cannot read package.json: {e}"));
    let hash = fnv64(&pkg_bytes);

    if fs::read_to_string(&marker).unwrap_or_default().trim() == hash
        && workspace_root.join("node_modules").exists()
    {
        return; // Already up-to-date
    }

    eprintln!("cargo:warning=Running npm install …");
    let status = Command::new("npm")
        .args(["install", "--prefer-offline"])
        .current_dir(workspace_root)
        .status()
        .expect("Failed to spawn `npm install` — is Node.js installed?");
    if !status.success() {
        panic!("`npm install` exited with {status}");
    }
    // Write marker AFTER successful install so a failed install retries next time
    let _ = fs::write(&marker, &hash);
}

// ── esbuild bundle ────────────────────────────────────────────────────────────

fn bundle_viewer(workspace_root: &Path, src: &Path, out: &Path) {
    let src_bytes = fs::read(src).unwrap_or_else(|e| panic!("Cannot read {}: {e}", src.display()));
    let hash = fnv64(&src_bytes);

    let hash_file = out.with_extension("bundle.js.hash");
    if fs::read_to_string(&hash_file).unwrap_or_default().trim() == hash && out.exists() {
        return; // Bundle is current
    }

    let esbuild = find_esbuild(workspace_root)
        .unwrap_or_else(|| panic!("esbuild not found — run `dx build` once so Dioxus installs it"));

    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).ok();
    }

    eprintln!("cargo:warning=Bundling {} …", src.display());
    let status = Command::new(&esbuild)
        .args([
            src.to_str().unwrap(),
            &format!("--outfile={}", out.display()),
            "--bundle",
            "--format=esm",
            "--minify",
            "--external:https://*",
            "--external:http://*",
            "--log-level=warning",
        ])
        // Run from workspace root so esbuild walks up to node_modules/ there
        .current_dir(workspace_root)
        .status()
        .expect("Failed to spawn esbuild");
    if !status.success() {
        panic!("esbuild bundle failed");
    }

    let _ = fs::write(&hash_file, &hash);
}

// ── eruda copy ────────────────────────────────────────────────────────────────

fn copy_eruda(workspace_root: &Path, manifest_dir: &Path) {
    let src = workspace_root
        .join("node_modules")
        .join("eruda")
        .join("eruda.js");
    let dst_dir = manifest_dir.join("assets").join("lib").join("eruda");
    let dst = dst_dir.join("eruda.js");

    fs::create_dir_all(&dst_dir).expect("Failed to create assets/lib/eruda/");

    let needs_copy = dst
        .metadata()
        .and_then(|dm| {
            src.metadata().map(|sm| {
                sm.modified().unwrap_or(std::time::UNIX_EPOCH)
                    > dm.modified().unwrap_or(std::time::UNIX_EPOCH)
            })
        })
        .unwrap_or(true);

    if needs_copy {
        fs::copy(&src, &dst)
            .unwrap_or_else(|e| panic!("Failed to copy eruda.js from node_modules: {e}"));
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// FNV-1a 64-bit hash — no external deps, returns a hex string.
fn fnv64(data: &[u8]) -> String {
    let mut h: u64 = 14695981039346656037;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("{h:016x}")
}

/// Locate an esbuild binary.  Search order:
///   1. DX_ESBUILD_PATH env var (explicit override)
///   2. The dx-managed tools dir (~/.local/share/.dx/tools on Linux,
///      ~/.dx/tools on macOS/Windows) — scans any esbuild-* subdirectory.
///   3. `esbuild` on the system PATH.
fn find_esbuild(workspace_root: &Path) -> Option<PathBuf> {
    // 0. Explicit override
    if let Some(p) = std::env::var_os("DX_ESBUILD_PATH") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }

    // 1. dx-managed cache
    let dx_home: PathBuf = std::env::var_os("DX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let base = if cfg!(any(target_os = "macos", target_os = "windows")) {
                home_dir().join(".dx")
            } else {
                std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home_dir().join(".local").join("share"))
                    .join(".dx")
            };
            base
        });

    let tools = dx_home.join("tools");
    if tools.is_dir() {
        if let Ok(entries) = fs::read_dir(&tools) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with("esbuild-") {
                    let bin = entry.path().join(esbuild_bin_name());
                    if bin.exists() {
                        return Some(bin);
                    }
                }
            }
        }
    }

    // 2. System PATH
    let cmd = if cfg!(windows) { "where" } else { "which" };
    if let Ok(out) = Command::new(cmd).arg(esbuild_bin_name()).output() {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout);
            let line = p.lines().next().unwrap_or("").trim();
            if !line.is_empty() {
                return Some(PathBuf::from(line));
            }
        }
    }

    // 3. Local node_modules (if someone added esbuild as devDep)
    let local = workspace_root
        .join("node_modules")
        .join(".bin")
        .join(esbuild_bin_name());
    if local.exists() {
        return Some(local);
    }

    None
}

fn esbuild_bin_name() -> &'static str {
    if cfg!(windows) {
        "esbuild.exe"
    } else {
        "esbuild"
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .expect("Cannot determine home directory (HOME / USERPROFILE not set)")
}
