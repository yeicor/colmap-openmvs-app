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
///   - repo:tag (default registry: registry-1.docker.io)
///   - repo (default registry and tag)
///
/// `docker.io` and `index.docker.io` are normalised to `registry-1.docker.io`
/// because the bare `docker.io` hostname redirects to the marketing site and
/// does not serve the OCI registry API.
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

    // Normalise Docker Hub aliases that don't serve the OCI API directly.
    let registry = match registry.as_str() {
        "docker.io" | "index.docker.io" => "registry-1.docker.io".to_string(),
        _ => registry,
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

/// Minimal standard Base64 encoder (RFC 4648 §4).  Used to build the
/// `Authorization: Basic …` header without pulling in an external crate,
/// because this file is shared between the normal crate and `build.rs` via
/// `include!`, where the dependency list is different.
pub(crate) fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        out.push(TABLE[b0 >> 2] as char);
        out.push(TABLE[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(if chunk.len() > 1 { TABLE[((b1 & 0xf) << 2) | (b2 >> 6)] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[b2 & 0x3f] as char } else { '=' });
    }
    out
}

/// Retry an HTTP operation with exponential backoff. Registry/CDN blips are
/// common enough (429/5xx, dropped connections) that a single failure should
/// not abort a build that has already downloaded hundreds of megabytes.
fn retry_http<T>(what: &str, mut op: impl FnMut() -> Result<T, String>) -> T {
    const ATTEMPTS: u32 = 4;
    let mut last_error = String::new();
    for attempt in 1..=ATTEMPTS {
        match op() {
            Ok(value) => return value,
            Err(err) => {
                tracing::warn!("{what} failed (attempt {attempt}/{ATTEMPTS}): {err}");
                last_error = err;
                if attempt < ATTEMPTS {
                    std::thread::sleep(std::time::Duration::from_secs(1 << (attempt - 1)));
                }
            }
        }
    }
    panic!("{what} failed after {ATTEMPTS} attempts: {last_error}");
}

fn registry_token(registry: &str, _repo: &str) -> String {
    // Read optional credentials from the environment.  When set, they are
    // forwarded as HTTP Basic auth to the token endpoint so that the returned
    // pull token carries an authenticated identity (Docker Hub: 5 000
    // pulls/6 h per account vs 100/6 h anonymous).
    //
    // Supported env vars (checked in order):
    //   DOCKER_USERNAME / DOCKER_PASSWORD  – generic, also used by docker/login-action
    //   DOCKERHUB_USERNAME / DOCKERHUB_TOKEN – Docker Hub-specific alternative
    let (cred_user, cred_pass): (Option<String>, Option<String>) = {
        let u = std::env::var("DOCKER_USERNAME")
            .ok()
            .or_else(|| std::env::var("DOCKERHUB_USERNAME").ok());
        let p = std::env::var("DOCKER_PASSWORD")
            .ok()
            .or_else(|| std::env::var("DOCKERHUB_TOKEN").ok());
        (u, p)
    };

    let probe_url = format!("https://{}/v2/", registry);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(15)))
        // Do NOT follow redirects on the probe: registry-1.docker.io/v2/ correctly
        // returns HTTP 401 with a www-authenticate header, but if a redirect is
        // followed (e.g. to www.docker.com which returns 200) we'd see a success
        // status with no auth challenge and emit an empty token, causing every
        // subsequent manifest/blob fetch to fail with 401.
        .max_redirects(0)
        .build()
        .into();

    // Use http_status_as_error(false) to inspect non-2xx responses manually
    let response = match agent
        .head(&probe_url)
        .config()
        .http_status_as_error(false)
        .build()
        .call()
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Registry probe failed: {e}");
            return String::new();
        }
    };

    let status = response.status();
    if status.as_u16() == 401 {
        // 401 is expected — continue to parse auth challenge
    } else if status.as_u16() / 100 == 3 {
        // A 3xx with max_redirects(0) means the probe was redirected away from
        // the OCI endpoint (e.g. registry-1.docker.io/v2/ redirecting to the
        // Docker Hub marketing site).  Bail – something is wrong with the
        // registry hostname.
        tracing::warn!(
            "Registry probe got redirect {status} — is the registry hostname correct?"
        );
        return String::new();
    } else if !status.is_success() {
        tracing::warn!("Registry probe returned {status}");
        return String::new();
    }

    let auth_header = response
        .headers()
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
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

    let body = retry_http(&format!("GET {token_url}"), || {
        let mut req = agent
            .get(&token_url)
            .header("Accept", "application/json");
        // Attach Basic auth when credentials are available so the returned
        // token carries an authenticated identity (Docker Hub: 5 000
        // pulls/6 h per account vs 100/6 h anonymous from a shared CI IP).
        if let (Some(ref user), Some(ref pass)) = (&cred_user, &cred_pass) {
            let credentials = base64_encode(format!("{user}:{pass}").as_bytes());
            req = req.header("Authorization", &format!("Basic {credentials}"));
        }
        req.call()
            .map_err(|e| e.to_string())?
            .into_body()
            .read_to_string()
            .map_err(|e| e.to_string())
    });

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

/// Fetch a registry resource, retrying transient network / 5xx failures.
fn registry_fetch(registry: &str, path: &str, accept: Option<&str>, token: &str) -> Vec<u8> {
    retry_http(&format!("registry fetch {registry}{path}"), || {
        registry_fetch_once(registry, path, accept, token)
    })
}

fn registry_fetch_once(
    registry: &str,
    path: &str,
    accept: Option<&str>,
    token: &str,
) -> Result<Vec<u8>, String> {
    let url = format!("https://{registry}{path}");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(30)))
        .build()
        .into();

    let mut req = agent.get(&url).config().http_status_as_error(false).build();
    if let Some(accept_val) = accept {
        req = req.header("Accept", accept_val);
    }
    if !token.is_empty() {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }

    let mut response = req
        .call()
        .map_err(|e| format!("request failed for {registry}{path}: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {} for {registry}{path}", response.status()));
    }

    response
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024 * 1024) // 2 GB limit for image layers
        .read_to_vec()
        .map_err(|e| format!("read response body failed for {registry}{path}: {e}"))
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
///
/// This mirrors [`tar::Archive::unpack`], which cannot be used directly because
/// it recreates hard links with `link(2)`.  Android's SELinux policy forbids
/// that for apps (`neverallow untrusted_app file_type:file link`), so layers
/// containing hard links (for example `usr/bin/perl5.38.2 -> usr/bin/perl` in
/// the Debian base image) would fail to extract.  Hard links therefore fall
/// back to copying the already-extracted target file.
fn extract_gzip_layer_to_dir(data: &[u8], target_dir: &std::path::Path) -> Result<(), String> {
    let mut decoder = flate2::read::GzDecoder::new(data);
    let mut tar_bytes = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut tar_bytes)
        .map_err(|e| format!("gunzip layer failed: {e}"))?;

    if target_dir.symlink_metadata().is_err() {
        std::fs::create_dir_all(target_dir).map_err(|e| {
            format!(
                "extract tar into dir failed: failed to create `{}`: {e}",
                target_dir.display()
            )
        })?;
    }
    let target_dir = &target_dir
        .canonicalize()
        .unwrap_or_else(|_| target_dir.to_path_buf());

    let cursor = std::io::Cursor::new(tar_bytes);
    let mut archive = tar::Archive::new(cursor);

    // Directory entries are applied after all other entries so that restrictive
    // directory permissions cannot prevent creating their descendants.
    let mut directories = Vec::new();
    let entries = archive
        .entries()
        .map_err(|e| format!("extract tar into dir failed: failed to iterate over archive: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| {
            format!("extract tar into dir failed: failed to iterate over archive: {e}")
        })?;
        if entry.header().entry_type().is_dir() {
            directories.push(entry);
        } else {
            unpack_layer_entry(&mut entry, target_dir)?;
        }
    }
    // Deepest directories first, so that restrictive permissions on a parent
    // cannot block applying the permissions of its children.
    directories.sort_by(|a, b| b.path_bytes().cmp(&a.path_bytes()));
    for mut entry in directories {
        entry
            .unpack_in(target_dir)
            .map_err(|e| format!("extract tar into dir failed: {e}"))?;
    }

    Ok(())
}

/// Extract a single (non-directory) layer entry, copying hard links when the
/// filesystem does not allow creating them.
fn unpack_layer_entry<R: std::io::Read>(
    entry: &mut tar::Entry<'_, R>,
    target_dir: &std::path::Path,
) -> Result<(), String> {
    let is_hard_link = entry.header().entry_type().is_hard_link();
    match entry.unpack_in(target_dir) {
        Ok(_) => Ok(()),
        // Hard links are the one entry type that cannot be recreated on
        // Android, so unpack the file contents instead of linking to them.
        Err(link_err) if is_hard_link => copy_hard_link_entry(entry, target_dir).map_err(|e| {
            let path = entry
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "<unknown>".to_string());
            format!(
                "extract tar into dir failed: failed to unpack `{path}`: {e} (hard link fallback after: {link_err})"
            )
        }),
        Err(e) => Err(format!("extract tar into dir failed: {e}")),
    }
}

/// Copy the target of a hard-link entry into the link's location.
///
/// `tar` validates that both paths stay inside the extraction root; the same
/// checks are repeated here before touching the filesystem.
fn copy_hard_link_entry<R: std::io::Read>(
    entry: &mut tar::Entry<'_, R>,
    target_dir: &std::path::Path,
) -> std::io::Result<()> {
    let path = entry.path()?;
    let link_name = entry.link_name()?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "hard link listed but no link name found",
        )
    })?;

    let Some(dst) = join_layer_path(target_dir, &path) else {
        // The entry path contains `..`; `tar` silently skips such entries.
        return Ok(());
    };
    if dst == target_dir {
        return Ok(());
    }
    let Some(src) = join_layer_path(target_dir, &link_name) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "hard link target escapes the rootfs: {}",
                link_name.display()
            ),
        ));
    };

    let Some(parent) = dst.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)?;
    // An earlier layer may have replaced a directory with a symlink, so make
    // sure the destination still resolves inside the extraction root.
    let canon_target = std::fs::canonicalize(target_dir)?;
    let canon_parent = std::fs::canonicalize(parent)?;
    if !canon_parent.starts_with(&canon_target) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "hard link destination escapes the rootfs: {}",
                dst.display()
            ),
        ));
    }

    let canon_src = std::fs::canonicalize(&src)?;
    if !canon_src.starts_with(&canon_target) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("hard link target escapes the rootfs: {}", src.display()),
        ));
    }

    // Match `tar`'s overwrite behaviour, and unlink first so that an existing
    // symlink cannot redirect the copy outside the rootfs.
    match std::fs::remove_file(&dst) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    // `fs::copy` also copies the permission bits, matching the hard link.
    std::fs::copy(&canon_src, &dst)?;
    Ok(())
}

/// Join an archive-relative path onto the extraction root, skipping `.` and
/// leading `/` components and rejecting `..`, like `tar` does.
fn join_layer_path(base: &std::path::Path, path: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut joined = base.to_path_buf();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(..)
            | std::path::Component::RootDir
            | std::path::Component::CurDir => {}
            std::path::Component::ParentDir => return None,
            std::path::Component::Normal(part) => joined.push(part),
        }
    }
    Some(joined)
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
        tracing::info!(
            "[{}/{}] Downloading layer {}...",
            i + 1,
            layers.len(),
            &layer_digest[..12.min(layer_digest.len())]
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A gzip-compressed layer with a regular file and a hard link to it,
    /// matching the Debian base layer layout that fails on Android.
    fn layer_with_hard_link() -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());

        let data = b"perl binary";
        let mut file_header = tar::Header::new_gnu();
        file_header.set_size(data.len() as u64);
        file_header.set_mode(0o755);
        file_header.set_entry_type(tar::EntryType::Regular);
        builder
            .append_data(&mut file_header, "usr/bin/perl", &data[..])
            .unwrap();

        let mut link_header = tar::Header::new_gnu();
        link_header.set_size(0);
        link_header.set_mode(0o755);
        link_header.set_entry_type(tar::EntryType::Link);
        link_header.set_link_name("usr/bin/perl").unwrap();
        builder
            .append_data(&mut link_header, "usr/bin/perl5.38.2", std::io::empty())
            .unwrap();

        let tar_bytes = builder.into_inner().unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn extracts_hard_links_by_copying_when_linking_fails() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("rootfs");
        std::fs::create_dir_all(target.join("usr/bin")).unwrap();

        // Pre-create the link destination: `link(2)` fails with `AlreadyExists`,
        // which is what Android effectively does for every hard link.
        std::fs::write(target.join("usr/bin/perl5.38.2"), b"stale").unwrap();

        extract_gzip_layer_to_dir(&layer_with_hard_link(), &target).unwrap();

        let perl = std::fs::read(target.join("usr/bin/perl")).unwrap();
        let link = std::fs::read(target.join("usr/bin/perl5.38.2")).unwrap();
        assert_eq!(perl, b"perl binary");
        assert_eq!(link, b"perl binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(target.join("usr/bin/perl5.38.2"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[test]
    fn join_layer_path_rejects_parent_components() {
        let base = std::path::Path::new("/rootfs");
        assert_eq!(
            join_layer_path(base, std::path::Path::new("/usr/bin/perl")),
            Some(std::path::PathBuf::from("/rootfs/usr/bin/perl"))
        );
        assert_eq!(
            join_layer_path(base, std::path::Path::new("../../etc/passwd")),
            None
        );
    }

    #[test]
    fn parse_image_ref_normalises_docker_io_aliases() {
        // Bare `docker.io` and `index.docker.io` must be rewritten to the real
        // OCI registry endpoint; the bare hostnames redirect to the marketing site.
        for alias in &["docker.io", "index.docker.io"] {
            let (registry, repo, tag) =
                parse_image_ref(&format!("{alias}/yeicor/colmap-openmvs:cpu-latest"));
            assert_eq!(
                registry, "registry-1.docker.io",
                "{alias} should be normalised"
            );
            assert_eq!(repo, "yeicor/colmap-openmvs");
            assert_eq!(tag, "cpu-latest");
        }
        // Already-canonical form must pass through unchanged.
        let (registry, repo, tag) =
            parse_image_ref("registry-1.docker.io/yeicor/colmap-openmvs:cpu-latest");
        assert_eq!(registry, "registry-1.docker.io");
        assert_eq!(repo, "yeicor/colmap-openmvs");
        assert_eq!(tag, "cpu-latest");
    }
}
