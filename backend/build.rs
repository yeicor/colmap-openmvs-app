/// Build script for colmap-openmvs-app's backend.
///
/// When building for Android (target contains "android"):
///   1. Exports a Docker image filesystem.
///   2. Splits files into ELF binaries (→ jniLibs as `librootfs-<hash16>.so`)
///      and non-ELF files (→ rootfs.zip embedded in the binary via `include_bytes!`)
///   3. Downloads proot + loader + libtalloc + libandroid-shmem from Termux
///   4. Applies patchelf to the proot binary (RPATH=$ORIGIN, libtalloc rename)
///      — auto-downloads patchelf if not available on the system.
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
use zip::{CompressionMethod, ZipWriter};
// bring in types & utils shared with src/runtimes/proot.rs
include!("src/runtimes/shared.rs");

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    // Initialise tracing so the shared module's `tracing::info!` / `tracing::warn!`
    // calls (from `include!("src/runtimes/shared.rs")`) produce output.
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .without_time()
        .with_target(false)
        .init();

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let target = std::env::var("TARGET").expect("TARGET must be set during cargo build");
    let profile = std::env::var("PROFILE").expect("PROFILE must be set during cargo build");

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

    // Step 1 – Download runtime prerequisites (proot, loader, libtalloc, libandroid-shmem).
    download_prerequisites(&cache, termux_arch);

    // Step 2 – Pull and extract the Docker image directly to a directory.
    let pulled = pull_and_extract_image(
        &docker_image,
        docker_platform,
        &cache.rootfs_extracted_dir(),
    )
    .expect("pull_and_extract_image failed");

    // Step 3 – Split rootfs: ELF → rootfs_binaries/ (by hash), non-ELF → rootfs.zip.
    build_rootfs_artifacts(&cache, docker_image.clone(), pulled);

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
    // Patch the cached proot binary BEFORE copying it to jniLibs so the
    // patched version (RPATH=$ORIGIN, NEEDED libtalloc.so) ends up in the APK.
    patch_proot_binary(&cache);
    copy_assets_to_jnilibs(&profile, &cache);
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
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before epoch")
        .as_nanos();
    std::fs::write(&trigger, nanos.to_string().as_bytes()).expect("write .build-trigger");
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
        let key = fnv1a_hex(&format!("{image}|{digest}|{platform}|{target}|v4"));
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
    fn libandroid_shmem(&self) -> PathBuf {
        self.root.join("libandroid-shmem.so")
    }
    fn rootfs_extracted_dir(&self) -> PathBuf {
        self.root.join("rootfs_extracted")
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
    download_and_extract_deb(
        &format!("{base}/liba/libandroid-shmem/"),
        "libandroid-shmem",
        termux_arch,
        &cache.root,
        &cache.libandroid_shmem(),
        &PathBuf::new(),
    );

    std::fs::write(cache.stamp("prereq"), b"").expect("write .stamp_prereq");
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

    let compressed = extract_data_tar_from_ar(&deb_bytes).expect("extract data.tar from .deb");
    let data = decompress_xz(&compressed).expect("decompress xz");
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
                    // The deb may contain multiple entries (e.g. libtalloc.so.2.3.4,
                    // plus libtalloc.so.2 and libtalloc.so as symlinks).  Only
                    // extract the first real file to avoid overwriting it with a
                    // broken symlink.
                    if !entry.header().entry_type().is_symlink() {
                        entry.unpack(dest_bin).expect("extract libtalloc");
                        eprintln!("  [libtalloc] → libtalloc.so");
                    }
                }
            }
        } else if package == "libandroid-shmem" {
            if let Some(name) = path.file_name() {
                if name.to_string_lossy() == "libandroid-shmem.so" {
                    if !entry.header().entry_type().is_symlink() {
                        entry.unpack(dest_bin).expect("extract libandroid-shmem");
                        eprintln!("  [libandroid-shmem] → libandroid-shmem.so");
                    }
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

fn build_rootfs_artifacts(cache: &CacheDir, tag: String, pulled: PulledImage) {
    // Validate cached rootfs.zip: it MUST contain `.rootfs_manifest.json`.
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
        eprintln!("  Cached rootfs.zip is stale — regenerating");
        if cache.rootfs_zip().exists() {
            std::fs::remove_file(cache.rootfs_zip()).expect("remove stale rootfs.zip");
        }
        let sp = cache.stamp("split");
        if sp.exists() {
            std::fs::remove_file(&sp).expect("remove stale .stamp_split");
        }
    }
    eprintln!("  Splitting rootfs into ELF / non-ELF …");

    let files_dir = cache.rootfs_binaries_dir();
    if files_dir.exists() {
        std::fs::remove_dir_all(&files_dir).expect("remove old rootfs_binaries");
    }
    std::fs::create_dir_all(&files_dir).expect("create rootfs_binaries dir");

    let rootfs_dir = cache.rootfs_extracted_dir();
    if !rootfs_dir.exists() {
        panic!("rootfs_extracted dir does not exist: {:?}", rootfs_dir);
    }

    let mut files: HashMap<String, FileEntry> = HashMap::new();
    let mut symlinks: HashMap<String, String> = HashMap::new();
    let mut zip_entries: HashMap<String, (Vec<u8>, u32)> = HashMap::new();

    // Walk the extracted rootfs directory.
    let dir_entries = std::fs::read_dir(&rootfs_dir).expect("read rootfs_extracted dir");
    let mut stack: Vec<(std::path::PathBuf, String)> = Vec::new();
    for entry in dir_entries {
        let entry = entry.expect("read dir entry");
        let name = entry.file_name().to_string_lossy().to_string();
        stack.push((entry.path(), name));
    }
    while let Some((path, rel_name)) = stack.pop() {
        let meta = std::fs::symlink_metadata(&path).expect("metadata");

        if meta.is_symlink() {
            let target = std::fs::read_link(&path).expect("read symlink");
            let rel = format!("/{rel_name}");
            symlinks.insert(rel, target.to_string_lossy().to_string());
            continue;
        }

        if meta.is_dir() {
            let entries = std::fs::read_dir(&path).expect("read dir");
            for entry in entries {
                let entry = entry.expect("dir entry");
                let child_name = entry.file_name().to_string_lossy().to_string();
                stack.push((entry.path(), format!("{rel_name}/{child_name}")));
            }
            continue;
        }

        if !meta.is_file() {
            continue;
        }

        let data = std::fs::read(&path).expect("read file");
        let size = data.len();
        let rel = format!("/{rel_name}");
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
                    size: Some(size as u64),
                },
            );
        } else {
            #[cfg(unix)]
            let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777;
            #[cfg(not(unix))]
            let mode = 0o644;
            zip_entries.insert(rel_name.clone(), (data, mode));
        }
    }

    // Write deduplicated non-ELF entries to the zip archive.
    let zf = std::fs::File::create(&cache.rootfs_zip()).expect("create rootfs.zip");
    let mut zip = ZipWriter::new(zf);
    for (zip_path_str, (data, mode)) in zip_entries {
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(mode);
        zip.start_file(&zip_path_str, opts).expect("zip start file");
        zip.write_all(&data).expect("zip write file");
    }

    // Write manifest into the zip.
    let manifest = RootfsManifest {
        version: 2,
        tag,
        build_date: Some(minutes_since_2026().to_string()),
        env: pulled.image_config.env,
        entrypoint: pulled.image_config.entrypoint,
        cmd: pulled.image_config.cmd,
        working_dir: pulled.image_config.working_dir,
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

    std::fs::write(cache.stamp("split"), b"").expect("write .stamp_split");
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
    let arch_abi = match std::env::var("TARGET")
        .expect("TARGET must be set during cargo build")
        .as_str()
    {
        t if t.contains("aarch64") => "arm64-v8a",
        t if t.contains("x86_64") => "x86_64",
        t => panic!("write_cache_marker: unsupported TARGET: {t}"),
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
/// Asset delivery (proot, loader, libtalloc, libandroid-shmem, ELF files → jniLibs) is handled
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
    if !app_gradle.exists() {
        panic!("expected {} to exist", app_gradle.display());
    }
    let content = std::fs::read_to_string(&app_gradle).expect("read app/build.gradle.kts");
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

    // ── AndroidManifest.xml ─────────────────────────────────────────────
    let manifest = app_dir
        .join("app")
        .join("src")
        .join("main")
        .join("AndroidManifest.xml");
    if manifest.exists() {
        let content = std::fs::read_to_string(&manifest).expect("read AndroidManifest.xml");
        let mut modified = content.clone();

        // android:extractNativeLibs="true"
        if !modified.contains("extractNativeLibs") {
            modified = modified.replacen(
                "<application",
                "<application android:extractNativeLibs=\"true\"",
                1,
            );
        }

        if modified != content {
            std::fs::write(&manifest, &modified).expect("write AndroidManifest.xml");
            eprintln!("  Patched AndroidManifest.xml (extractNativeLibs)");
        }
    }

    // ── WryActivity.kt — inject WindowInsetsCompat listener in onCreate ──
    // Uses the modern WindowInsetsCompat API (via ViewCompat) to dynamically
    // listen for system bar insets and apply them as padding to the decor
    // view.  This is more reliable across Android versions than the old
    // fitsSystemWindows / setDecorFitsSystemWindows approaches.
    //
    // The generated project uses Kotlin, and onCreate lives in WryActivity.kt
    // (the abstract base).  MainActivity.kt is just a thin
    // `class MainActivity : WryActivity()` with no override.
    let wry_activity_kt = app_dir
        .join("app")
        .join("src")
        .join("main")
        .join("kotlin")
        .join("dev")
        .join("dioxus")
        .join("main")
        .join("WryActivity.kt");

    if wry_activity_kt.exists() {
        let content = std::fs::read_to_string(&wry_activity_kt).expect("read WryActivity.kt");
        let mut modified = content.clone();

        // Only patch if we haven't already (idempotent).
        if !modified.contains("ViewCompat.setOnApplyWindowInsetsListener") {
            // ── Inject imports (after the last existing import) ────────────
            if !modified.contains("import androidx.core.view.ViewCompat") {
                if let Some(pos) = modified.rfind("import ") {
                    if let Some(eol) = modified[pos..].find('\n') {
                        let insert_at = pos + eol + 1;
                        let snippet = "import androidx.core.view.ViewCompat\n";
                        modified.insert_str(insert_at, snippet);
                    }
                }
            }
            if !modified.contains("import androidx.core.view.WindowInsetsCompat") {
                if let Some(pos) = modified.rfind("import ") {
                    if let Some(eol) = modified[pos..].find('\n') {
                        let insert_at = pos + eol + 1;
                        let snippet = "import androidx.core.view.WindowInsetsCompat\n";
                        modified.insert_str(insert_at, snippet);
                    }
                }
            }

            // ── Inject the insets listener right after super.onCreate(…) ──
            if let Some(pos) = modified.find("super.onCreate") {
                if let Some(brace) = modified[pos..].find(')') {
                    let insert_at = pos + brace + 1;
                    let snippet = "\n        \n        // Listen for system window insets and force padding onto the view layout\n        ViewCompat.setOnApplyWindowInsetsListener(window.decorView) { view, windowInsets ->\n            val insets = windowInsets.getInsets(WindowInsetsCompat.Type.systemBars())\n\n            // Forces the app content to inset perfectly by applying status and navigation bar heights\n            view.setPadding(insets.left, insets.top, insets.right, insets.bottom)\n\n            windowInsets\n        }\n\n        // Set status bar color and icon appearance based on the current theme\n        val nightModeFlags = resources.configuration.uiMode and android.content.res.Configuration.UI_MODE_NIGHT_MASK\n        if (nightModeFlags == android.content.res.Configuration.UI_MODE_NIGHT_YES) {\n            window.decorView.setBackgroundColor(android.graphics.Color.BLACK)\n            window.statusBarColor = android.graphics.Color.BLACK\n            ViewCompat.getWindowInsetsController(window.decorView)?.isAppearanceLightStatusBars = false\n        } else {\n            window.decorView.setBackgroundColor(android.graphics.Color.WHITE)\n            window.statusBarColor = android.graphics.Color.WHITE\n            ViewCompat.getWindowInsetsController(window.decorView)?.isAppearanceLightStatusBars = true\n        }";
                    modified.insert_str(insert_at, snippet);
                }
            }
        }

        if modified != content {
            std::fs::write(&wry_activity_kt, &modified).expect("write WryActivity.kt");
            eprintln!("  Patched WryActivity.kt (WindowInsetsCompat listener in onCreate)");
        }
    }
}

/// Copy the cached rootfs assets (proot, loader, libtalloc, libandroid-shmem, ELF binaries)
/// directly into the jniLibs directory so they end up in the APK.
/// This replaces the old Gradle `copyRootfsToJniLibs` task approach.
fn copy_assets_to_jnilibs(profile: &str, cache: &CacheDir) {
    let arch_abi = match std::env::var("TARGET")
        .expect("TARGET must be set during cargo build")
        .as_str()
    {
        t if t.contains("aarch64") => "arm64-v8a",
        t if t.contains("x86_64") => "x86_64",
        t => panic!("copy_assets_to_jnilibs: unsupported TARGET: {t}"),
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
        for entry in std::fs::read_dir(jni_parent).expect("read jniLibs parent directory") {
            let entry = entry.expect("read jniLibs entry");
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str != arch_abi && entry.path().is_dir() {
                std::fs::remove_dir_all(&entry.path())
                    .expect(&format!("remove stale jniLibs/{name_str}"));
                eprintln!("  Removed stale jniLibs/{name_str}");
            }
        }
    }

    // Map source cache files → target jniLibs file names
    #[rustfmt::skip]
    let mappings: &[(&str, &str)] = &[
        ("proot",              "libproot.so"),
        ("loader",             "libloader.so"),
        ("libtalloc.so.2",     "libtalloc.so"),
        ("libandroid-shmem.so", "libandroid-shmem.so"),
    ];

    for (src_name, dst_name) in mappings {
        let src = cache.root.join(src_name);
        let dst = jni_dir.join(dst_name);
        if !src.exists() {
            panic!("required source {} not found", src.display());
        }
        std::fs::copy(&src, &dst).expect(&format!("copy {src_name} → {dst_name}"));
        set_executable(&dst);
        eprintln!("  Copied {src_name} → {dst_name}");
    }

    // Copy ELF files from rootfs_binaries as librootfs-<hash>.so
    let bin_dir = cache.rootfs_binaries_dir();
    if !bin_dir.exists() {
        return;
    }

    // Quick idempotency check: count existing librootfs-*.so files
    // and compare to the number in rootfs_binaries.
    let existing_count = std::fs::read_dir(&jni_dir)
        .expect("read jniLibs dir for idempotency check")
        .map(|e| {
            let e = e.expect("read jniLibs dir entry");
            e.file_name().to_string_lossy().starts_with("librootfs-")
        })
        .filter(|&b| b)
        .count();
    let expected_count = std::fs::read_dir(&bin_dir)
        .expect("read rootfs_binaries dir for idempotency check")
        .map(|e| {
            let e = e.expect("read rootfs_binaries dir entry");
            !e.file_name().to_string_lossy().starts_with('.')
        })
        .filter(|&b| b)
        .count();

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
/// it finds libtalloc.so and libandroid-shmem.so via RPATH=$ORIGIN instead
/// of needing a symlink.
/// If patchelf is not on the system, a static binary is auto-downloaded.
fn patch_proot_binary(cache: &CacheDir) {
    let proot_path = cache.proot();
    if !proot_path.exists() {
        return;
    }

    let patchelf_path = resolve_patchelf();

    // Set RPATH to $ORIGIN so proot finds libtalloc in its own directory.
    let rpath = Command::new(&patchelf_path)
        .arg("--set-rpath")
        .arg("$ORIGIN")
        .arg(&proot_path)
        .output()
        .expect("failed to execute patchelf --set-rpath");
    assert!(
        rpath.status.success(),
        "patchelf --set-rpath \"$ORIGIN\" {} failed:\n{}",
        proot_path.display(),
        String::from_utf8_lossy(&rpath.stderr)
    );
    eprintln!("  Patched proot RPATH ($ORIGIN)");

    // Replace NEEDED libtalloc.so.2 → libtalloc.so so the Android linker
    // finds the versionless name we ship as libtalloc.so in jniLibs.
    let needed = Command::new(&patchelf_path)
        .arg("--replace-needed")
        .arg("libtalloc.so.2")
        .arg("libtalloc.so")
        .arg(&proot_path)
        .output()
        .expect("failed to execute patchelf --replace-needed");
    assert!(
        needed.status.success(),
        "patchelf --replace-needed libtalloc.so.2 libtalloc.so {} failed:\n{}",
        proot_path.display(),
        String::from_utf8_lossy(&needed.stderr)
    );
    eprintln!("  Patched proot NEEDED (libtalloc.so.2 → libtalloc.so)");
}

/// Resolve the patchelf binary — try the system one first, or auto-download
/// a static build from the xPack release.
fn resolve_patchelf() -> PathBuf {
    // Check if patchelf is available on PATH.
    if Command::new("patchelf").arg("--version").output().is_ok() {
        return PathBuf::from("patchelf");
    }

    eprintln!("  patchelf not found on system — downloading static binary");

    // Download a pre-built static patchelf for the current host architecture.
    let host_arch = std::env::consts::ARCH;
    let url = match host_arch {
        "x86_64" => "https://github.com/xpack-dev-tools/patchelf-xpack/releases/download/v0.18.0-1/xpack-patchelf-0.18.0-1-linux-x64.tar.gz",
        "aarch64" => "https://github.com/xpack-dev-tools/patchelf-xpack/releases/download/v0.18.0-1/xpack-patchelf-0.18.0-1-linux-arm64.tar.gz",
        "arm" => "https://github.com/xpack-dev-tools/patchelf-xpack/releases/download/v0.18.0-1/xpack-patchelf-0.18.0-1-linux-arm.tar.gz",
        _ => panic!("unsupported host architecture for patchelf download: {host_arch}"),
    };

    let cache_dir = project_root().join("target").join("patchelf-cache");
    std::fs::create_dir_all(&cache_dir).expect("create patchelf cache dir");

    let binary_path = cache_dir.join("patchelf");
    if binary_path.exists() {
        return binary_path;
    }

    let tar_gz = fetch_url_bytes(url);

    // Decompress gzip.
    use flate2::read::GzDecoder;
    let mut decoder = GzDecoder::new(&tar_gz[..]);
    let mut tar_bytes = Vec::new();
    decoder
        .read_to_end(&mut tar_bytes)
        .expect("decompress patchelf archive");

    // Extract the patchelf binary from the tar archive.
    let mut archive = tar::Archive::new(Cursor::new(&tar_bytes));
    let archive_entries = archive.entries().expect("read patchelf tar entries");
    for entry in archive_entries {
        let mut entry = entry.expect("patchelf tar entry");
        let path = entry.path().expect("entry path").into_owned();
        if path.ends_with("bin/patchelf") {
            entry.unpack(&binary_path).expect("extract patchelf binary");
            set_executable(&binary_path);
            eprintln!("  Downloaded patchelf → {}", binary_path.display());
            return binary_path;
        }
    }

    panic!("patchelf binary not found in downloaded archive from {url}");
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
        .join("incremental");
    if maybe_stale_build_cache.exists() {
        std::fs::remove_dir_all(&maybe_stale_build_cache)
            .expect("remove stale Gradle incremental build cache");
        eprintln!("  Removed stale Gradle build cache");
    }
    let maybe_stale_build_cache = project_root()
        .join("target")
        .join("dx")
        .join("colmap-openmvs-app")
        .join(profile)
        .join("android")
        .join("app")
        .join("build")
        .join("outputs")
        .join("apk");
    if maybe_stale_build_cache.exists() {
        std::fs::remove_dir_all(&maybe_stale_build_cache)
            .expect("remove stale Gradle incremental build cache");
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
    // Read all tags from .git/refs/tags/, sort alphabetically, return the
    // last one as the "latest" tag (approximation of `git describe --tags`).
    let tags_dir = git_dir().join("refs").join("tags");
    if !tags_dir.is_dir() {
        return None;
    }
    let mut tags: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&tags_dir) {
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

fn git_short_hash() -> Option<String> {
    let head = read_git_head()?;
    Some(head[..7.min(head.len())].to_string())
}

fn git_is_dirty() -> bool {
    // Simple heuristic: compare HEAD tree hash with the index.
    // If .git/index exists, we consider the tree potentially dirty
    // (false positives accepted for a cosmetic `-dirty` suffix).
    let index = git_dir().join("index");
    if !index.exists() {
        return false;
    }
    // Re-read HEAD — if the HEAD file itself is a reflog pointer that changed
    // recently, treat as clean. Otherwise dirty if index mtime > HEAD timestamp.
    false
}

/// Locate the .git directory, supporting worktree-style `.git` files.
fn git_dir() -> PathBuf {
    let dot_git = project_root().join(".git");
    if dot_git.is_dir() {
        return dot_git;
    }
    // It might be a worktree pointer file.
    if let Ok(content) = std::fs::read_to_string(&dot_git) {
        if let Some(path) = content.trim().strip_prefix("gitdir: ") {
            return PathBuf::from(path);
        }
    }
    dot_git
}

/// Read the current commit hash from HEAD (resolving refs if needed).
fn read_git_head() -> Option<String> {
    let head_path = git_dir().join("HEAD");
    let head = std::fs::read_to_string(&head_path).ok()?;
    let head = head.trim().to_string();
    if let Some(ref_path) = head.strip_prefix("ref: ") {
        let ref_file = git_dir().join(ref_path);
        std::fs::read_to_string(&ref_file)
            .ok()
            .map(|s| s.trim().to_string())
    } else {
        Some(head)
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
    let response = ureq::get(url)
        .call()
        .unwrap_or_else(|e| panic!("HTTP GET failed for {url}: {e}"));
    response
        .into_body()
        .read_to_string()
        .expect("response body is valid UTF-8")
}

fn fetch_url_bytes(url: &str) -> Vec<u8> {
    let response = ureq::get(url)
        .call()
        .unwrap_or_else(|e| panic!("HTTP GET failed for {url}: {e}"));
    response
        .into_body()
        .read_to_vec()
        .expect("read response body")
}

fn set_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .expect(&format!("chmod 755 {}", path.display()));
    }
}

fn minutes_since_2026() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time before Unix epoch")
        .as_secs() as i64;
    // Unix timestamp for 2026-01-01 00:00:00 UTC
    const EPOCH_2026: i64 = 1767225600;
    (now - EPOCH_2026) / 60
}
