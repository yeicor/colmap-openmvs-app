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
// OCI Distribution API client (replaces Docker for image export)
// ---------------------------------------------------------------------------

/// Parse an image reference into (registry, repository, tag).
/// Supports formats:
///   - `registry/repo:tag`
///   - `registry/repo` (default tag: latest)
///   - `repo:tag` (default registry: docker.io)
///   - `repo` (default registry and tag)
fn parse_image_ref(image: &str) -> (String, String, String) {
    let (registry, rest) = if let Some(slash) = image.find('/') {
        let part = &image[..slash];
        // If the part before the first slash contains a dot or colon, it's
        // a registry hostname. Otherwise it's a Docker Hub namespace/user.
        if part.contains('.') || part.contains(':') || part == "localhost" {
            (part.to_string(), &image[slash + 1..])
        } else {
            ("registry-1.docker.io".to_string(), image)
        }
    } else {
        ("registry-1.docker.io".to_string(), image)
    };

    let (repo, tag) = if let Some(colon) = rest.rfind(':') {
        (&rest[..colon], &rest[colon + 1..])
    } else {
        (rest, "latest")
    };

    (registry, repo.to_string(), tag.to_string())
}

/// Obtain an anonymous OCI bearer token from the registry's auth endpoint.
/// Returns empty string if the registry allows anonymous access directly.
fn registry_token(registry: &str, _repo: &str) -> String {
    // First, probe the registry to get the auth challenge.
    let probe_url = format!("https://{}/v2/", registry);
    let output = Command::new("curl")
        .args([
            "-fsSI",
            "--max-time",
            "15",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}\n%header{www-authenticate}",
            &probe_url,
        ])
        .output()
        .expect("curl probe failed");
    let stdout = String::from_utf8(output.stdout).expect("curl output");
    let mut lines = stdout.lines();
    let http_code = lines.next().unwrap_or("").trim();
    let auth_header = lines.next().unwrap_or("").trim().to_string();

    if http_code == "200" || http_code == "" {
        // No auth needed
        return String::new();
    }

    // Parse Bearer challenge: Bearer realm="...",service="...",scope="..."
    if !auth_header.starts_with("Bearer ") {
        eprintln!("  WARNING: unsupported auth challenge: {auth_header}");
        return String::new();
    }
    let params = &auth_header[7..];
    let mut realm = String::new();
    let mut service = String::new();
    let mut scope = String::new();
    for part in params.split(',') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("realm=\"") {
            realm = val.trim_end_matches('"').to_string();
        } else if let Some(val) = part.strip_prefix("service=\"") {
            service = val.trim_end_matches('"').to_string();
        } else if let Some(val) = part.strip_prefix("scope=\"") {
            scope = val.trim_end_matches('"').to_string();
        }
    }

    if realm.is_empty() {
        eprintln!("  WARNING: no realm in auth challenge: {auth_header}");
        return String::new();
    }

    let mut token_url = format!("{realm}?service={service}");
    if !scope.is_empty() {
        token_url.push_str(&format!("&scope={scope}"));
    }

    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "15",
            "-H",
            "Accept: application/json",
            &token_url,
        ])
        .output()
        .expect("curl token request failed");
    assert!(
        output.status.success(),
        "Failed to get registry token from {realm}"
    );

    #[derive(serde::Deserialize)]
    struct TokenResponse {
        #[serde(default)]
        token: String,
        #[serde(default)]
        access_token: String,
    }
    let body = String::from_utf8(output.stdout).expect("token response UTF-8");
    let token_resp: TokenResponse = serde_json::from_str(&body).expect("parse token response JSON");
    if !token_resp.token.is_empty() {
        token_resp.token
    } else {
        token_resp.access_token
    }
}

/// Fetch a path from the registry API, following redirects.
fn registry_fetch(registry: &str, path: &str, accept: Option<&str>, token: &str) -> Vec<u8> {
    let url = format!("https://{registry}{path}");
    let mut args = vec![
        "-fsSL".to_string(),
        "--max-time".to_string(),
        "30".to_string(),
    ];
    if let Some(accept_val) = accept {
        args.push("-H".to_string());
        args.push(format!("Accept: {accept_val}"));
    }
    if !token.is_empty() {
        args.push("-H".to_string());
        args.push(format!("Authorization: Bearer {token}"));
    }
    args.push(url);

    let output = Command::new("curl")
        .args(&args)
        .output()
        .expect("curl registry_fetch failed");
    assert!(
        output.status.success(),
        "registry_fetch failed for {registry}{path}"
    );
    output.stdout
}

/// Get the manifest list (or single manifest) and return the platform-specific
/// manifest digest plus the config digest and layer digests.
fn fetch_image_manifest(
    registry: &str,
    repo: &str,
    tag: &str,
    platform: &str,
    token: &str,
) -> (String, Vec<String>, String, String) {
    let path = format!("/v2/{repo}/manifests/{tag}");

    // Try OCI image index first
    let manifest_data = registry_fetch(
        registry,
        &path,
        Some("application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json"),
        token,
    );

    let manifest_str = String::from_utf8_lossy(&manifest_data);
    let json: serde_json::Value = serde_json::from_str(&manifest_str).expect("parse manifest JSON");

    let media_type = json["mediaType"]
        .as_str()
        .unwrap_or("application/vnd.docker.distribution.manifest.v2+json");

    let target_arch = platform.split('/').next().unwrap_or("amd64");
    let target_os = platform.split('/').nth(1).unwrap_or("linux");

    if media_type.contains("manifest.list")
        || media_type.contains("image.index")
        || json["manifests"].is_array()
    {
        // It's a manifest list — find the matching platform
        let manifests = json["manifests"]
            .as_array()
            .expect("manifest list has manifests array");
        for entry in manifests {
            let arch = entry["platform"]["architecture"].as_str().unwrap_or("");
            let os = entry["platform"]["os"].as_str().unwrap_or("");
            if arch == target_arch && os == target_os {
                let plat_digest = entry["digest"]
                    .as_str()
                    .expect("platform manifest digest")
                    .to_string();
                // Fetch the platform-specific manifest
                let plat_path = format!("/v2/{repo}/manifests/{plat_digest}");
                let plat_data = registry_fetch(
                    registry,
                    &plat_path,
                    Some("application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json"),
                    token,
                );
                return extract_manifest_info(&plat_data, &plat_digest);
            }
        }
        eprintln!("  Platform {platform} not found in manifest list, using first entry");
        let first = &manifests[0];
        let plat_digest = first["digest"]
            .as_str()
            .expect("first platform digest")
            .to_string();
        let plat_path = format!("/v2/{repo}/manifests/{plat_digest}");
        let plat_data = registry_fetch(
            registry,
            &plat_path,
            Some("application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json"),
            token,
        );
        return extract_manifest_info(&plat_data, &plat_digest);
    }

    // Single manifest — extract info directly
    let digest = json["config"]["digest"]
        .as_str()
        .map(|s| {
            // Use the config digest as the image digest
            s.to_string()
        })
        .unwrap_or_else(|| tag.to_string());
    extract_manifest_info(&manifest_data, &digest)
}

/// Extract config digest and layer digests from a platform-specific manifest.
fn extract_manifest_info(
    manifest_data: &[u8],
    manifest_digest: &str,
) -> (String, Vec<String>, String, String) {
    let manifest_str = String::from_utf8_lossy(manifest_data);
    let json: serde_json::Value =
        serde_json::from_str(&manifest_str).expect("parse platform manifest");

    let config_digest = json["config"]["digest"]
        .as_str()
        .expect("config digest")
        .to_string();

    let mut layers = Vec::new();
    if let Some(layer_list) = json["layers"].as_array() {
        for layer in layer_list {
            if let Some(digest) = layer["digest"].as_str() {
                layers.push(digest.to_string());
            }
        }
    }

    (
        manifest_digest.to_string(),
        layers,
        config_digest,
        manifest_str.to_string(),
    )
}

/// Fetch and parse the image config blob.
fn fetch_image_config(registry: &str, repo: &str, config_digest: &str, token: &str) -> ImageConfig {
    let path = format!("/v2/{repo}/blobs/{config_digest}");
    let data = registry_fetch(registry, &path, None, token);

    let json: serde_json::Value = serde_json::from_slice(&data).expect("parse config JSON");
    let cfg = &json["config"];

    let env: Vec<String> = cfg["Env"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let entrypoint: Option<Vec<String>> = cfg["Entrypoint"].as_array().map(|a| {
        a.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    });
    let cmd: Option<Vec<String>> = cfg["Cmd"].as_array().map(|a| {
        a.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    });
    let working_dir = cfg["WorkingDir"].as_str().unwrap_or("").to_string();

    ImageConfig {
        env,
        entrypoint,
        cmd,
        working_dir: if working_dir.is_empty() {
            None
        } else {
            Some(working_dir)
        },
    }
}

/// Download and extract a single image layer (gzip-compressed tar).
fn download_and_extract_layer(
    registry: &str,
    repo: &str,
    digest: &str,
    token: &str,
    dest_tar: &mut tar::Builder<std::fs::File>,
) {
    let path = format!("/v2/{repo}/blobs/{digest}");
    let data = registry_fetch(registry, &path, None, token);
    eprintln!("  Layer {}: {} bytes", &digest[..20], data.len());

    // Docker layers are gzip-compressed tar archives.
    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(&data[..]);
    let mut tar_bytes = Vec::new();
    decoder.read_to_end(&mut tar_bytes).expect("gunzip layer");

    let mut archive = tar::Archive::new(std::io::Cursor::new(&tar_bytes));
    for entry in archive.entries().expect("tar entries") {
        let mut entry = entry.expect("tar entry");
        // Append each entry into the destination tar
        let header = entry.header().clone();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("read entry");

        dest_tar
            .append(&header, std::io::Cursor::new(&data))
            .expect("append to rootfs.tar");
    }
}

/// Download all layers for an image and assemble them into a single tar,
/// without needing Docker. Only supports gzip-compressed layers.
fn export_image_via_registry(image: &str, platform: &str, dest: &Path) -> String {
    let (registry, repo, tag) = parse_image_ref(image);
    eprintln!("  Fetching image {image} ({platform}) via OCI registry API ...");
    eprintln!("    Registry: {registry}, Repo: {repo}, Tag: {tag}");

    let token = registry_token(&registry, &repo);
    let (_manifest_digest, layers, config_digest, _manifest_str) =
        fetch_image_manifest(&registry, &repo, &tag, platform, &token);

    eprintln!("    Config: {config_digest}");
    eprintln!("    Layers: {}", layers.len());

    let tar_file = std::fs::File::create(dest).expect("create rootfs.tar");
    let mut tar_builder = tar::Builder::new(tar_file);

    for (i, layer_digest) in layers.iter().enumerate() {
        eprint!("    [{}/{}] Downloading layer ... ", i + 1, layers.len());
        download_and_extract_layer(&registry, &repo, layer_digest, &token, &mut tar_builder);
    }

    let tar_file = tar_builder.into_inner().expect("finish tar");
    tar_file.sync_all().expect("sync rootfs.tar");

    eprintln!("    Done — rootfs.tar created");

    config_digest
}

fn image_digest(image: &str, platform: &str) -> String {
    let (registry, repo, tag) = parse_image_ref(image);
    let token = registry_token(&registry, &repo);
    let (manifest_digest, _layers, _config_digest, _manifest_str) =
        fetch_image_manifest(&registry, &repo, &tag, platform, &token);
    manifest_digest
}

fn export_rootfs(cache: &CacheDir, image: &str, platform: &str) {
    if cache.stamp("rootfs_export").exists() {
        return;
    }
    eprintln!("  Exporting rootfs from {image} …");
    let _config_digest = export_image_via_registry(image, platform, &cache.rootfs_tar());
    std::fs::write(cache.stamp("rootfs_export"), b"").expect("write .stamp_rootfs_export");
}

fn image_config(image: &str, platform: &str) -> ImageConfig {
    let (registry, repo, tag) = parse_image_ref(image);
    let token = registry_token(&registry, &repo);
    let (_manifest_digest, _layers, config_digest, _manifest_str) =
        fetch_image_manifest(&registry, &repo, &tag, platform, &token);
    fetch_image_config(&registry, &repo, &config_digest, &token)
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

fn build_rootfs_artifacts(cache: &CacheDir, tag: String, config: ImageConfig) {
    // Stale cache cleanup: old builds used `rootfs_files` instead of `rootfs_binaries`.
    let old_dir = cache.root.join("rootfs_files");
    if old_dir.exists() {
        std::fs::remove_dir_all(&old_dir).expect("remove stale rootfs_files");
        let split_stamp = cache.stamp("split");
        if split_stamp.exists() {
            std::fs::remove_file(&split_stamp).expect("remove stale .stamp_split");
        }
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
        if cache.rootfs_zip().exists() {
            std::fs::remove_file(cache.rootfs_zip()).expect("remove stale rootfs.zip");
        }
        let split_stamp = cache.stamp("split");
        if split_stamp.exists() {
            std::fs::remove_file(&split_stamp).expect("remove stale .stamp_split");
        }
    }
    eprintln!("  Splitting rootfs into ELF / non-ELF …");

    let files_dir = cache.rootfs_binaries_dir();
    if files_dir.exists() {
        std::fs::remove_dir_all(&files_dir).expect("remove old rootfs_binaries");
    }
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
            let link_target = entry
                .link_name()
                .expect("read symlink target")
                .unwrap_or_default();
            if !link_target.as_os_str().is_empty() {
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
            let mode = entry
                .header()
                .mode()
                .expect("tar entry should have mode bits");
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
    if cache.rootfs_tar().exists() {
        std::fs::remove_file(cache.rootfs_tar()).expect("remove rootfs.tar");
    }

    std::fs::write(cache.stamp("split"), b"").expect("write .stamp_split");
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

/// Copy the cached rootfs assets (proot, loader, libtalloc, ELF binaries)
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
        ("proot",         "libproot.so"),
        ("loader",        "libloader.so"),
        ("libtalloc.so.2", "libtalloc.so"),
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
/// it finds libtalloc.so via RPATH=$ORIGIN instead of needing a symlink.
fn patch_proot_binary(cache: &CacheDir) {
    let proot_path = cache.proot();
    if !proot_path.exists() {
        return;
    }

    // Verify patchelf is available.
    let check = Command::new("patchelf")
        .arg("--version")
        .output()
        .expect("failed to execute patchelf");
    assert!(
        check.status.success(),
        "patchelf --version failed:\n{}",
        String::from_utf8_lossy(&check.stderr)
    );

    // Set RPATH to $ORIGIN so proot finds libtalloc in its own directory.
    let rpath = Command::new("patchelf")
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
    let needed = Command::new("patchelf")
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
    String::from_utf8(output.stdout).expect("curl output is valid UTF-8")
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
