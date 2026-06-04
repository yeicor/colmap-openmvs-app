//! Docker-in-Docker path translation.
//!
//! When the application runs inside a Docker container that has the host's
//! Docker socket mounted (`/var/run/docker.sock`), any volume paths it passes
//! to `docker run` are interpreted by the **host** Docker daemon using the
//! **host's** filesystem layout — not the container's.
//!
//! For example, if the container was started with:
//!   `docker run -v /mnt/storage/data:/data ...`
//!
//! Then inside the container the projects are at `/data/projects/foo`, but the
//! host sees them at `/mnt/storage/data/projects/foo`.  Passing the container
//! path directly to a sibling `docker run -v` will fail because that path
//! doesn't exist on the host.
//!
//! This module detects the containerised environment and queries Docker (or
//! falls back to `/proc/self/mountinfo`) to build a container→host path
//! translation table so that all mounts are transparently rewritten.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Static global state (lazily initialised once)
// ---------------------------------------------------------------------------

static DIND_STATE: OnceLock<DockerInDocker> = OnceLock::new();

/// Access the global DinD translation state.
///
/// The first call performs detection and path-resolution; subsequent calls
/// return the cached result.
fn global() -> &'static DockerInDocker {
    DIND_STATE.get_or_init(DockerInDocker::detect)
}

// ---------------------------------------------------------------------------
// Core struct
// ---------------------------------------------------------------------------

/// Holds the result of container-detection and the host-path translation map.
#[derive(Debug)]
pub(crate) struct DockerInDocker {
    /// Whether we positively identified that we are inside a Docker container.
    pub(crate) detected: bool,
    /// Ordered list of (host_root, container_root) pairs.
    ///
    /// When resolving paths we try the *longest* matching container_root first
    /// so that nested mounts produce correct results.
    ///
    /// Invariant: all paths are canonicalised and absolute.
    mappings: Vec<(PathBuf, PathBuf)>,
}

impl DockerInDocker {
    // ── Detection ──────────────────────────────────────────────────────────

    /// Run detection and build the translation map.
    fn detect() -> Self {
        if !is_running_in_container() {
            debug!("docker_dind: not inside a container — no path translation needed");
            return Self {
                detected: false,
                mappings: Vec::new(),
            };
        }
        info!("docker_dind: detected containerised environment");

        // ── Strategy 1: docker inspect (most reliable) ─────────────────
        match resolve_via_docker_inspect() {
            Some(mappings) if !mappings.is_empty() => {
                info!(
                    "docker_dind: resolved {} mount mapping(s) via docker inspect",
                    mappings.len()
                );
                for (host, container) in &mappings {
                    debug!(
                        "docker_dind:   {}  ←→  {}",
                        container.display(),
                        host.display()
                    );
                }
                return Self {
                    detected: true,
                    mappings,
                };
            }
            other => {
                debug!(
                    "docker_dind: docker inspect produced {} mappings — will try mountinfo",
                    other.as_ref().map_or(0, Vec::len)
                );
            }
        }

        // ── Strategy 2: /proc/self/mountinfo (fallback) ────────────────
        match resolve_via_mountinfo() {
            Some(mappings) if !mappings.is_empty() => {
                info!(
                    "docker_dind: resolved {} mount mapping(s) via /proc/self/mountinfo",
                    mappings.len()
                );
                for (host, container) in &mappings {
                    debug!(
                        "docker_dind:   {}  ←→  {}",
                        container.display(),
                        host.display()
                    );
                }
                return Self {
                    detected: true,
                    mappings,
                };
            }
            _ => {
                warn!(
                    "docker_dind: could not resolve any host-path mappings from mountinfo either"
                );
            }
        }

        warn!(
            "docker_dind: running in container but unable to resolve host paths — \
             volume mounts will use container paths and likely fail on the host"
        );
        Self {
            detected: true,
            mappings: Vec::new(),
        }
    }

    // ── Path resolution ────────────────────────────────────────────────────

    /// Translate a single *container-visible* path to the corresponding host
    /// path that can be passed to `docker run -v`.
    ///
    /// Works by walking the mount mappings and picking the *longest* matching
    /// container prefix so that nested mounts (e.g. `/data` + `/data/projects`)
    /// are handled correctly.
    ///
    /// If no mapping matches, the original path is returned unchanged and a
    /// warning is emitted.
    pub(crate) fn resolve_host_path(&self, container_path: &Path) -> PathBuf {
        if !self.detected || self.mappings.is_empty() {
            return container_path.to_path_buf();
        }

        // Try to find the best (longest prefix) match.
        // We iterate in reverse so that later entries (added by mountinfo which
        // may be in mount-order) can override earlier ones, but we explicitly
        // track the longest container_root for correctness.
        let mut best: Option<(usize, &PathBuf, &PathBuf)> = None;

        for (host_root, container_root) in &self.mappings {
            if let Ok(relative) = container_path.strip_prefix(container_root) {
                let match_len = container_root.as_os_str().len();
                let replace = match best {
                    Some((prev_len, _, _)) => match_len > prev_len,
                    None => true,
                };
                if replace {
                    best = Some((match_len, host_root, container_root));
                    let _ = relative; // we'll reconstruct later
                }
            }
        }

        if let Some((_, host_root, container_root)) = best {
            // SAFETY: we already know the strip_prefix succeeds via the check above
            if let Ok(relative) = container_path.strip_prefix(container_root) {
                let host_path = host_root.join(relative);
                debug!(
                    "docker_dind: translated {} → {}",
                    container_path.display(),
                    host_path.display()
                );
                return host_path;
            }
        }

        warn!(
            "docker_dind: no mapping found for {} — using original path (may fail on host)",
            container_path.display()
        );
        container_path.to_path_buf()
    }

    /// Convenience: translate and return a `String`.
    pub(crate) fn resolve_host_path_str(&self, container_path: &str) -> String {
        self.resolve_host_path(Path::new(container_path))
            .to_string_lossy()
            .into_owned()
    }
}

// ---------------------------------------------------------------------------
// Container detection helpers
// ---------------------------------------------------------------------------

/// Check whether the current process is likely running inside a Docker
/// (or containerd) container.
fn is_running_in_container() -> bool {
    // ── Marker file ──────────────────────────────────────────────────────
    if Path::new("/.dockerenv").exists() {
        debug!("docker_dind: detected via /.dockerenv");
        return true;
    }

    // ── cgroup v1: /proc/1/cgroup ────────────────────────────────────────
    if let Ok(contents) = std::fs::read_to_string("/proc/1/cgroup") {
        for line in contents.lines() {
            if line.contains("/docker/")
                || line.contains("/docker-ce/")
                || line.contains("/containerd/")
            {
                debug!(
                    "docker_dind: detected via /proc/1/cgroup (line: {})",
                    line.trim()
                );
                return true;
            }
            // cgroup v2 style: 0::/system.slice/docker-<id>.scope
            if line.contains("docker-") {
                debug!(
                    "docker_dind: detected via /proc/1/cgroup (cgroup v2 docker, line: {})",
                    line.trim()
                );
                return true;
            }
        }
    }

    // ── PID 1 scheduling identity ────────────────────────────────────────
    // Inside a container PID 1 is usually the application entrypoint (bash,
    // sh, tini, the app itself, etc.) rather than systemd or init(1).
    if let Ok(contents) = std::fs::read_to_string("/proc/1/sched") {
        if let Some(first_line) = contents.lines().next() {
            let comm = first_line
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_end_matches(':');
            // Common container init processes
            if matches!(comm, "bash" | "sh" | "tini" | "dumb-init" | "catatonit") {
                debug!("docker_dind: detected via /proc/1/sched (comm={comm})");
                return true;
            }
        }
    }

    debug!("docker_dind: no container indicators found");
    false
}

// ---------------------------------------------------------------------------
// Container ID discovery
// ---------------------------------------------------------------------------

/// Try to determine the current container's ID using several strategies.
///
/// Returns `None` when the ID cannot be determined (e.g. not in a container,
/// or the cgroup format is unusual).
fn get_container_id() -> Option<String> {
    // ── Strategy 1: HOSTNAME env var ────────────────────────────────────
    // Docker sets HOSTNAME to the container ID (short form).
    if let Ok(hostname) = std::env::var("HOSTNAME") {
        let trimmed = hostname.trim().to_string();
        if trimmed.len() >= 12 {
            debug!("docker_dind: container ID from HOSTNAME: {}", trimmed);
            return Some(trimmed);
        }
    }

    // ── Strategy 2: /proc/1/cgroup (cgroup v1) ──────────────────────────
    if let Ok(contents) = std::fs::read_to_string("/proc/1/cgroup") {
        for line in contents.lines() {
            // Format: "1:name:/docker/<id>"
            if let Some(pos) = line.rfind("/docker/") {
                let id = line[pos + "/docker/".len()..].trim().to_string();
                if !id.is_empty() {
                    debug!("docker_dind: container ID from /proc/1/cgroup (v1 docker): {id}");
                    return Some(id);
                }
            }
            // Format: "1:name:/docker-ce/<id>"
            if let Some(pos) = line.rfind("/docker-ce/") {
                let id = line[pos + "/docker-ce/".len()..].trim().to_string();
                if !id.is_empty() {
                    debug!("docker_dind: container ID from /proc/1/cgroup (v1 docker-ce): {id}");
                    return Some(id);
                }
            }
        }
    }

    // ── Strategy 3: /proc/1/cgroup (cgroup v2) ──────────────────────────
    if let Ok(contents) = std::fs::read_to_string("/proc/1/cgroup") {
        for line in contents.lines() {
            // Format: "0::/system.slice/docker-<id>.scope"
            if let Some(pos) = line.find("docker-") {
                let after = &line[pos + "docker-".len()..];
                if let Some(end) = after.find(".scope") {
                    let id = after[..end].trim().to_string();
                    if id.len() >= 12 {
                        debug!("docker_dind: container ID from /proc/1/cgroup (v2 docker): {id}");
                        return Some(id);
                    }
                }
            }
        }
    }

    // ── Strategy 4: /proc/self/mountinfo ────────────────────────────────
    // For overlay2, the path contains the container/image ID.
    if let Ok(contents) = std::fs::read_to_string("/proc/self/mountinfo") {
        for line in contents.lines() {
            if let Some(pos) = line.find("/docker/overlay2/") {
                let after = &line[pos + "/docker/overlay2/".len()..];
                if let Some(slash) = after.find('/') {
                    let id = after[..slash].trim().to_string();
                    if id.len() >= 12 {
                        debug!("docker_dind: container ID from mountinfo overlay2: {id}");
                        return Some(id);
                    }
                }
            }
            // Also check containerd overlay paths
            if let Some(pos) = line.find("/containerd/") {
                let rest = &line[pos + "/containerd/".len()..];
                // Try extract something that looks like an ID
                for part in rest.split('/') {
                    if part.len() >= 12 && part.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                        debug!("docker_dind: container ID from mountinfo containerd: {part}");
                        return Some(part.to_string());
                    }
                }
            }
        }
    }

    warn!("docker_dind: could not determine container ID from any source");
    None
}

// ---------------------------------------------------------------------------
// Mount resolution strategies
// ---------------------------------------------------------------------------

/// Resolve host-path mappings by inspecting our own container via the Docker
/// API (requires the docker binary + reachable daemon).
fn resolve_via_docker_inspect() -> Option<Vec<(PathBuf, PathBuf)>> {
    let container_id = get_container_id()?;
    debug!("docker_dind: running docker inspect for container {container_id}");

    let output = std::process::Command::new("docker")
        .args(["inspect", "--format", "{{json .Mounts}}", &container_id])
        .output()
        .ok()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(
            "docker_dind: docker inspect failed (exit={:?}): {}",
            output.status.code(),
            stderr.trim()
        );
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mounts: Vec<DockerMount> = match serde_json::from_str(&stdout) {
        Ok(v) => v,
        Err(e) => {
            warn!("docker_dind: failed to parse docker inspect JSON: {e}");
            return None;
        }
    };

    let mut mappings: Vec<(PathBuf, PathBuf)> = Vec::new();
    for m in &mounts {
        let host = PathBuf::from(&m.source);
        let container = PathBuf::from(&m.destination);

        // Only include mounts where both paths are absolute
        if host.is_absolute() && container.is_absolute() {
            mappings.push((host, container));
            debug!(
                "docker_dind:   mount {} → {} (type: {})",
                m.destination, m.source, m.type_
            );
        } else {
            debug!(
                "docker_dind:   skipping non-absolute mount: {} → {}",
                m.destination, m.source
            );
        }
    }

    if mappings.is_empty() {
        warn!("docker_dind: docker inspect returned no usable mount mappings");
        return None;
    }

    Some(mappings)
}

/// Fallback: parse `/proc/self/mountinfo` to extract bind-mount mappings.
///
/// This works without Docker API access but may be less accurate for volumes
/// or advanced storage drivers.
fn resolve_via_mountinfo() -> Option<Vec<(PathBuf, PathBuf)>> {
    let contents = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    let mut mappings: Vec<(PathBuf, PathBuf)> = Vec::new();

    for line in contents.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        // Fields: id parent maj:min root mountpoint options... - fs_type source super_options
        if fields.len() < 10 {
            continue;
        }

        let root = fields[3]; // path on the device (host path for bind mounts)
        let mount_point = fields[4]; // path inside current namespace

        // Find the separator '-'
        let sep_pos = fields.iter().position(|&f| f == "-")?;
        if sep_pos + 2 >= fields.len() {
            continue;
        }
        let _fs_type = fields[sep_pos + 1];
        let _source = fields[sep_pos + 2];

        // Skip pseudo-filesystems
        if mount_point.starts_with("/proc/")
            || mount_point.starts_with("/sys/")
            || mount_point.starts_with("/dev/")
        {
            continue;
        }

        // Consider only entries where the root is an absolute path — these
        // correspond to bind mounts (as opposed to plain device mounts where
        // root is usually "/").
        if !root.starts_with('/') {
            continue;
        }

        let host_path = PathBuf::from(root);
        let container_path = PathBuf::from(mount_point);

        // Canonicalise to resolve any symlinks in the host path (e.g.
        // `/var/lib/docker/volumes/.../_data`).
        let host_path = std::fs::canonicalize(&host_path).unwrap_or(host_path);

        mappings.push((host_path, container_path));
    }

    if mappings.is_empty() {
        return None;
    }

    Some(mappings)
}

// ---------------------------------------------------------------------------
// Public API (used by the Docker runtime)
// ---------------------------------------------------------------------------

/// Translate a path visible inside the container into the corresponding host
/// path suitable for `docker run -v`.
///
/// This is the main entrypoint called by the Docker runtime's `run` method.
/// It transparently handles Docker-in-Docker scenarios without any extra
/// configuration.
///
/// If the app is not running inside a container, the original path is returned
/// unchanged (no overhead).
pub(crate) fn resolve_host_path(path: &Path) -> PathBuf {
    global().resolve_host_path(path)
}

/// String-typed convenience wrapper around [`resolve_host_path`].
pub(crate) fn resolve_host_path_str(path: &str) -> String {
    global().resolve_host_path_str(path)
}

/// Returns `true` if the DinD module detected a containerised environment
/// and successfully resolved host-path mappings.
///
/// Useful for debugging / structured logging at startup.
pub(crate) fn is_active() -> bool {
    let state = global();
    state.detected && !state.mappings.is_empty()
}

/// Returns a human-readable summary of the DinD state for diagnostics.
pub(crate) fn diagnostic_summary() -> String {
    let state = global();
    if !state.detected {
        return "DinD: not detected (not running inside a container)".to_string();
    }
    if state.mappings.is_empty() {
        return "DinD: detected but no host-path mappings resolved".to_string();
    }
    let mut lines = format!("DinD: active with {} mapping(s)", state.mappings.len());
    for (host, container) in &state.mappings {
        lines.push_str(&format!(
            "\n  {}  ←→  {}",
            container.display(),
            host.display()
        ));
    }
    lines
}

// ---------------------------------------------------------------------------
// JSON deserialisation helper for `docker inspect`
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize, Debug)]
struct DockerMount {
    #[serde(rename = "Type")]
    type_: String,
    #[serde(rename = "Source")]
    source: String,
    #[serde(rename = "Destination")]
    destination: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_mappings() -> DockerInDocker {
        DockerInDocker {
            detected: true,
            mappings: vec![
                (PathBuf::from("/var/lib/data"), PathBuf::from("/data")),
                (
                    PathBuf::from("/home/user/extras"),
                    PathBuf::from("/data/extras"),
                ),
            ],
        }
    }

    #[test]
    fn test_path_translation_simple() {
        let dind = test_mappings();
        let result = dind.resolve_host_path(Path::new("/data/projects/my-project"));
        assert_eq!(result, PathBuf::from("/var/lib/data/projects/my-project"));
    }

    #[test]
    fn test_path_translation_nested_mount() {
        let dind = test_mappings();
        let result = dind.resolve_host_path(Path::new("/data/extras/some-tool"));
        // Should match the more specific /data/extras → /home/user/extras mapping
        assert_eq!(result, PathBuf::from("/home/user/extras/some-tool"));
    }

    #[test]
    fn test_path_translation_no_match() {
        let dind = test_mappings();
        let result = dind.resolve_host_path(Path::new("/unmounted/path"));
        assert_eq!(result, PathBuf::from("/unmounted/path"));
    }

    #[test]
    fn test_path_translation_not_detected() {
        let dind = DockerInDocker {
            detected: false,
            mappings: vec![(PathBuf::from("/host/data"), PathBuf::from("/data"))],
        };
        let result = dind.resolve_host_path(Path::new("/data/projects/x"));
        assert_eq!(result, PathBuf::from("/data/projects/x"));
    }

    #[test]
    fn test_no_translation_when_empty() {
        let dind = DockerInDocker {
            detected: true,
            mappings: vec![],
        };
        let result = dind.resolve_host_path(Path::new("/data/projects/x"));
        assert_eq!(result, PathBuf::from("/data/projects/x"));
    }
}
