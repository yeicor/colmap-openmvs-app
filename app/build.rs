use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(target_family = "windows")]
const SHELL: &str = "cmd.exe";
#[cfg(target_family = "windows")]
const SHELL_ARG: &str = "/C";

#[cfg(not(target_family = "windows"))]
const SHELL: &str = "bash";
#[cfg(not(target_family = "windows"))]
const SHELL_ARG: &str = "-c";

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .join("..")
        .canonicalize()
        .expect("Failed to resolve workspace root");

    // ── Rebuild triggers ─────────────────────────────────────────────────────
    let pkg_json_path = workspace_root.join("package.json");
    let viewer_src = manifest_dir.join("js").join("viewer3d.js");
    println!("cargo:rerun-if-changed={}", pkg_json_path.display());
    println!("cargo:rerun-if-changed={}", viewer_src.display());

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

// ── npm ci ────────────────────────────────────────────────────────────────────

fn npm_install_if_needed(workspace_root: &Path, pkg_json_path: &Path) {
    let marker = workspace_root.join(".build-rs-hash");
    let pkg_bytes =
        fs::read(pkg_json_path).unwrap_or_else(|e| panic!("Cannot read package.json: {e}"));
    let hash = fnv64(&pkg_bytes);

    if fs::read_to_string(&marker).unwrap_or_default().trim() == hash
        && workspace_root.join("node_modules").exists()
    {
        return;
    }

    eprintln!("cargo:warning=Running npm ci …");
    let output = run_shell("npm ci", workspace_root)
        .unwrap_or_else(|e| panic!("Failed to spawn `npm ci`: {e} — is Node.js installed?"));
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "`npm ci` exited with {}\n--- stderr ---\n{stderr}\n--------------",
            output.status
        );
    }
    let _ = fs::write(&marker, &hash);
}

// ── esbuild bundle ────────────────────────────────────────────────────────────

fn bundle_viewer(workspace_root: &Path, src: &Path, out: &Path) {
    let src_bytes = fs::read(src).unwrap_or_else(|e| panic!("Cannot read {}: {e}", src.display()));
    let hash = fnv64(&src_bytes);

    let hash_file = out.with_extension("hash");
    if fs::read_to_string(&hash_file).unwrap_or_default().trim() == hash && out.exists() {
        return;
    }

    let esbuild = find_esbuild(workspace_root)
        .unwrap_or_else(|| panic!("esbuild not found in node_modules — run `npm install` first"));

    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).ok();
    }

    let cmd = format!(
        "{} {} --outfile={} --bundle --format=esm --minify --external:https://* --external:http://* --log-level=warning",
        quote_path(&esbuild),
        normalized_path(src),
        normalized_path(out),
    );

    eprintln!("cargo:warning=Bundling {} …", src.display());
    let status = run_shell(&cmd, workspace_root)
        .unwrap_or_else(|e| panic!("Failed to spawn esbuild: {e}"))
        .status;
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

fn run_shell(cmd: &str, dir: &Path) -> std::io::Result<std::process::Output> {
    Command::new(SHELL)
        .args([SHELL_ARG, cmd])
        .current_dir(dir)
        .output()
}

/// FNV-1a 64-bit hash — no external deps, returns a hex string.
fn fnv64(data: &[u8]) -> String {
    let mut h: u64 = 14695981039346656037;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("{h:016x}")
}

/// Find esbuild binary or shim in node_modules/.bin
fn find_esbuild(workspace_root: &Path) -> Option<PathBuf> {
    let bin_dir = workspace_root.join("node_modules").join(".bin");
    // Try platform-native names first (important on Windows where the bare
    // "esbuild" is a POSIX script, not a .cmd batch file).
    let names: &[&str] = if cfg!(windows) {
        &["esbuild.cmd", "esbuild.exe", "esbuild"]
    } else {
        &["esbuild"]
    };
    for name in names {
        let candidate = bin_dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Produce a string suitable for esbuild CLI arguments.
/// On Windows, backslashes are converted to forward slashes to avoid
/// escaping issues in argument parsing.
fn normalized_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.into_owned()
    }
}

/// Wrap a path in double quotes so the shell command string handles spaces.
fn quote_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    if s.contains(' ') {
        format!("\"{}\"", s)
    } else {
        s.into_owned()
    }
}
