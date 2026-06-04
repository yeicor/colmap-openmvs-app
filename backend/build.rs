/// Build script for colmap-openmvs-app's backend.
///
/// When building for Android (target contains "android"):
///   1. Exports a Docker image filesystem.
///   2. Splits files into ELF binaries (→ jniLibs as `librootfs-<hash16>.so`)
///      and non-ELF files (→ rootfs.zip embedded in the binary via `include_bytes!`)
///   3. Downloads proot + loader + libtalloc from Termux
///   4. Applies patchelf to the proot binary (RPATH=$ORIGIN, libtalloc rename)
///   5. Emits `EMBEDDED_IMAGE_TAG` so the app can read it at runtime
///   6. Writes `.rootfs_cache_dir` marker for the Gradle preBuild task
///   7. Patches the Gradle project (extractNativeLibs, useLegacyPackaging,
///      SDK versions, versionCode/Name, preBuild copy task)
///
/// Environment variable (optional):
///   `DOCKER_IMAGE` – Docker image tag to export
///                    (default: mirror.gcr.io/yeicor/colmap-openmvs:cpu-latest)
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

// Import from build-dependencies.
use serde::Serialize;
use zip::{CompressionMethod, ZipWriter};

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let target = std::env::var("TARGET").unwrap_or_default();
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());

    if !target.contains("android") {
        return;
    }

    let rootfs_zip_dest = out_dir.join("rootfs.zip");

    // ── Android-specific setup ──────────────────────────────────────────

    let docker_image = std::env::var("DOCKER_IMAGE")
        .unwrap_or_else(|_| "mirror.gcr.io/yeicor/colmap-openmvs:cpu-latest".to_string());

    let (docker_platform, _arch_abi, termux_arch) = match target.as_str() {
        t if t.contains("aarch64") => ("linux/arm64", "arm64-v8a", "aarch64"),
        t if t.contains("x86_64") => ("linux/amd64", "x86_64", "x86_64"),
        _ => panic!("Unsupported Android target: {target}"),
    };

    let cache = CacheDir::new(&docker_image, docker_platform, &target);

    // Step 1 – Download runtime prerequisites (proot, loader, libtalloc).
    download_prerequisites(&cache, termux_arch);

    // Step 2 – Export the Docker image filesystem as a tar archive.
    export_rootfs(&cache, &docker_image, docker_platform);

    // Step 3 – Split rootfs: ELF → rootfs_binaries/ (by hash), non-ELF → rootfs.zip.
    let img_cfg = image_config(&docker_image, docker_platform);
    build_rootfs_artifacts(&cache, docker_image.clone(), img_cfg);

    // Step 4 – Copy rootfs.zip to OUT_DIR so include_bytes! picks it up.
    std::fs::copy(cache.rootfs_zip(), &rootfs_zip_dest).expect("copy rootfs.zip to OUT_DIR");

    // Step 5 – Set EMBEDDED_IMAGE_TAG for runtime use.
    println!("cargo:rustc-env=EMBEDDED_IMAGE_TAG={}", docker_image);

    // Step 6 – Write a marker with the cache path so that the Gradle project
    //          (or external tooling) can find the rootfs cache after dx bundle
    //          has finished its own project generation.
    write_cache_marker(&profile, &cache);

    // Step 7 – Patch the Gradle project (useLegacyPackaging, SDK versions,
    //          versionCode, extractNativeLibs) and copy assets directly
    //          into jniLibs (proot, loader, libtalloc, ELF files).
    //          Also removes stale ABI jniLibs and the Gradle build cache so
    //          that a sequential arm64 + x86_64 build succeeds without error.
    patch_gradle_project(&profile);
    copy_assets_to_jnilibs(&profile, &cache);
    patch_proot_binary(&cache);
    remove_stale_build_artifacts(&profile);

    // ── Change-detection for cargo ───────────────────────────────────────
    //
    // Cargo skips re-running build.rs when none of the rerun-if-* paths/
    // env-vars have changed.  However, we MUST run every time for Android
    // because `dx bundle` regenerates the entire Gradle project (including
    // jniLibs) AFTER cargo build finishes.  If cargo skips build.rs, the
    // next `dx bundle` invocation will find a fresh Gradle project with
    // no rootfs ELF binaries in jniLibs.
    //
    // The solution: write a monotonic trigger file and point
    // `rerun-if-changed` at it.  Every build.rs run touches this file with
    // a new value, guaranteeing that Cargo always sees a change and
    // re-runs the build script.
    let trigger = project_root()
        .join("target")
        .join("android-cache")
        .join(".build-trigger");
    let _ = std::fs::write(
        &trigger,
        format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        ),
    );
    println!("cargo:rerun-if-changed={}", trigger.display());
}

// ---------------------------------------------------------------------------
// Cache directory
// ---------------------------------------------------------------------------

struct CacheDir {
    root: PathBuf,
}

impl CacheDir {
    fn new(image: &str, platform: &str, target: &str) -> Self {
        let digest = image_digest(image, platform);
        let key = fnv1a_hex(&format!("{image}|{digest}|{platform}|{target}|v3"));
        let root = project_root()
            .join("target")
            .join("android-cache")
            .join(&key[..16]);
        std::fs::create_dir_all(&root).expect("create cache dir");
        CacheDir { root }
    }

    fn proot(&self) -> PathBuf {
        self.root.join("proot")
    }
    fn loader(&self) -> PathBuf {
        self.root.join("loader")
    }
    fn libtalloc(&self) -> PathBuf {
        self.root.join("libtalloc.so.2")
    }
    fn rootfs_tar(&self) -> PathBuf {
        self.root.join("rootfs.tar")
    }
    fn rootfs_zip(&self) -> PathBuf {
        self.root.join("rootfs.zip")
    }
    fn rootfs_binaries_dir(&self) -> PathBuf {
        self.root.join("rootfs_binaries")
    }
    fn stamp(&self, name: &str) -> PathBuf {
        self.root.join(format!(".stamp_{name}"))
    }
}

// ---------------------------------------------------------------------------
// Docker helpers
// ---------------------------------------------------------------------------

fn image_digest(image: &str, platform: &str) -> String {
    let status = Command::new("docker")
        .args(["pull", "--platform", platform, "--quiet", image])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("docker pull failed — is Docker installed and running?");
    assert!(
        status.success(),
        "docker pull failed for {image} ({platform})"
    );

    let output = Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", image])
        .output()
        .expect("docker image inspect failed");
    assert!(output.status.success(), "docker inspect failed for {image}");
    String::from_utf8(output.stdout)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn export_docker_tar(image: &str, platform: &str, dest: &Path) {
    let cid_output = Command::new("docker")
        .args(["create", "--platform", platform, image])
        .output()
        .expect("docker create failed");
    assert!(
        cid_output.status.success(),
        "docker create failed for {image}"
    );
    let cid = String::from_utf8(cid_output.stdout)
        .unwrap_or_default()
        .trim()
        .to_string();

    let tar_file = std::fs::File::create(dest).expect("create rootfs.tar");
    let mut child = Command::new("docker")
        .args(["export", &cid])
        .stdout(tar_file)
        .spawn()
        .expect("docker export spawn failed");
    let status = child.wait().expect("docker export wait failed");
    assert!(status.success(), "docker export failed");

    let _ = Command::new("docker").args(["rm", "-f", &cid]).output();
}

// ---------------------------------------------------------------------------
// Termux package downloads
// ---------------------------------------------------------------------------

fn download_prerequisites(cache: &CacheDir, termux_arch: &str) {
    if cache.stamp("prereq").exists() {
        return;
    }
    let base = "https://packages.termux.dev/apt/termux-main/pool/main";

    download_and_extract_deb(
        &format!("{base}/p/proot/"),
        "proot",
        termux_arch,
        &cache.root,
        &cache.proot(),
        &cache.loader(),
    );
    download_and_extract_deb(
        &format!("{base}/libt/libtalloc/"),
        "libtalloc",
        termux_arch,
        &cache.root,
        &cache.libtalloc(),
        &PathBuf::new(),
    );

    std::fs::write(cache.stamp("prereq"), b"").ok();
}

fn download_and_extract_deb(
    repo_url: &str,
    package: &str,
    arch: &str,
    _work_dir: &Path,
    dest_bin: &Path,
    dest_loader: &Path,
) {
    // Find the latest .deb by scraping the directory listing.
    let html = fetch_url(repo_url);
    let deb_name = find_latest_deb(&html, package, arch)
        .unwrap_or_else(|| panic!("no {package} .deb found for {arch}"));

    let deb_url = format!("{repo_url}{deb_name}");
    let deb_bytes = fetch_url_bytes(&deb_url);
    eprintln!("  [{package}] downloaded {} bytes", deb_bytes.len());

    let compressed = extract_data_tar_from_ar(&deb_bytes);
    let data = decompress_xz(&compressed);
    let mut archive = tar::Archive::new(Cursor::new(&data));

    for entry in archive.entries().expect("tar entries") {
        let mut entry = entry.expect("tar entry");
        let path = entry.path().expect("entry path").into_owned();

        if package == "proot" {
            if path.ends_with("usr/bin/proot") {
                entry.unpack(dest_bin).expect("extract proot");
                set_executable(dest_bin);
                eprintln!("  [proot] → proot");
            } else if path.to_string_lossy().contains("libexec/proot/loader")
                && !path.to_string_lossy().contains("libexec/proot/loader32")
                && !dest_loader.as_os_str().is_empty()
            {
                entry.unpack(dest_loader).expect("extract loader");
                set_executable(dest_loader);
                eprintln!("  [loader] → loader");
            }
        } else if package == "libtalloc" {
            if let Some(name) = path.file_name() {
                if name.to_string_lossy().starts_with("libtalloc.so") {
                    entry.unpack(dest_bin).expect("extract libtalloc");
                    eprintln!("  [libtalloc] → libtalloc.so");
                }
            }
        }
    }
}

/// Simple HTML scraper that finds the newest `.deb` matching `package` + `arch`.
fn find_latest_deb(html: &str, package: &str, arch: &str) -> Option<String> {
    let mut best: Option<String> = None;
    for line in html.lines() {
        // Look for href="<something>.deb"
        let mut pos = 0;
        while let Some(href_start) = line[pos..].find("href=\"") {
            let val_start = pos + href_start + 6;
            if let Some(quote_end) = line[val_start..].find('"') {
                let val = &line[val_start..val_start + quote_end];
                if val.ends_with(".deb") && val.contains(package) && val.contains(arch) {
                    // Keep the last match (alphabetically last = newest version).
                    best = Some(val.to_string());
                }
            }
            pos = val_start + 1;
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Rootfs export & splitting
// ---------------------------------------------------------------------------

fn export_rootfs(cache: &CacheDir, image: &str, platform: &str) {
    if cache.stamp("rootfs_export").exists() {
        return;
    }
    eprintln!("  Exporting rootfs from {image} …");
    export_docker_tar(image, platform, &cache.rootfs_tar());
    std::fs::write(cache.stamp("rootfs_export"), b"").ok();
}

/// Extract the runtime config (env, entrypoint, cmd, working_dir) from a
/// Docker image.  These are baked into the rootfs manifest so the PRoot
/// runtime can reconstruct the exact container environment.
fn image_config(image: &str, platform: &str) -> ImageConfig {
    let output = Command::new("docker")
        .args([
            "image",
            "inspect",
            "--format",
            "{{json .Config}}",
            "--platform",
            platform,
            image,
        ])
        .output()
        .expect("docker image inspect failed");
    assert!(output.status.success(), "docker inspect failed for {image}");

    let stdout = String::from_utf8(output.stdout).unwrap_or_default();
    #[allow(non_snake_case)]
    #[derive(serde::Deserialize)]
    struct DockerConfig {
        #[serde(default)]
        Env: Vec<String>,
        #[serde(default)]
        Entrypoint: Option<Vec<String>>,
        #[serde(default)]
        Cmd: Option<Vec<String>>,
        #[serde(default)]
        WorkingDir: String,
    }
    let cfg: DockerConfig = serde_json::from_str(&stdout).expect("parse docker config JSON");

    ImageConfig {
        env: cfg.Env,
        entrypoint: cfg.Entrypoint,
        cmd: cfg.Cmd,
        working_dir: if cfg.WorkingDir.is_empty() {
            None
        } else {
            Some(cfg.WorkingDir)
        },
    }
}

fn build_rootfs_artifacts(cache: &CacheDir, tag: String, config: ImageConfig) {
    // Stale cache cleanup: old builds used `rootfs_files` instead of `rootfs_binaries`.
    let old_dir = cache.root.join("rootfs_files");
    if old_dir.exists() {
        let _ = std::fs::remove_dir_all(&old_dir);
        let _ = std::fs::remove_file(cache.stamp("split"));
    }

    // Validate cached rootfs.zip: it MUST contain `.rootfs_manifest.json`.
    // Older versions of this function stored the manifest as a separate file
    // rather than inside the zip, so a stale zip would cause a runtime error
    // (".rootfs_manifest.json not found in embedded rootfs.zip").
    if cache.stamp("split").exists() && cache.rootfs_zip().exists() {
        let manifest_valid = std::fs::File::open(cache.rootfs_zip())
            .ok()
            .and_then(|f| {
                let mut archive = zip::ZipArchive::new(f).ok()?;
                archive.by_name(".rootfs_manifest.json").ok()?;
                Some(())
            })
            .is_some();
        if manifest_valid {
            return;
        }
        eprintln!("  Cached rootfs.zip is stale (missing manifest) — regenerating");
        // Remove stale artifacts so we rebuild below.
        let _ = std::fs::remove_file(cache.rootfs_zip());
        let _ = std::fs::remove_file(cache.stamp("split"));
    }
    eprintln!("  Splitting rootfs into ELF / non-ELF …");

    let files_dir = cache.rootfs_binaries_dir();
    let _ = std::fs::remove_dir_all(&files_dir);
    std::fs::create_dir_all(&files_dir).expect("create rootfs_binaries dir");

    let tar_file = std::fs::File::open(cache.rootfs_tar()).expect("open rootfs.tar");
    let mut archive = tar::Archive::new(tar_file);

    let mut files: HashMap<String, FileEntry> = HashMap::new();
    let mut symlinks: HashMap<String, String> = HashMap::new();

    let zf = std::fs::File::create(&cache.rootfs_zip()).expect("create rootfs.zip");
    let mut zip = ZipWriter::new(zf);

    for entry in archive.entries().expect("tar entries") {
        let mut entry = entry.expect("tar entry");
        let path = entry.path().expect("entry path").into_owned();
        let rel = format!("/{}", path.display());

        // Symlinks
        if entry.header().entry_type().is_symlink() {
            if let Ok(Some(link_target)) = entry.link_name() {
                symlinks.insert(rel, link_target.to_string_lossy().to_string());
            }
            continue;
        }

        // Hard links – skip, handled by symlinks.
        if entry.header().entry_type().is_hard_link() {
            continue;
        }

        // Only regular files.
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let size = entry.size();
        let mut data = Vec::with_capacity(size as usize);
        entry.read_to_end(&mut data).expect("read tar entry");

        let is_elf = data.len() >= 4 && data[..4] == [0x7f, b'E', b'L', b'F'];

        if is_elf {
            let hash = fnv1a_hex(&rel);
            let h16: String = hash.chars().take(16).collect();
            std::fs::write(files_dir.join(&h16), &data).expect("write ELF file");
            set_executable(&files_dir.join(&h16));
            files.insert(
                h16,
                FileEntry {
                    path: rel,
                    mode: 0o755,
                    size,
                },
            );
        } else {
            let zip_path = path.strip_prefix("/").unwrap_or(&path);
            let mode = entry.header().mode().unwrap_or(0o644);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .unix_permissions(mode);
            zip.start_file(zip_path.to_string_lossy().as_ref(), opts)
                .expect("zip start file");
            zip.write_all(&data).expect("zip write file");
        }
    }

    // Write manifest into the zip.
    let manifest = RootfsManifest {
        version: 2,
        tag,
        created: minutes_since_2026().to_string(),
        env: config.env,
        entrypoint: config.entrypoint,
        cmd: config.cmd,
        working_dir: config.working_dir,
        files,
        symlinks,
    };
    let json = serde_json::to_string(&manifest).expect("serialize manifest");
    let opts: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file(".rootfs_manifest.json", opts)
        .expect("zip manifest start");
    zip.write_all(json.as_bytes()).expect("zip manifest write");
    zip.finish().expect("finish rootfs.zip");

    eprintln!(
        "  rootfs.zip created ({} ELF files, {} symlinks)",
        manifest.files.len(),
        manifest.symlinks.len(),
    );

    // Clean up the tar — no longer needed now that we have the zip and binaries.
    let _ = std::fs::remove_file(cache.rootfs_tar());

    std::fs::write(cache.stamp("split"), b"").ok();
}

// ---------------------------------------------------------------------------
// Manifest types (serialized inside rootfs.zip as .rootfs_manifest.json)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct RootfsManifest {
    version: u32,
    tag: String,
    created: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    env: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entrypoint: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cmd: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    working_dir: Option<String>,
    #[serde(default)]
    files: HashMap<String, FileEntry>,
    #[serde(default)]
    symlinks: HashMap<String, String>,
}

#[derive(Serialize)]
struct FileEntry {
    path: String,
    #[serde(default)]
    mode: u32,
    #[serde(default)]
    size: u64,
}

/// Runtime config extracted from the Docker image.
struct ImageConfig {
    env: Vec<String>,
    entrypoint: Option<Vec<String>>,
    cmd: Option<Vec<String>>,
    working_dir: Option<String>,
}

// ---------------------------------------------------------------------------
// Gradle project patching & asset delivery
// ---------------------------------------------------------------------------

/// Write a marker file with the cache path so the Gradle preBuild task
/// can find it when copying ELF files into jniLibs.
fn write_cache_marker(profile: &str, cache: &CacheDir) {
    let app_dir = project_root()
        .join("target")
        .join("dx")
        .join("colmap-openmvs-app")
        .join(profile)
        .join("android")
        .join("app");

    if !app_dir.exists() {
        eprintln!("  NOTE: Android project not yet generated — run `dx bundle` first");
        return;
    }

    let marker = app_dir.join(".rootfs_cache_dir");
    let arch_abi = match std::env::var("TARGET").unwrap_or_default().as_str() {
        t if t.contains("aarch64") => "arm64-v8a",
        t if t.contains("x86_64") => "x86_64",
        _ => return,
    };
    std::fs::write(&marker, format!("{}\n{arch_abi}\n", cache.root.display()))
        .expect("write .rootfs_cache_dir");
    eprintln!("  Wrote .rootfs_cache_dir marker");
}

/// Patch the Gradle project so that:
///   - compileSdk / targetSdk → 36
///   - useLegacyPackaging = true
///   - versionCode = minutes since 2026-01-01
///   - versionName = git describe + hash
///   - AndroidManifest.xml gets extractNativeLibs="true"
///
/// Asset delivery (proot, loader, libtalloc, ELF files → jniLibs) is handled
/// by `copy_assets_to_jnilibs` called separately — no Gradle task needed.
fn patch_gradle_project(profile: &str) {
    let app_dir = project_root()
        .join("target")
        .join("dx")
        .join("colmap-openmvs-app")
        .join(profile)
        .join("android")
        .join("app");

    if !app_dir.join("build.gradle.kts").exists() {
        eprintln!("  NOTE: Gradle project not yet generated (run `dx bundle` first)");
        return;
    }

    // ── app/build.gradle.kts ───────────────────────────────────────────
    let app_gradle = app_dir.join("app").join("build.gradle.kts");
    if app_gradle.exists() {
        let content = std::fs::read_to_string(&app_gradle).unwrap_or_default();
        let mut modified = content.clone();

        // useLegacyPackaging
        if !modified.contains("useLegacyPackaging") {
            modified = modified.replace(
                "android {",
                "android {\n    packagingOptions {\n        jniLibs {\n            useLegacyPackaging = true\n        }\n    }",
            );
        }

        // Bump compileSdk / targetSdk to 36.
        modified = set_kv(&modified, "compileSdk", "36");
        modified = set_kv(&modified, "targetSdk", "36");

        // Time-based versionCode (minutes since 2026-01-01).
        modified = set_kv(&modified, "versionCode", &minutes_since_2026().to_string());

        // versionName
        let desc = git_describe().unwrap_or_else(|| "0.0.0".into());
        let hash = git_short_hash().unwrap_or_else(|| "unknown".into());
        let dirty = if git_is_dirty() { "-dirty" } else { "" };
        let hash = format!("{}{}", hash, dirty);
        modified = set_kv_str(&modified, "versionName", &format!("{desc}+{hash}"));

        if modified != content {
            std::fs::write(&app_gradle, &modified).expect("write build.gradle.kts");
            eprintln!("  Patched app/build.gradle.kts");
        }
    }

    // ── AndroidManifest.xml ─────────────────────────────────────────────
    let manifest = app_dir
        .join("app")
        .join("src")
        .join("main")
        .join("AndroidManifest.xml");
    if manifest.exists() {
        let content = std::fs::read_to_string(&manifest).unwrap_or_default();
        let mut modified = content.clone();

        // android:extractNativeLibs="true"
        if !modified.contains("extractNativeLibs") {
            modified = modified.replacen(
                "<application",
                "<application android:extractNativeLibs=\"true\"",
                1,
            );
        }

        // Ensure the manifest references our theme (for edge-to-edge opt-out).
        // If no android:theme is set on <application>, add one pointing to @style/AppTheme.
        if !modified.contains("android:theme") {
            modified = modified.replacen(
                "<application",
                "<application android:theme=\"@style/AppTheme\"",
                1,
            );
        }

        if modified != content {
            std::fs::write(&manifest, &modified).expect("write AndroidManifest.xml");
            eprintln!("  Patched AndroidManifest.xml (extractNativeLibs + theme ref)");
        }
    }

    // ── Theme XML (res/values/themes.xml) ───────────────────────────────
    // Add windowOptOutEdgeToEdgeEnforcement to opt out of Android 15+
    // edge-to-edge enforcement at the theme level (works on API ≤ 35).
    // On API 36+ this attribute is ignored, so the JNI approach in theme.rs
    // is the primary mechanism.
    let themes_xml = app_dir
        .join("app")
        .join("src")
        .join("main")
        .join("res")
        .join("values")
        .join("themes.xml");
    // Fall back to the legacy styles.xml path
    let styles_xml = app_dir
        .join("app")
        .join("src")
        .join("main")
        .join("res")
        .join("values")
        .join("styles.xml");

    // Helper to patch a theme XML file.
    let patch_theme_file = |path: &std::path::Path| -> bool {
        if !path.exists() {
            return false;
        }
        let content = std::fs::read_to_string(path).unwrap_or_default();
        if content.contains("windowOptOutEdgeToEdgeEnforcement") {
            return false; // already patched
        }
        // Insert into the first <style> block (typically AppTheme).
        if let Some(style_start) = content.find("<style") {
            // Find the opening brace of that style block
            if let Some(brace) = content[style_start..].find('>') {
                let insert_at = style_start + brace + 1;
                let item = "\n        <item name=\"android:windowOptOutEdgeToEdgeEnforcement\">true</item>";
                let modified =
                    format!("{}{}{}", &content[..insert_at], item, &content[insert_at..],);
                if modified != content {
                    std::fs::write(path, &modified).expect("write themes.xml");
                    return true;
                }
            }
        }
        false
    };

    if patch_theme_file(&themes_xml) {
        eprintln!("  Patched themes.xml (windowOptOutEdgeToEdgeEnforcement=true)");
    } else if patch_theme_file(&styles_xml) {
        eprintln!("  Patched styles.xml (windowOptOutEdgeToEdgeEnforcement=true)");
    } else {
        // No existing theme file — create one with the opt-out.
        if let Some(parent) = themes_xml.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let theme_content = r##"<?xml version="1.0" encoding="utf-8"?>
<resources>
    <style name="AppTheme" parent="android:Theme.Material.Light.NoActionBar">
        <item name="android:windowOptOutEdgeToEdgeEnforcement">true</item>
    </style>
</resources>
"##;
        std::fs::write(&themes_xml, theme_content).expect("write themes.xml");
        eprintln!("  Created themes.xml with windowOptOutEdgeToEdgeEnforcement=true");
    }
}

/// Copy the cached rootfs assets (proot, loader, libtalloc, ELF binaries)
/// directly into the jniLibs directory so they end up in the APK.
/// This replaces the old Gradle `copyRootfsToJniLibs` task approach.
fn copy_assets_to_jnilibs(profile: &str, cache: &CacheDir) {
    let arch_abi = match std::env::var("TARGET").unwrap_or_default().as_str() {
        t if t.contains("aarch64") => "arm64-v8a",
        t if t.contains("x86_64") => "x86_64",
        _ => return,
    };

    let app_dir = project_root()
        .join("target")
        .join("dx")
        .join("colmap-openmvs-app")
        .join(profile)
        .join("android")
        .join("app");

    let jni_dir = app_dir
        .join("app")
        .join("src")
        .join("main")
        .join("jniLibs")
        .join(arch_abi);

    if !jni_dir.exists() {
        eprintln!(
            "  NOTE: jniLibs directory not found — creating {}",
            jni_dir.display()
        );
        std::fs::create_dir_all(&jni_dir).expect("create jniLibs dir");
    }

    // ── Clean-up: remove stale ABI jniLibs from OTHER architectures ──────
    // When building sequentially (e.g. arm64 then x86_64) the Gradle
    // packager fails if it finds native libs from multiple ABIs.
    if let Some(jni_parent) = jni_dir.parent() {
        if let Ok(entries) = std::fs::read_dir(jni_parent) {
            for entry in entries {
                if let Ok(entry) = entry {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str != arch_abi && entry.path().is_dir() {
                        let _ = std::fs::remove_dir_all(&entry.path());
                        eprintln!("  Removed stale jniLibs/{name_str}");
                    }
                }
            }
        }
    }

    // Map source cache files → target jniLibs file names
    #[rustfmt::skip]
    let mappings: &[(&str, &str)] = &[
        ("proot",         "libproot.so"),
        ("loader",        "libloader.so"),
        ("libtalloc.so.2", "libtalloc.so"),
    ];

    for (src_name, dst_name) in mappings {
        let src = cache.root.join(src_name);
        let dst = jni_dir.join(dst_name);
        if src.exists() {
            std::fs::copy(&src, &dst).expect(&format!("copy {src_name} → {dst_name}"));
            set_executable(&dst);
            eprintln!("  Copied {src_name} → {dst_name}");
        }
    }

    // Copy ELF files from rootfs_binaries as librootfs-<hash>.so
    let bin_dir = cache.rootfs_binaries_dir();
    if !bin_dir.exists() {
        return;
    }

    // Quick idempotency check: count existing librootfs-*.so files
    // and compare to the number in rootfs_binaries.
    let existing_count = std::fs::read_dir(&jni_dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with("librootfs-"))
                .count()
        })
        .unwrap_or(0);
    let expected_count = std::fs::read_dir(&bin_dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .count()
        })
        .unwrap_or(0);

    if existing_count >= expected_count && existing_count > 0 {
        eprintln!(
            "  ELF binaries already synced ({} files), skipping",
            existing_count
        );
        return;
    }

    for entry in std::fs::read_dir(&bin_dir).expect("read rootfs_binaries dir") {
        let entry = entry.expect("read dir entry");
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        let dst_name = format!("librootfs-{name_str}.so");
        let dst = jni_dir.join(&dst_name);
        std::fs::copy(&entry.path(), &dst).expect(&format!("copy ELF {name_str}"));
    }
    eprintln!("  Copied ELF binaries → jniLibs/");
}

/// If patchelf is available, apply it to the cached proot binary so that
/// it finds libtalloc.so via RPATH=$ORIGIN instead of needing a symlink.
fn patch_proot_binary(cache: &CacheDir) {
    let proot_path = cache.proot();
    if !proot_path.exists() {
        return;
    }

    // Check if patchelf is available by running it with --version.
    if Command::new("patchelf").arg("--version").output().is_err() {
        eprintln!("  NOTE: patchelf not found — skipping proot RPATH patching");
        return;
    }

    let output = Command::new("patchelf")
        .arg("--set-rpath")
        .arg("$ORIGIN")
        .arg(&proot_path)
        .output();
    if let Ok(o) = &output {
        if o.status.success() {
            eprintln!("  Patched proot RPATH");
        }
    }

    let output = Command::new("patchelf")
        .arg("--replace-needed")
        .arg("libtalloc.so.2")
        .arg("libtalloc.so")
        .arg(&proot_path)
        .output();
    if let Ok(o) = &output {
        if o.status.success() {
            eprintln!("  Patched proot NEEDED (libtalloc.so.2 → libtalloc.so)");
        }
    }
}

/// Remove the stale Gradle build cache so that a sequential arm64 + x86_64 build succeeds without error.
fn remove_stale_build_artifacts(profile: &str) {
    let maybe_stale_build_cache = project_root()
        .join("target")
        .join("dx")
        .join("colmap-openmvs-app")
        .join(profile)
        .join("android")
        .join("app")
        .join("build")
        .join("intermediates")
        .join("apk_ide_redirect_file");
    if maybe_stale_build_cache.exists() {
        let _ = std::fs::remove_dir_all(&maybe_stale_build_cache);
        eprintln!("  Removed stale Gradle build cache");
    }
}

/// Replace `key = <integer>` in a Gradle Kotlin DSL line.
fn set_kv(content: &str, key: &str, value: &str) -> String {
    let mut result = String::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(eq) = trimmed.find('=') {
            let k = trimmed[..eq].trim();
            if k == key {
                let indent = &line[..line.len() - line.trim_start().len()];
                result.push_str(&format!("{indent}{key} = {value}\n"));
                continue;
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    if result.ends_with('\n') {
        result.pop();
    }
    result
}

/// Replace `key = "<string>"` in a Gradle Kotlin DSL line.
fn set_kv_str(content: &str, key: &str, value: &str) -> String {
    let mut result = String::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(eq) = trimmed.find('=') {
            let k = trimmed[..eq].trim();
            if k == key {
                let indent = &line[..line.len() - line.trim_start().len()];
                result.push_str(&format!("{indent}{key} = \"{value}\"\n"));
                continue;
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    if result.ends_with('\n') {
        result.pop();
    }
    result
}

fn git_describe() -> Option<String> {
    let out = Command::new("git")
        .args([
            "-C",
            &project_root().to_string_lossy(),
            "describe",
            "--tags",
            "--abbrev=0",
        ])
        .output()
        .ok()?;
    if out.status.success() {
        Some(
            String::from_utf8(out.stdout)
                .unwrap_or_default()
                .trim()
                .to_string(),
        )
    } else {
        None
    }
}

fn git_short_hash() -> Option<String> {
    let out = Command::new("git")
        .args([
            "-C",
            &project_root().to_string_lossy(),
            "rev-parse",
            "--short",
            "HEAD",
        ])
        .output()
        .ok()?;
    if out.status.success() {
        Some(
            String::from_utf8(out.stdout)
                .unwrap_or_default()
                .trim()
                .to_string(),
        )
    } else {
        None
    }
}

fn git_is_dirty() -> bool {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(project_root())
        .output()
        .ok();
    if let Some(out) = out {
        if out.status.success() {
            !out.stdout.is_empty()
        } else {
            false
        }
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn project_root() -> PathBuf {
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    PathBuf::from(dir)
        .parent()
        .expect("backend is child of project root")
        .to_path_buf()
}

fn fetch_url(url: &str) -> String {
    let output = Command::new("curl")
        .args(["-fsSL", url])
        .output()
        .expect("curl not found — is curl installed?");
    assert!(output.status.success(), "curl failed for {url}");
    String::from_utf8(output.stdout).unwrap_or_default()
}

fn fetch_url_bytes(url: &str) -> Vec<u8> {
    let output = Command::new("curl")
        .args(["-fsSL", url])
        .output()
        .expect("curl not found");
    assert!(output.status.success(), "curl download failed for {url}");
    output.stdout
}

fn extract_data_tar_from_ar(deb: &[u8]) -> Vec<u8> {
    assert!(
        deb.len() >= 8 && &deb[..8] == b"!<arch>\n",
        "invalid ar archive"
    );
    let mut off = 8usize;
    while off + 60 <= deb.len() {
        let name = String::from_utf8_lossy(&deb[off..off + 16])
            .trim_end()
            .trim_end_matches('/')
            .to_string();
        let size: usize = String::from_utf8_lossy(&deb[off + 48..off + 58])
            .trim_end()
            .parse()
            .expect("ar member size");
        if name.starts_with("data.tar.") {
            return deb[off + 60..off + 60 + size].to_vec();
        }
        off += 60 + ((size + 1) & !1);
    }
    panic!("data.tar not found in ar archive");
}

fn decompress_xz(data: &[u8]) -> Vec<u8> {
    use std::io::Read;
    let mut decoder = xz2::read::XzDecoder::new(data);
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .expect("XZ decompression failed");
    output
}

/// FNV-1a 64-bit hash, returned as a hex string.
fn fnv1a_hex(input: &str) -> String {
    let hash = fnv1a(input);
    format!("{hash:016x}")
}

fn fnv1a(input: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in input.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn set_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).ok();
    }
}

fn minutes_since_2026() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    // Unix timestamp for 2026-01-01 00:00:00 UTC
    const EPOCH_2026: i64 = 1767225600;
    (now - EPOCH_2026) / 60
}
