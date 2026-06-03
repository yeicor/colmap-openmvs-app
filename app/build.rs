use std::path::{Path, PathBuf};
use std::process::Command;
use std::{fs, panic};

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

    // ── 4. Extract build metadata from Git, date, and Rust version ───────────
    extract_build_metadata();

    // ── 5. Generate demo assets if the demo feature is enabled ────────────────
    if std::env::var("CARGO_FEATURE_DEMO").is_ok() {
        generate_demo_assets(&manifest_dir);
    }
}

// ── Generate demo assets ──────────────────────────────────────────────────────

fn generate_demo_assets(manifest_dir: &Path) {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("demo_assets.rs");
    
    let demo_dir = manifest_dir.join("assets").join("demo");
    println!("cargo:rerun-if-changed={}", demo_dir.display());

    let manifest_path = demo_dir.join("manifest.json");
    if !manifest_path.exists() {
        // Just generate empty stubs if the assets don't exist yet so it builds
        fs::write(
            &dest_path,
            "pub const DEMO_MANIFEST: &str = \"{}\";\n\
             pub const DOWNLOAD_EVENTS_JSON: &str = \"[]\";\n\
             pub const PIPELINE_EVENTS_JSON: &str = \"[]\";\n\
             pub fn demo_image_bytes(_name: &str) -> Option<&'static [u8]> { None }\n\
             pub fn demo_output_bytes(_path: &str) -> Option<&'static [u8]> { None }\n"
        ).unwrap();
        return;
    }

    let manifest_str = fs::read_to_string(&manifest_path).unwrap();
    
    let download_events_path = demo_dir.join("download_events.json");
    let download_events_str = if download_events_path.exists() {
        fs::read_to_string(&download_events_path).unwrap()
    } else {
        "[]".to_string()
    };
    println!("cargo:rerun-if-changed={}", download_events_path.display());

    let pipeline_events_path = demo_dir.join("pipeline_events.json");
    let pipeline_events_str = if pipeline_events_path.exists() {
        fs::read_to_string(&pipeline_events_path).unwrap()
    } else {
        "[]".to_string()
    };
    println!("cargo:rerun-if-changed={}", pipeline_events_path.display());
    
    let mut out = String::new();
    out.push_str(&format!(
        "pub const DEMO_MANIFEST: &str = r###\"{}\"###;\n\n",
        manifest_str
    ));
    out.push_str(&format!(
        "pub const DOWNLOAD_EVENTS_JSON: &str = r###\"{}\"###;\n\n",
        download_events_str
    ));
    out.push_str(&format!(
        "pub const PIPELINE_EVENTS_JSON: &str = r###\"{}\"###;\n\n",
        pipeline_events_str
    ));
    
    // Images (flat directory, no subdirectories)
    out.push_str("pub fn demo_image_bytes(name: &str) -> Option<&'static [u8]> {\n");
    out.push_str("    match name {\n");
    let images_dir = demo_dir.join("images");
    if images_dir.exists() {
        for entry in fs::read_dir(&images_dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name().unwrap().to_str().unwrap();
                let abs_path = path.canonicalize().unwrap().to_string_lossy().into_owned();
                let abs_path_escaped = abs_path.replace("\\", "\\\\");
                out.push_str(&format!(
                    "        \"{}\" => Some(include_bytes!(\"{}\")),\n",
                    name, abs_path_escaped
                ));
            }
        }
    }
    out.push_str("        _ => None,\n");
    out.push_str("    }\n}\n\n");
    
    // Outputs (recursive, preserves relative directory structure)
    out.push_str("pub fn demo_output_bytes(path: &str) -> Option<&'static [u8]> {\n");
    out.push_str("    match path {\n");
    let outputs_dir = demo_dir.join("outputs");
    if outputs_dir.exists() {
        collect_files(&outputs_dir, &outputs_dir, &mut out);
    }
    out.push_str("        _ => None,\n");
    out.push_str("    }\n}\n");

    fs::write(&dest_path, out).unwrap();
}

fn collect_files(base: &Path, dir: &Path, out: &mut String) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            collect_files(base, &path, out);
        } else if path.is_file() {
            let rel_path = path.strip_prefix(base).unwrap();
            let rel_str = rel_path.to_string_lossy().into_owned();
            let abs_path = path.canonicalize().unwrap().to_string_lossy().into_owned();
            let abs_path_escaped = abs_path.replace("\\", "\\\\");
            out.push_str(&format!(
                "        \"{}\" => Some(include_bytes!(\"{}\")),\n",
                rel_str, abs_path_escaped
            ));
        }
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

    eprintln!("cargo:warning=Bundling {} …", src.display());

    let output = Command::new(&esbuild)
        .current_dir(workspace_root)
        .arg(normalized_path(src))
        .arg(format!("--outfile={}", normalized_path(out)))
        .arg("--bundle")
        .arg("--format=esm")
        .arg("--minify")
        .arg("--external:https://*")
        .arg("--external:http://*")
        .arg("--log-level=warning")
        .output()
        .expect("failed to run esbuild");

    if !output.status.success() {
        panic!(
            "esbuild bundle failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
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

// ── Extract build metadata ────────────────────────────────────────────────────────────────

fn extract_build_metadata() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    // Git information
    let git_hash =
        run("git", &["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".into());

    let git_hash_full = run("git", &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());

    let git_branch =
        run("git", &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|| "unknown".into());

    let git_tag =
        run("git", &["describe", "--tags", "--abrev=0"]).unwrap_or_else(|| "unknown".into());

    let git_dirty = Command::new("git")
        .args(["diff", "--quiet"])
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);

    // UTC timestamp
    let build_date =
        run("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]).unwrap_or_else(|| "unknown".into());

    // Profile and target from environment
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".into());
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());

    // Rust version
    let rustc_version = run("rustc", &["--version"]).unwrap_or_else(|| "unknown".into());

    println!("cargo:rustc-env=BUILD_DATE={build_date}");
    println!("cargo:rustc-env=PROFILE={profile}");
    println!("cargo:rustc-env=TARGET={target}");
    println!("cargo:rustc-env=GIT_TAG={git_tag}");
    println!("cargo:rustc-env=GIT_HASH={git_hash}");
    println!("cargo:rustc-env=GIT_HASH_FULL={git_hash_full}");
    println!("cargo:rustc-env=GIT_BRANCH={git_branch}");
    println!("cargo:rustc-env=GIT_DIRTY={git_dirty}");
    println!("cargo:rustc-env=RUSTC_VERSION={rustc_version}");
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

/// Run a command and return its trimmed stdout if it exits successfully.
fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(cmd).args(args).output().ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout)
        .ok()
        .map(|s| s.trim().to_string())
}
