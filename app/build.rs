use base64::Engine as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Write a file only if its content has changed, preserving mtime to avoid
/// triggering rebuild loops from file watchers (e.g. dioxus serve).
fn write_if_changed(path: &Path, content: &str) {
    if let Ok(existing) = fs::read_to_string(path) {
        if existing == content {
            return;
        }
    }
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(path, content).unwrap();
}

/// Metadata for a single embedded binary file (image or output).
struct DemoFile {
    match_key: String,
    ident: String,
    b64_include_path: String,
}

#[cfg(target_family = "windows")]
const SHELL: &str = "cmd.exe";
#[cfg(target_family = "windows")]
const SHELL_ARG: &str = "/C";

#[cfg(not(target_family = "windows"))]
const SHELL: &str = "bash";
#[cfg(not(target_family = "windows"))]
const SHELL_ARG: &str = "-c";

fn read_app_name_from_dioxus_toml() -> String {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir.join("..").join("Dioxus.toml");
    let content = fs::read_to_string(&path).expect("Failed to read Dioxus.toml");

    // Find the [application] section
    let app_section_start = content
        .find("[application]")
        .expect("Missing [application] section in Dioxus.toml");
    let rest = &content[app_section_start..];

    // Find the name field within the application section
    let name_prefix = "name = \"";
    if let Some(name_start) = rest.find(name_prefix) {
        let value_start = name_start + name_prefix.len();
        if let Some(value_end) = rest[value_start..].find('"') {
            return rest[value_start..value_start + value_end].to_string();
        }
    }

    panic!("Could not find application name in Dioxus.toml");
}

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .join("..")
        .canonicalize()
        .expect("Failed to resolve workspace root");

    // ── App name from the single source of truth (Dioxus.toml) ───────────────
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join("Dioxus.toml").display()
    );
    let app_name = read_app_name_from_dioxus_toml();
    println!("cargo:rustc-env=APP_NAME={app_name}");

    // ── Rebuild triggers ─────────────────────────────────────────────────────
    let pkg_json_path = workspace_root.join("package.json");
    let viewer_src = manifest_dir.join("ts").join("index.ts");
    println!("cargo:rerun-if-changed={}", pkg_json_path.display());
    // Watch all TypeScript source files in the ts directory
    if let Ok(entries) = std::fs::read_dir(manifest_dir.join("ts")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("ts") {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }

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

    // ── 2. Bundle app/ts/index.ts → assets/viewer3d.bundle.js ────────────────
    bundle_viewer(&workspace_root, &viewer_src, &bundle_out);

    // ── 3. Copy eruda (debug builds only) ────────────────────────────────────
    if profile != "release" {
        copy_eruda(&workspace_root, &manifest_dir);
    }

    // ── 4. Extract build metadata from Git, date, and Rust version ───────────
    extract_build_metadata();

    // ── 5. Copy icon into the Android project (if building for Android) ──────
    embed_android_icon_and_app_name(&manifest_dir, &app_name);

    // ── 6. Generate demo assets (always, so the LSP can find the file) ──────
    if std::env::var("CARGO_FEATURE_DEMO").is_ok() {
        generate_demo_assets(&manifest_dir);
    } else {
        generate_demo_assets_stub(&manifest_dir);
    }
}

// ── Generate demo assets ──────────────────────────────────────────────────────

fn generate_demo_assets(manifest_dir: &Path) {
    let gen_dir = manifest_dir.join("gen");
    fs::create_dir_all(&gen_dir).ok();
    let dest_path = gen_dir.join("demo_assets_gen.rs");

    let target_dir = manifest_dir.parent().unwrap().join("target");

    let demo_dir = manifest_dir.join("assets").join("demo");
    println!("cargo:rerun-if-changed={}", demo_dir.display());

    let images_dir = demo_dir.join("images");
    let outputs_dir = demo_dir.join("outputs");

    // ── Base64 output directory (in target/, outside source tree to avoid rebuild loop) ─
    let base64_dir = target_dir.join("demo_base64");
    if base64_dir.exists() {
        fs::remove_dir_all(&base64_dir).unwrap();
    }

    // ── Auto-generate demo data if needed ───────────────────────────────
    let manifest_path = demo_dir.join("manifest.json");
    if !manifest_path.exists() || !outputs_dir.exists() {
        println!(
            "cargo:warning=Demo data missing. Running `cargo test --test generate_demo_data -p colmap-openmvs-backend` to auto-generate..."
        );
        run_demo_data_generation(manifest_dir, &demo_dir);
    }

    // ===== LOAD OR GENERATE DATA =====

    // -- manifest.json --
    let (manifest_str, images_from_manifest) = if manifest_path.exists() {
        let text = fs::read_to_string(&manifest_path).unwrap();
        if let Err(e) = serde_json::from_str::<serde_json::Value>(&text) {
            let preview: String = text.chars().take(200).collect();
            panic!(
                "Invalid JSON in {}: {}\nFirst 200 characters:\n{}",
                manifest_path.display(),
                e,
                preview,
            );
        }
        let images: Vec<String> = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v["project"]["images"].as_array().map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
            })
            .unwrap_or_default();
        (text, images)
    } else {
        panic!(
            "Demo manifest.json not found at {}.\n\
             Auto-generation via `cargo test --test generate_demo_data -p colmap-openmvs-backend`\n\
             was attempted but did not produce the expected file.\n\
             Ensure Docker or PRoot is available, then try running the command manually.",
            manifest_path.display(),
        );
    };

    // ===== CHECK EXPECTED FILES EXIST =====
    if !images_from_manifest.is_empty() && !images_dir.exists() {
        println!("cargo:warning=Demo images directory missing. Attempting auto-generation...");
        run_demo_data_generation(manifest_dir, &demo_dir);
        if !images_dir.exists() {
            panic!(
                "Demo images directory missing: {}.\n\
                 Auto-generation was attempted but did not produce the expected directory.\n\
                 Ensure Docker or PRoot is available, then run:\n\
                 `cargo test --test generate_demo_data -p colmap-openmvs-backend`",
                images_dir.display(),
            );
        }
    }
    if !outputs_dir.exists() {
        println!("cargo:warning=Demo outputs directory missing. Attempting auto-generation...");
        run_demo_data_generation(manifest_dir, &demo_dir);
        if !outputs_dir.exists() {
            panic!(
                "Demo outputs directory missing: {}.\n\
                 Auto-generation was attempted but did not produce the expected directory.\n\
                 Ensure Docker or PRoot is available, then run:\n\
                 `cargo test --test generate_demo_data -p colmap-openmvs-backend`",
                outputs_dir.display(),
            );
        }
    }

    // -- download_events.json (optional) --
    let download_events_path = demo_dir.join("download_events.json");
    let download_events_str = if download_events_path.exists() {
        let text = fs::read_to_string(&download_events_path)
            .unwrap_or_else(|e| panic!("Cannot read {}: {e}", download_events_path.display()));
        println!("cargo:rerun-if-changed={}", download_events_path.display());
        text
    } else {
        "[]".to_string()
    };

    // -- pipeline_events.json (optional) --
    let pipeline_events_path = demo_dir.join("pipeline_events.json");
    let pipeline_events_str = if pipeline_events_path.exists() {
        let text = fs::read_to_string(&pipeline_events_path)
            .unwrap_or_else(|e| panic!("Cannot read {}: {e}", pipeline_events_path.display()));
        println!("cargo:rerun-if-changed={}", pipeline_events_path.display());
        text
    } else {
        "[]".to_string()
    };

    // ===== GENERATE RUST SOURCE (base64 + include_str!; no include_bytes!) =====

    let mut out = String::new();

    // Dynamic raw string delimiter - pick a safe number of # that doesn't appear in content
    let manifest_hashes = "#".repeat(safe_raw_delimiter(&manifest_str));
    let download_events_hashes = "#".repeat(safe_raw_delimiter(&download_events_str));
    let pipeline_events_hashes = "#".repeat(safe_raw_delimiter(&pipeline_events_str));

    out.push_str(&format!(
        "pub const DEMO_MANIFEST: &str = r{0}\"{1}\"{0};\n\n",
        manifest_hashes, manifest_str
    ));
    out.push_str(&format!(
        "pub const DOWNLOAD_EVENTS_JSON: &str = r{0}\"{1}\"{0};\n\n",
        download_events_hashes, download_events_str
    ));
    out.push_str(&format!(
        "pub const PIPELINE_EVENTS_JSON: &str = r{0}\"{1}\"{0};\n\n",
        pipeline_events_hashes, pipeline_events_str
    ));

    // ===== Process binary files: base64-encode and generate include_str! constants =====
    // include_str! returns &str — the LSP handles this natively without size inference
    // (unlike include_bytes! which returns &[u8; N] and triggers "type annotations needed").
    // At runtime, OnceLock caches the decoded bytes. An init_demo_data() function pre-decodes
    // everything at startup so there's no lazy-decode latency on first access.
    let engine = base64::engine::general_purpose::STANDARD;

    let mut image_files: Vec<DemoFile> = Vec::new();
    let mut output_files: Vec<DemoFile> = Vec::new();

    if images_dir.exists() {
        fs::create_dir_all(&base64_dir.join("images")).unwrap();
        for entry in fs::read_dir(&images_dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name().unwrap().to_str().unwrap().to_string();
                let escaped_name = escape_path(&name);
                let ident = to_ident(&name);

                let raw_bytes = fs::read(&path).unwrap();
                let b64 = engine.encode(&raw_bytes);
                let b64_filename = format!("{}.b64", name);
                fs::write(base64_dir.join("images").join(&b64_filename), &b64).unwrap();

                image_files.push(DemoFile {
                    match_key: escaped_name,
                    ident,
                    b64_include_path: format!(
                        "concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../target/demo_base64/images/{}\")",
                        escape_path(&b64_filename)
                    ),
                });
            }
        }
    }

    if outputs_dir.exists() {
        collect_output_statics(
            &outputs_dir,
            &outputs_dir,
            &mut output_files,
            &base64_dir,
            &engine,
        );
    }

    // ── Section 1: include_str! + concat!(CARGO_MANIFEST_DIR, ...) for each file ──
    // concat! is evaluated at compile time to build an absolute path to the base64 file.
    // This works regardless of whether the generated file is in OUT_DIR or the source tree.
    for f in image_files.iter().chain(output_files.iter()) {
        out.push_str(&format!(
            "pub const {}_B64: &str = include_str!({});\n",
            f.ident, f.b64_include_path
        ));
    }

    // ── Section 2: OnceLock statics for each file ──────────────────────────
    if !image_files.is_empty() || !output_files.is_empty() {
        out.push_str("\n");
    }
    for f in image_files.iter().chain(output_files.iter()) {
        out.push_str(&format!(
            "#[allow(non_snake_case)]\nstatic {}: OnceLock<Vec<u8>> = OnceLock::new();\n",
            f.ident
        ));
    }

    // ── Section 3: Init function — decodes all data at startup ─────────────
    if !image_files.is_empty() || !output_files.is_empty() {
        out.push_str("\npub fn init_demo_data() {\n");
        out.push_str("    use base64::Engine as _;\n");
        out.push_str("    let engine = base64::engine::general_purpose::STANDARD;\n");
        for f in image_files.iter().chain(output_files.iter()) {
            out.push_str(&format!(
                "    {}.get_or_init(|| engine.decode({}_B64).unwrap());\n",
                f.ident, f.ident
            ));
        }
        out.push_str("}\n");
    }

    // ── Section 4: Lookup functions ───────────────────────────────────────
    out.push_str("\npub fn demo_image_bytes(name: &str) -> Option<&'static [u8]> {\n");
    out.push_str("    use base64::Engine as _;\n");
    out.push_str("    let engine = base64::engine::general_purpose::STANDARD;\n");
    out.push_str("    match name {\n");
    for f in &image_files {
        out.push_str(&format!(
            "        \"{}\" => Some({}.get_or_init(|| engine.decode({}_B64).unwrap()).as_slice()),\n",
            f.match_key, f.ident, f.ident
        ));
    }
    out.push_str("        _ => None,\n");
    out.push_str("    }\n}\n\n");

    out.push_str("pub fn demo_output_bytes(path: &str) -> Option<&'static [u8]> {\n");
    out.push_str("    use base64::Engine as _;\n");
    out.push_str("    let engine = base64::engine::general_purpose::STANDARD;\n");
    out.push_str("    match path {\n");
    for f in &output_files {
        out.push_str(&format!(
            "        \"{}\" => Some({}.get_or_init(|| engine.decode({}_B64).unwrap()).as_slice()),\n",
            f.match_key, f.ident, f.ident
        ));
    }
    out.push_str("        _ => None,\n");
    out.push_str("    }\n}\n");

    write_if_changed(&dest_path, &out);
}

/// Generate a minimal stub so the `include!` in demo.rs always resolves.
fn generate_demo_assets_stub(manifest_dir: &Path) {
    let gen_dir = manifest_dir.join("gen");
    fs::create_dir_all(&gen_dir).ok();
    let dest_path = gen_dir.join("demo_assets_gen.rs");
    let content = r#"pub const DEMO_MANIFEST: &str = "{}";
pub const DOWNLOAD_EVENTS_JSON: &str = "[]";
pub const PIPELINE_EVENTS_JSON: &str = "[]";
pub fn init_demo_data() {}
pub fn demo_image_bytes(_name: &str) -> Option<&'static [u8]> { None }
pub fn demo_output_bytes(_path: &str) -> Option<&'static [u8]> { None }"#;
    write_if_changed(&dest_path, content);
}

fn to_ident(s: &str) -> String {
    // Convert a filename to a valid Rust identifier (UPPER_CASE for static naming)
    let mut result = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            result.push(ch.to_ascii_uppercase());
        } else {
            result.push('_');
        }
    }
    if result.is_empty() || result.starts_with(|c: char| c.is_numeric()) {
        result.insert(0, '_');
    }
    result
}

fn collect_output_statics(
    base: &Path,
    dir: &Path,
    files: &mut Vec<DemoFile>,
    base64_dir: &Path,
    engine: &base64::engine::general_purpose::GeneralPurpose,
) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            collect_output_statics(base, &path, files, base64_dir, engine);
        } else if path.is_file() {
            let rel_path = path.strip_prefix(base).unwrap();
            let rel_str = rel_path.to_string_lossy().into_owned();
            let escaped_rel = escape_path(&rel_str);
            let ident = to_ident(&rel_str);

            let raw_bytes = fs::read(&path).unwrap();
            let b64 = engine.encode(&raw_bytes);

            // Preserve subdirectory structure in the base64 output
            let parent_rel = rel_path.parent().and_then(|p| p.to_str()).unwrap_or("");
            let b64_subdir = base64_dir.join(parent_rel);
            fs::create_dir_all(&b64_subdir).unwrap();

            let filename = path.file_name().unwrap().to_str().unwrap();
            let b64_filename = format!("{filename}.b64");
            fs::write(b64_subdir.join(&b64_filename), &b64).unwrap();

            files.push(DemoFile {
                match_key: escaped_rel,
                ident,
                b64_include_path: format!(
                    "concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../target/demo_base64/{}.b64\")",
                    escape_path(&rel_str)
                ),
            });
        }
    }
}

/// Determine the minimum number of `#` characters needed for a safe raw string
/// delimiter `r#"..."#` so that the content does not contain the closing sequence.
fn safe_raw_delimiter(content: &str) -> usize {
    let mut n = 3;
    loop {
        let seq = format!("\"{}", "#".repeat(n));
        if !content.contains(&seq) {
            return n;
        }
        n += 1;
    }
}

/// Escape a string for use in a Rust string literal, handling `\\` and `"`.
fn escape_path(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\"', "\\\"")
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
    // Hash ALL .ts source files (not just the entry point) so that editing
    // any imported module like viewer3d-class.ts, state.ts, utils.ts, etc.
    // invalidates the cached bundle and triggers a rebuild.
    let mut combined = String::new();
    let ts_dir = src.parent().expect("src has no parent directory");
    if let Ok(entries) = std::fs::read_dir(ts_dir) {
        let mut files: Vec<_> = entries
            .flatten()
            .filter(|e| e.path().extension().and_then(|e| e.to_str()) == Some("ts"))
            .collect();
        // Sort for deterministic hash order across filesystem iterations.
        files.sort_by_key(|e| e.file_name());
        for entry in &files {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                combined.push_str(&content);
            }
        }
    }
    let hash = fnv64(combined.as_bytes());

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

// ── Extract build metadata ────────────────────────────────────────────────

fn extract_build_metadata() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    // Git information (pure Rust — reads .git files directly)
    let head_commit = read_git_head();

    let git_hash = head_commit
        .as_ref()
        .map(|h| h[..12.min(h.len())].to_string())
        .unwrap_or_else(|| "unknown".into());

    let git_hash_full = head_commit.unwrap_or_else(|| "unknown".into());

    let git_branch = read_git_branch().unwrap_or_else(|| "unknown".into());

    let git_tag = find_latest_tag().unwrap_or_else(|| "unknown".into());

    let git_dirty = false;

    // UTC timestamp (pure Rust — chrono)
    let build_date = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

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

/// Read the current commit hash from HEAD (resolving refs if needed).
fn read_git_head() -> Option<String> {
    let head_path = Path::new(".git").join("HEAD");
    let head = std::fs::read_to_string(&head_path).ok()?;
    let head = head.trim().to_string();
    if let Some(ref_path) = head.strip_prefix("ref: ") {
        let ref_file = Path::new(".git").join(ref_path);
        std::fs::read_to_string(&ref_file)
            .ok()
            .map(|s| s.trim().to_string())
    } else {
        Some(head)
    }
}

/// Read the current branch name from HEAD.
fn read_git_branch() -> Option<String> {
    let head_path = Path::new(".git").join("HEAD");
    let head = std::fs::read_to_string(&head_path).ok()?;
    let head = head.trim().to_string();
    head.strip_prefix("ref: refs/heads/").map(|s| s.to_string())
}

/// Find the latest tag by listing .git/refs/tags/.
fn find_latest_tag() -> Option<String> {
    let tags_dir = Path::new(".git").join("refs").join("tags");
    if !tags_dir.is_dir() {
        return None;
    }
    let mut tags: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(tags_dir) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    tags.push(name.to_string());
                }
            }
        }
    }
    tags.sort();
    tags.last().cloned()
}

// ── Android-specific patching ────────────────────────────────────────────────

/// Customise the generated Android project:
///
/// 1. **App name** — Overrides `res/values/strings.xml` with the canonical
///    Play Store display name.
///
/// 2. **Icon** — Resizes `assets/icon.png` into each density-specific
///    `mipmap-*dpi/ic_launcher.webp` and generates a properly padded
///    `ic_launcher_foreground.webp` for adaptive icons (API 26+).  Rewrites
///    `mipmap-anydpi-v26/ic_launcher.xml` to use our foreground so the icon
///    respects squircle / circle / rounded-square masks on modern Android.
///
///    (Dioxus CLI issue #3685 — the generated project always uses placeholder
///    icons regardless of `Dioxus.toml`'s `[bundle] icon` setting.)
fn embed_android_icon_and_app_name(manifest_dir: &Path, app_name: &str) {
    let profile = std::env::var("PROFILE").unwrap_or_default();

    // Compute the Android project's resource root WITHOUT canonicalize (which
    // would fail if parts of the path don't exist yet).
    let android_res = manifest_dir
        .join("..")
        .join("target")
        .join("dx")
        .join("colmap-openmvs-app")
        .join(&profile)
        .join("android")
        .join("app")
        .join("app")
        .join("src")
        .join("main")
        .join("res");

    if !android_res.is_dir() {
        // Not building for Android (or `dx bundle` hasn't generated the project yet).
        return;
    }

    // ── App name ──────────────────────────────────────────────────────────
    let strings_xml = android_res.join("values").join("strings.xml");
    if strings_xml.is_file() {
        let content = fs::read_to_string(&strings_xml).unwrap_or_default();
        let desired = r#"<string name="app_name">"#;
        if let Some(start) = content.find(desired) {
            let after_open = start + desired.len();
            if let Some(end) = content[after_open..].find("</string>") {
                let current = &content[after_open..after_open + end];
                if current != app_name {
                    let patched = format!("{desired}{app_name}</string>");
                    // Skip past the old </string> closing tag so we don't end up
                    // with a duplicate (patched already contains it).
                    let close_tag_len = "</string>".len();
                    let new_content = format!(
                        "{}{}{}",
                        &content[..start],
                        patched,
                        &content[after_open + end + close_tag_len..]
                    );
                    if fs::write(&strings_xml, &new_content).is_ok() {
                        eprintln!("cargo:warning=Set Android app name to \"{app_name}\"");
                    }
                }
            }
        }
    }

    // ── Icon ──────────────────────────────────────────────────────────────
    let icon_src = manifest_dir
        .join("assets")
        .join("icon.png")
        .canonicalize()
        .unwrap_or_else(|_| manifest_dir.join("assets").join("icon.png"));
    if !icon_src.is_file() {
        eprintln!("cargo:warning=icon.png not found at {}", icon_src.display());
        return;
    }

    println!("cargo:rerun-if-changed={}", icon_src.display());

    // Density bucket → pixel size at that bucket (launcher icon).
    // Adaptive-icon viewport = 108dp, safe-zone (inner 72dp) = 2/3 of viewport.
    // Foreground content should occupy the inner 2/3 of the canvas so it is
    // never clipped by the mask (squircle, circle, rounded-square).
    const DENSITIES: &[(&str, u32)] = &[
        ("mdpi", 48),
        ("hdpi", 72),
        ("xhdpi", 96),
        ("xxhdpi", 144),
        ("xxxhdpi", 192),
    ];

    let img = match image::open(&icon_src) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("cargo:warning=Failed to open icon.png: {e}");
            return;
        }
    };

    let mut any_written = false;
    for &(density, size) in DENSITIES {
        let mipmap_dir = android_res.join(format!("mipmap-{density}"));
        if !mipmap_dir.is_dir() {
            continue;
        }

        // ── Flat (non-adaptive) icon — used as fallback on pre-API-26 ──
        let flat = img.resize_exact(size, size, image::imageops::FilterType::Lanczos3);
        if let Err(e) = flat.save(&mipmap_dir.join("ic_launcher.webp")) {
            eprintln!("cargo:warning=Failed to write ic_launcher.webp (mipmap-{density}): {e}");
        } else {
            any_written = true;
        }

        // ── Adaptive-icon foreground — full-bleed, same as flat icon ─────
        // The user prefers the icon as large as possible and accepts that the
        // device mask (squircle, circle, etc.) may clip edges.
        if let Err(e) = flat.save(&mipmap_dir.join("ic_launcher_foreground.webp")) {
            eprintln!(
                "cargo:warning=Failed to write ic_launcher_foreground.webp (mipmap-{density}): {e}"
            );
        } else {
            eprintln!("cargo:warning=Updated Android launcher icon  mipmap-{density}");
        }
    }

    if !any_written {
        return;
    }

    // ── Repurpose the adaptive-icon XML ───────────────────────────────────
    // The generated project includes `mipmap-anydpi-v26/ic_launcher.xml` which
    // references the default Dioxus vector drawables.  We rewrite it to point
    // at our foreground bitmap so the icon respects the device's mask shape.
    // The existing `@drawable/ic_launcher_background` (solid green grid) is
    // kept as-is — it provides a neutral backing behind the masked icon.
    let anydpi_dir = android_res.join("mipmap-anydpi-v26");
    let anydpi_xml = anydpi_dir.join("ic_launcher.xml");
    let adaptive_icon_xml = r#"<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@drawable/ic_launcher_background" />
    <foreground android:drawable="@mipmap/ic_launcher_foreground" />
</adaptive-icon>
"#;
    // Also write a round-icon variant so round launchers (Pixel, etc.) also
    // show our icon properly.
    let round_xml = anydpi_dir.join("ic_launcher_round.xml");
    let round_icon_xml = r#"<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@drawable/ic_launcher_background" />
    <foreground android:drawable="@mipmap/ic_launcher_foreground" />
</adaptive-icon>
"#;

    if fs::create_dir_all(&anydpi_dir).is_ok() {
        if let Err(e) = fs::write(&anydpi_xml, adaptive_icon_xml) {
            eprintln!(
                "cargo:warning=Failed to write {}: {e}",
                anydpi_xml.display()
            );
        }
        if let Err(e) = fs::write(&round_xml, round_icon_xml) {
            eprintln!("cargo:warning=Failed to write {}: {e}", round_xml.display());
        }
        eprintln!("cargo:warning=Adaptive-icon XML updated — icon now respects mask shapes");
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

/// Detect the host target triple by parsing `rustc -vV`.
fn get_host_target() -> String {
    let output = Command::new("rustc")
        .args(["-vV"])
        .output()
        .expect("Failed to run `rustc -vV` to determine host target");
    let stdout = String::from_utf8(output.stdout).expect("rustc output is not valid UTF-8");
    for line in stdout.lines() {
        if let Some(target) = line.strip_prefix("host: ") {
            return target.to_string();
        }
    }
    panic!("Could not determine host target from rustc -vV:\n{stdout}",);
}

/// Run the demo data generation test in an isolated sandboxed environment.
///
/// - Uses the host target (so cross-compilation does not interfere).
/// - Sets `CARGO_TARGET_DIR` to a temporary directory to avoid concurrent
///   cargo lock conflicts with the ongoing build.
/// - Sets `DEMO_ASSETS_DIR` so the test writes output into the proper location.
/// - Starts with a clean environment to avoid cross-contamination.
fn run_demo_data_generation(manifest_dir: &Path, demo_dir: &Path) {
    let workspace_root = manifest_dir
        .join("..")
        .canonicalize()
        .expect("Failed to resolve workspace root");

    let host_target = get_host_target();

    // Create a separate target directory to avoid cargo lock contention
    let sandbox_target =
        std::env::temp_dir().join(format!("colmap-openmvs-demo-gen-{}", std::process::id()));
    let _ = fs::remove_dir_all(&sandbox_target);
    fs::create_dir_all(&sandbox_target).expect("Failed to create sandbox CARGO_TARGET_DIR");

    let output = Command::new("cargo")
        .args([
            "test",
            "--test",
            "generate_demo_data",
            "-p",
            "colmap-openmvs-backend",
            "--target",
            &host_target,
        ])
        .current_dir(&workspace_root)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .env(
            "CARGO_HOME",
            std::env::var("CARGO_HOME").unwrap_or_default(),
        )
        .env(
            "CARGO_TARGET_DIR",
            sandbox_target.to_str().expect("non-UTF-8 sandbox path"),
        )
        .env(
            "DEMO_ASSETS_DIR",
            demo_dir.to_str().expect("non-UTF-8 demo path"),
        )
        .output()
        .expect("Failed to execute cargo test for demo data generation");

    // Clean up sandbox target directory
    let _ = fs::remove_dir_all(&sandbox_target);

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "Demo data generation command failed (exit code: {:?}).\n\
             stdout:\n{}\nstderr:\n{}\n\n\
             Ensure Docker or PRoot is available, then run manually:\n\
             $ cargo test --test generate_demo_data -p colmap-openmvs-backend\n",
            output.status.code(),
            stdout,
            stderr,
        );
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
