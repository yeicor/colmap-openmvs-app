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

    // ── 4. Extract build metadata from Git, date, and Rust version ───────────
    extract_build_metadata();

    // ── 5. Copy icon into the Android project (if building for Android) ──────
    embed_android_icon(&manifest_dir);

    // ── 6. Generate demo assets if the demo feature is enabled ─────────────────
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

    let images_dir = demo_dir.join("images");
    let outputs_dir = demo_dir.join("outputs");

    // ===== LOAD OR GENERATE DATA =====

    // -- manifest.json --
    let manifest_path = demo_dir.join("manifest.json");
    let (manifest_str, images_from_manifest) = if manifest_path.exists() {
        let text = fs::read_to_string(&manifest_path).unwrap();
        if let Err(e) = serde_json::from_str::<serde_json::Value>(&text) {
            let preview: String = text.chars().take(200).collect();
            panic!(
                "Invalid JSON in {}: {}
First 200 characters:
{}",
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
        // Generate a minimal valid placeholder from whatever files exist on disk.
        // The full manifest is produced by `test_generate_demo_data` in the backend.
        println!(
            "cargo:warning=Demo manifest not found at {}. ",
            manifest_path.display(),
        );
        println!("cargo:warning=  Generating minimal placeholder from existing images/outputs.",);
        println!(
            "cargo:warning=  Run `cargo test --test generate_demo_data -p colmap-openmvs-backend` for full demo data.",
        );
        let manifest = generate_placeholder_manifest(&images_dir, &outputs_dir);
        let images = serde_json::from_str::<serde_json::Value>(&manifest)
            .ok()
            .and_then(|v| {
                v["project"]["images"].as_array().map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
            })
            .unwrap_or_default();
        (manifest, images)
    };

    // -- download_events.json --
    let download_events_path = demo_dir.join("download_events.json");
    let download_events_str = if download_events_path.exists() {
        let text = fs::read_to_string(&download_events_path).unwrap();
        if let Err(e) = serde_json::from_str::<serde_json::Value>(&text) {
            let preview: String = text.chars().take(200).collect();
            panic!(
                "Invalid JSON in {}: {}
First 200 characters:
{}",
                download_events_path.display(),
                e,
                preview,
            );
        }
        println!("cargo:rerun-if-changed={}", download_events_path.display());
        text
    } else {
        "[]".to_string()
    };

    // -- pipeline_events.json --
    let pipeline_events_path = demo_dir.join("pipeline_events.json");
    let pipeline_events_str = if pipeline_events_path.exists() {
        let text = fs::read_to_string(&pipeline_events_path).unwrap();
        if let Err(e) = serde_json::from_str::<serde_json::Value>(&text) {
            let preview: String = text.chars().take(200).collect();
            panic!(
                "Invalid JSON in {}: {}
First 200 characters:
{}",
                pipeline_events_path.display(),
                e,
                preview,
            );
        }
        println!("cargo:rerun-if-changed={}", pipeline_events_path.display());
        text
    } else {
        "[]".to_string()
    };

    // ===== VALIDATE IMAGES =====
    for image_name in &images_from_manifest {
        let image_path = images_dir.join(image_name);
        if !image_path.is_file() {
            panic!(
                "Demo image '{}' listed in manifest does not exist as a file.\n\
                 Expected file: {}\n\
                 Run `cargo test --test generate_demo_data -p colmap-openmvs-backend` to regenerate.",
                image_name,
                image_path.display(),
            );
        }
    }

    // ===== VALIDATE VIEWABLE OUTPUTS =====
    if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&manifest_str) {
        if let Some(outputs) = manifest["project"]["outputs"].as_array() {
            for output_val in outputs {
                let is_viewable = output_val["is_viewable"].as_bool().unwrap_or(false);
                let is_png = output_val["relative_path"]
                    .as_str()
                    .map(|p| p.ends_with(".png"))
                    .unwrap_or(false);
                if is_viewable || is_png {
                    let rel_path = output_val["relative_path"].as_str().unwrap_or_else(|| {
                        panic!("Expected string in manifest project.outputs[].relative_path");
                    });
                    let output_path = outputs_dir.join(rel_path);
                    if !output_path.is_file() {
                        panic!(
                            "Viewable demo output '{}' listed in manifest does not exist as a file.\n\
                             Expected file: {}\n\
                             Run `cargo test --test generate_demo_data -p colmap-openmvs-backend` to regenerate.",
                            rel_path,
                            output_path.display(),
                        );
                    }
                }
            }
        }
    }

    // ===== GENERATE RUST SOURCE =====

    let mut out = String::new();

    // 6. Dynamic raw string delimiter - pick a safe number of # that doesn't appear in content
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

    // Images (flat directory, no subdirectories)
    out.push_str("pub fn demo_image_bytes(name: &str) -> Option<&'static [u8]> {\n");
    out.push_str("    match name {\n");
    if images_dir.exists() {
        for entry in fs::read_dir(&images_dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name().unwrap().to_str().unwrap();
                let escaped_name = escape_path(name);
                let abs_path = path.canonicalize().unwrap().to_string_lossy().into_owned();
                let escaped_abs = escape_path(&abs_path);
                out.push_str(&format!(
                    "        \"{}\" => Some(include_bytes!(\"{}\")),\n",
                    escaped_name, escaped_abs
                ));
            }
        }
    }
    out.push_str("        _ => None,\n");
    out.push_str("    }\n}\n\n");

    // Outputs (recursive, preserves relative directory structure)
    out.push_str("pub fn demo_output_bytes(path: &str) -> Option<&'static [u8]> {\n");
    out.push_str("    match path {\n");
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
            let escaped_rel = escape_path(&rel_str);
            let escaped_abs = escape_path(&abs_path);
            out.push_str(&format!(
                "        \"{}\" => Some(include_bytes!(\"{}\")),\n",
                escaped_rel, escaped_abs
            ));
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

/// Generate a minimal valid placeholder manifest when the real one is missing.
///
/// This produces a valid JSON string that the DemoManifest struct can deserialize,
/// using only the files actually present in the images/ and outputs/ directories.
/// Complex fields (config_schema, runtime_info, etc.) get minimal valid stubs.
fn generate_placeholder_manifest(images_dir: &Path, outputs_dir: &Path) -> String {
    // Collect images from disk
    let images: Vec<String> = if images_dir.exists() {
        let mut list: Vec<String> = fs::read_dir(images_dir)
            .unwrap()
            .filter_map(|e| {
                let path = e.ok()?.path();
                if path.is_file() {
                    path.file_name()?.to_str().map(String::from)
                } else {
                    None
                }
            })
            .collect();
        list.sort();
        list
    } else {
        vec![]
    };

    // Collect outputs from disk (recursive)
    let outputs: Vec<serde_json::Value> = if outputs_dir.exists() {
        collect_output_files(outputs_dir, outputs_dir)
    } else {
        vec![]
    };

    let placeholder = serde_json::json!({
        "projects": [],
        "settings": {
            "projects_folder": "",
            "proot_binary_dir": "",
            "proot_images_dir": "",
            "default_image_tag": null,
            "custom_mounts": [],
            "settings_file_path": null
        },
        "dark_mode": null,
        "project": {
            "images": images,
            "config_schema": {
                "image_tag": "",
                "tools": [],
                "environment_variables": [],
                "build_date": ""
            },
            "project_config": {
                "image_tag": "",
                "environment_variables": [],
                "custom_script": null
            },
            "outputs": outputs,
            "run_status": {
                "is_running": false,
                "is_dry_run": false,
                "task_id": null,
                "progress": null
            }
        },
        "runtime_info": {
            "name": "none",
            "version": null,
            "installed": false,
            "supported": false,
            "unsupported_reason": null
        }
    });

    serde_json::to_string_pretty(&placeholder).unwrap()
}

/// Walk the outputs directory and produce the JSON array of output entries
/// matching the structure expected by the manifest's `project.outputs` field.
fn collect_output_files(base: &Path, dir: &Path) -> Vec<serde_json::Value> {
    let mut entries = vec![];
    if !dir.exists() {
        return entries;
    }
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            entries.extend(collect_output_files(base, &path));
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let metadata = fs::metadata(&path).unwrap();
            let size = metadata.len();
            let is_viewable = rel.ends_with(".png")
                || rel.ends_with(".ply")
                || rel.ends_with(".jpg")
                || rel.ends_with(".jpeg")
                || rel.ends_with(".bin");
            entries.push(serde_json::json!({
                "relative_path": rel,
                "name": path.file_name().unwrap().to_string_lossy(),
                "size": size,
                "modified_at": chrono::Utc::now().to_rfc3339(),
                "is_viewable": is_viewable
            }));
        }
    }
    entries
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
fn embed_android_icon(manifest_dir: &Path) {
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
                let new_name = "Photos to 3D Model Offline";
                if current != new_name {
                    let patched = format!("{desired}{new_name}</string>");
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
                        eprintln!("cargo:warning=Set Android app name to \"{new_name}\"");
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
