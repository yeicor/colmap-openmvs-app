// Shared types and utilities used by both build.rs and the PRoot runtime.
//
// This file is compiled in two ways:
// - As a regular module (`mod shared`) in proot.rs.
// - Via `include!` in build.rs, where its contents are pasted directly
//   into the build script's root scope.
//
// To support both modes, this file avoids `use` statements that would
// conflict when included into build.rs (which already imports some of the
// same names).  All paths are fully qualified instead.

// ---------------------------------------------------------------------------
// Manifest types (serialised inside rootfs.zip as .rootfs_manifest.json)
// ---------------------------------------------------------------------------

/// Manifest produced at build time describing the embedded rootfs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RootfsManifest {
    #[serde(default)]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) tag: String,
    /// Minutes since 2026-01-01 UTC when the manifest was created.
    #[serde(default)]
    pub(crate) build_date: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) env: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) entrypoint: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cmd: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) working_dir: Option<String>,
    #[serde(default)]
    pub(crate) files: std::collections::HashMap<String, FileEntry>,
    #[serde(default)]
    pub(crate) symlinks: std::collections::HashMap<String, String>,
}

/// A single file entry inside a [`RootfsManifest`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct FileEntry {
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) mode: u32,
    #[serde(default)]
    pub(crate) size: Option<u64>,
}

// ---------------------------------------------------------------------------
// Image pulling result types
// ---------------------------------------------------------------------------

/// Metadata extracted from an OCI image config blob.
#[derive(Debug, Clone, Default)]
pub(crate) struct ImageConfig {
    pub(crate) env: Vec<String>,
    pub(crate) entrypoint: Option<Vec<String>>,
    pub(crate) cmd: Option<Vec<String>>,
    pub(crate) working_dir: Option<String>,
}

/// Full result of pulling an OCI image from a registry.
#[derive(Debug, Clone)]
pub(crate) struct PulledImage {
    pub(crate) image_config: ImageConfig,
    /// Image build date / created timestamp from config.
    #[allow(dead_code)] // because currently only used by backend but not on build.rs
    pub(crate) created: Option<String>,
}

// ---------------------------------------------------------------------------
// FNV-1a hashing
// ---------------------------------------------------------------------------

pub(crate) fn fnv1a_hex(input: &str) -> String {
    let hash = fnv1a(input);
    format!("{hash:016x}")
}

pub(crate) fn fnv1a(input: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in input.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ---------------------------------------------------------------------------
// .deb archive helpers
// ---------------------------------------------------------------------------

pub(crate) fn extract_data_tar_from_ar(deb: &[u8]) -> Result<Vec<u8>, String> {
    if deb.len() < 8 || &deb[..8] != b"!<arch>\n" {
        return Err("invalid ar archive: bad magic signature".to_string());
    }
    let mut off = 8usize;
    while off + 60 <= deb.len() {
        let name = String::from_utf8_lossy(&deb[off..off + 16])
            .trim_end()
            .trim_end_matches('/')
            .to_string();
        let size: usize = String::from_utf8_lossy(&deb[off + 48..off + 58])
            .trim_end()
            .parse()
            .map_err(|_| "invalid ar member size".to_string())?;
        if name.starts_with("data.tar.") {
            return Ok(deb[off + 60..off + 60 + size].to_vec());
        }
        off += 60 + ((size + 1) & !1);
    }
    Err("data.tar.* member not found in ar archive".to_string())
}

pub(crate) fn decompress_xz(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = xz2::read::XzDecoder::new(data);
    let mut output = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut output)
        .map_err(|e| format!("XZ decompression failed: {e}"))?;
    Ok(output)
}

// ---------------------------------------------------------------------------
// OCI image reference parser
// ---------------------------------------------------------------------------

/// Parse an image reference into (registry, repository, tag).
/// Supports formats:
///   - registry/repo:tag
///   - registry/repo (default tag: latest)
///   - repo:tag (default registry: docker.io)
///   - repo (default registry and tag)
pub(crate) fn parse_image_ref(image: &str) -> (String, String, String) {
    let (registry, rest) = if let Some(slash) = image.find('/') {
        let part = &image[..slash];
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

// ---------------------------------------------------------------------------
// OCI Registry HTTP helpers (ureq-based, sync)
// ---------------------------------------------------------------------------

fn registry_token(registry: &str, _repo: &str) -> String {
    let probe_url = format!("https://{}/v2/", registry);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(15))
        .build();

    let response = match agent.head(&probe_url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(401, r)) => r,
        Err(ureq::Error::Status(code, _)) => {
            tracing::warn!("Registry probe returned {code}");
            return String::new();
        }
        Err(e) => {
            tracing::warn!("Registry probe failed: {e}");
            return String::new();
        }
    };

    let auth_header = response
        .header("www-authenticate")
        .unwrap_or("")
        .to_string();

    if auth_header.is_empty() {
        // No auth required.
        return String::new();
    }

    if !auth_header.starts_with("Bearer ") {
        tracing::warn!("Unsupported auth challenge: {auth_header}");
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
        tracing::warn!("No realm in auth challenge: {auth_header}");
        return String::new();
    }

    let mut token_url = format!("{realm}?service={service}");
    if !scope.is_empty() {
        token_url.push_str(&format!("&scope={scope}"));
    }

    let body = agent
        .get(&token_url)
        .set("Accept", "application/json")
        .call()
        .unwrap_or_else(|e| panic!("Failed to get registry token from {realm}: {e}"))
        .into_string()
        .expect("token response UTF-8");

    #[derive(serde::Deserialize)]
    struct TokenResponse {
        #[serde(default)]
        token: String,
        #[serde(default)]
        access_token: String,
    }
    let token_resp: TokenResponse = serde_json::from_str(&body).expect("parse token response JSON");
    if !token_resp.token.is_empty() {
        token_resp.token
    } else {
        token_resp.access_token
    }
}

fn registry_fetch(registry: &str, path: &str, accept: Option<&str>, token: &str) -> Vec<u8> {
    let url = format!("https://{registry}{path}");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(30))
        .build();

    let mut req = agent.get(&url);
    if let Some(accept_val) = accept {
        req = req.set("Accept", accept_val);
    }
    if !token.is_empty() {
        req = req.set("Authorization", &format!("Bearer {token}"));
    }

    let response = req.call().unwrap_or_else(|e| {
        panic!("registry_fetch failed for {registry}{path}: {e}");
    });

    let mut body: Vec<u8> = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut body)
        .expect("read registry response body");
    body
}

// ---------------------------------------------------------------------------
// Manifest parsing
// ---------------------------------------------------------------------------

fn fetch_image_manifest(
    registry: &str,
    repo: &str,
    tag: &str,
    platform: &str,
    token: &str,
) -> (String, Vec<String>, String, String) {
    let path = format!("/v2/{repo}/manifests/{tag}");
    let manifest_data = registry_fetch(
        registry,
        &path,
        Some(
            "application/vnd.oci.image.index.v1+json, \
             application/vnd.docker.distribution.manifest.list.v2+json, \
             application/vnd.oci.image.manifest.v1+json, \
             application/vnd.docker.distribution.manifest.v2+json",
        ),
        token,
    );

    let manifest_str = String::from_utf8_lossy(&manifest_data);
    let json: serde_json::Value = serde_json::from_str(&manifest_str).expect("parse manifest JSON");

    let media_type = json["mediaType"]
        .as_str()
        .unwrap_or("application/vnd.docker.distribution.manifest.v2+json");

    let target_os = platform.split('/').next().unwrap_or("linux");
    let target_arch = platform.split('/').nth(1).unwrap_or("amd64");

    if media_type.contains("manifest.list")
        || media_type.contains("image.index")
        || json["manifests"].is_array()
    {
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
                let plat_path = format!("/v2/{repo}/manifests/{plat_digest}");
                let plat_data = registry_fetch(
                    registry,
                    &plat_path,
                    Some(
                        "application/vnd.oci.image.manifest.v1+json, \
                         application/vnd.docker.distribution.manifest.v2+json",
                    ),
                    token,
                );
                return extract_manifest_info(&plat_data, &plat_digest);
            }
        }
        panic!("Platform {platform} not found in manifest list");
    }

    let digest = json["config"]["digest"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| tag.to_string());
    extract_manifest_info(&manifest_data, &digest)
}

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

// ---------------------------------------------------------------------------
// Config parsing
// ---------------------------------------------------------------------------

fn parse_image_config_from_blob(data: &[u8]) -> ImageConfig {
    let json: serde_json::Value = serde_json::from_slice(data).expect("parse config JSON");
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

/// Fetch the image config blob from the registry and parse it.
fn fetch_image_config(
    registry: &str,
    repo: &str,
    config_digest: &str,
    token: &str,
) -> (ImageConfig, Option<String>) {
    let path = format!("/v2/{repo}/blobs/{config_digest}");
    let data = registry_fetch(registry, &path, None, token);

    let json: serde_json::Value = serde_json::from_slice(&data).expect("parse config JSON");
    let created = json["created"].as_str().map(|s| s.to_string());
    (parse_image_config_from_blob(&data), created)
}

// ---------------------------------------------------------------------------
// Layer extraction helpers
// ---------------------------------------------------------------------------

/// Decompress a gzip-compressed tar layer, then extract its contents into
/// `target_dir`.
fn extract_gzip_layer_to_dir(data: &[u8], target_dir: &std::path::Path) -> Result<(), String> {
    let mut decoder = flate2::read::GzDecoder::new(data);
    let mut tar_bytes = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut tar_bytes)
        .map_err(|e| format!("gunzip layer failed: {e}"))?;

    let cursor = std::io::Cursor::new(tar_bytes);
    let mut archive = tar::Archive::new(cursor);
    archive
        .unpack(target_dir)
        .map_err(|e| format!("extract tar into dir failed: {e}"))?;
    Ok(())
}

/// Resolve the manifest digest for an image tag (lightweight registry call).
pub(crate) fn image_digest(image: &str, platform: &str) -> String {
    let (registry, repo, tag) = parse_image_ref(image);
    let token = registry_token(&registry, &repo);
    let (manifest_digest, _layers, _config_digest, _manifest_str) =
        fetch_image_manifest(&registry, &repo, &tag, platform, &token);
    manifest_digest
}

// ---------------------------------------------------------------------------
// Main entry: pull and extract an OCI image
// ---------------------------------------------------------------------------

/// Download an OCI image from a registry and extract its filesystem layers
/// directly into `target_dir`.
///
/// This is the primary function used by both build-time and runtime code:
///
/// - **build.rs** calls it to materialise the rootfs, then splits ELF
///   binaries from non-ELF files for the Android `.apk` / `.aab` bundle.
/// - **proot.rs** (runtime) calls it via `spawn_blocking` when no embedded
///   image is available, then persists the returned metadata.
///
/// The function is synchronous and uses `curl` for HTTP.  It:
/// 1. Parses the image reference.
/// 2. Obtains an anonymous OCI bearer token.
/// 3. Fetches the manifest (handles multi-platform index).
/// 4. Downloads every layer (gzip-compressed tar) and extracts it
///    directly into `target_dir`.
/// 5. Reads and returns the image configuration metadata.
pub(crate) fn pull_and_extract_image(
    image_ref: &str,
    platform: &str,
    target_dir: &std::path::Path,
) -> Result<PulledImage, String> {
    let (registry, repo, tag) = parse_image_ref(image_ref);
    tracing::info!("Pulling {image_ref} ({platform})");
    tracing::info!("  registry={registry}, repo={repo}, tag={tag}");

    let token = registry_token(&registry, &repo);
    let (_manifest_digest, layers, config_digest, _manifest_str) =
        fetch_image_manifest(&registry, &repo, &tag, platform, &token);

    tracing::info!("Config digest: {config_digest}");
    tracing::info!("Layers: {}", layers.len());

    // Clean any previous extraction and ensure target directory exists.
    if target_dir.exists() {
        std::fs::remove_dir_all(target_dir).expect("clean target_dir for image extraction");
    }
    std::fs::create_dir_all(target_dir).expect("create target_dir for image extraction");

    // Download and extract every layer directly into target_dir.
    for (i, layer_digest) in layers.iter().enumerate() {
        let path = format!("/v2/{repo}/blobs/{layer_digest}");
        let data = registry_fetch(&registry, &path, None, &token);
        tracing::info!(
            "[{}/{}] Layer {}: {} bytes",
            i + 1,
            layers.len(),
            &layer_digest[..12.min(layer_digest.len())],
            data.len()
        );

        extract_gzip_layer_to_dir(&data, target_dir)?;
    }

    // Fetch and parse image config.
    let (image_config, created) = fetch_image_config(&registry, &repo, &config_digest, &token);

    tracing::info!("Done — rootfs extracted to {}", target_dir.display());

    Ok(PulledImage {
        image_config,
        created,
    })
}
