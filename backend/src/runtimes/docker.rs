//! Docker runtime implementation using the system `docker` CLI.
//!
//! This runtime delegates all container operations to the `docker` binary found
//! in `$PATH`.  It requires no extra download step – if Docker is installed on
//! the host machine it will work out of the box.

use super::{
    ImageHash, ImageTag, Mount, PrepareProgressTx, PreparedImage, ProcessHandle, Runtime,
    RuntimeResult,
};
use async_trait::async_trait;
use colmap_openmvs_api::PrepareProgress;
use std::process::Stdio;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Docker struct
// ---------------------------------------------------------------------------

/// A zero-sized runtime that delegates container operations to the system
/// `docker` binary.  All instances are interchangeable.
pub struct Docker;

impl Docker {
    pub fn new() -> Self {
        Docker
    }

    /// Parse a human-readable size string returned by `docker images`
    /// (e.g. `"77.9MB"`, `"1.23GB"`, `"456kB"`, `"789B"`) into bytes.
    /// Docker uses SI (decimal) prefixes: 1 kB = 1 000 B.
    fn parse_size(s: &str) -> u64 {
        let s = s.trim();
        // Find where the numeric part ends
        let split_pos = s
            .rfind(|c: char| c.is_ascii_digit() || c == '.')
            .map(|i| i + 1)
            .unwrap_or(s.len());
        let (num_str, unit) = s.split_at(split_pos);
        let num: f64 = num_str.trim().parse().unwrap_or(0.0);
        match unit.trim().to_ascii_uppercase().as_str() {
            "TB" | "TIB" => (num * 1_000_000_000_000.0) as u64,
            "GB" | "GIB" => (num * 1_000_000_000.0) as u64,
            "MB" | "MIB" => (num * 1_000_000.0) as u64,
            "KB" | "KIB" => (num * 1_000.0) as u64,
            _ => num as u64,
        }
    }

    /// Convert the `CreatedAt` field from `docker images` JSON output
    /// (`"2024-01-15 10:30:45 +0000 UTC"`) to an RFC-3339 string.
    fn parse_created_at(s: &str) -> Option<String> {
        use chrono::DateTime;
        // Strip the trailing " UTC" or "UTC" if present, since the offset "+0000" already
        // carries the information and the extra token confuses the parser.
        let cleaned = s
            .trim()
            .trim_end_matches("UTC")
            .trim_end_matches("utc")
            .trim();
        DateTime::parse_from_str(cleaned, "%Y-%m-%d %H:%M:%S %z")
            .map(|dt| dt.to_rfc3339())
            .ok()
    }
}

impl Default for Docker {
    fn default() -> Self {
        Docker::new()
    }
}

// ---------------------------------------------------------------------------
// Runtime trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Runtime for Docker {
    // ── Support check ────────────────────────────────────────────────────────

    fn is_supported(&self) -> RuntimeResult<()> {
        which::which("docker").map(|_| ()).map_err(|_| {
            anyhow::anyhow!(
                "The `docker` binary was not found in $PATH. \
                 Please install Docker and make sure it is accessible."
            )
        })
    }

    // ── Version ──────────────────────────────────────────────────────────────

    async fn version(&self) -> RuntimeResult<String> {
        // Try the structured format first; fall back to the human-readable one.
        let output = Command::new("docker")
            .args(["version", "--format", "{{.Server.Version}}"])
            .output()
            .await;

        if let Ok(out) = output {
            if out.status.success() {
                let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !v.is_empty() {
                    return Ok(v);
                }
            }
        }

        // Fallback: `docker --version` → "Docker version 24.0.7, build afdd53b"
        let out = Command::new("docker")
            .arg("--version")
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to run `docker --version`: {}", e))?;

        if out.status.success() {
            let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // Extract just the version number after "version "
            let version = line
                .split("version ")
                .nth(1)
                .and_then(|s| s.split(',').next())
                .map(|s| s.trim().to_string())
                .unwrap_or(line);
            Ok(version)
        } else {
            Err(anyhow::anyhow!("`docker --version` failed"))
        }
    }

    // ── Available versions ───────────────────────────────────────────────────

    async fn available_versions(&self) -> RuntimeResult<Vec<String>> {
        // Docker is managed by the system package manager; there are no
        // downloadable versions through this app.
        Ok(vec![])
    }

    // ── Download / install ───────────────────────────────────────────────────

    async fn download(&self, _version: &str) -> RuntimeResult<()> {
        Err(anyhow::anyhow!(
            "Docker is managed by the system package manager. \
             Please install or update Docker through your OS package manager \
             (e.g. `apt install docker.io` or `brew install docker`)."
        ))
    }

    // ── Prepare (docker pull) ────────────────────────────────────────────────

    async fn prepare(&self, image: &str, tx: PrepareProgressTx) -> RuntimeResult<()> {
        info!(image, "docker pull: starting");

        let mut child = Command::new("docker")
            .args(["pull", image])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn `docker pull`: {}", e))?;

        // Docker writes progress output to stderr in non-TTY mode.
        let stderr = child.stderr.take().expect("stderr should be piped");
        // Drain stdout so the OS pipe buffer never fills up (blocking the child).
        let stdout = child.stdout.take().expect("stdout should be piped");

        // --- drain stdout in background ---
        tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(stdout).lines();
            while let Ok(Some(_)) = reader.next_line().await {}
        });

        // --- parse stderr for progress events ---
        let tx_clone = tx.clone();
        let stderr_handle = tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(stderr).lines();
            let mut layers_total = 0usize;
            let mut layers_done = 0usize;
            let mut sent_initial = false;

            while let Ok(Some(line)) = reader.next_line().await {
                let line = line.trim().to_string();
                debug!("docker pull stderr: {}", line);

                if line.contains(": Pulling from") {
                    // e.g. "22.04: Pulling from library/ubuntu"
                    if !sent_initial {
                        let _ = tx_clone
                            .send(PrepareProgress::Downloading {
                                downloaded_bytes: 0,
                                total_bytes: None,
                            })
                            .await;
                        sent_initial = true;
                    }
                } else if line.ends_with(": Pulling fs layer") || line.contains(": Waiting") {
                    layers_total += 1;
                } else if line.ends_with(": Pull complete")
                    || line.ends_with(": Already exists")
                    || line.ends_with(": Verifying Checksum")
                    || line.ends_with(": Download complete")
                {
                    layers_done += 1;
                    let total = layers_total.max(layers_done);
                    let progress = layers_done as f32 / total as f32;
                    let _ = tx_clone
                        .send(PrepareProgress::ExtractingLayer {
                            layer: format!("{}/{}", layers_done, total),
                            progress,
                        })
                        .await;
                } else if line.to_lowercase().contains("error") || line.starts_with("ERROR") {
                    warn!("docker pull error line: {}", line);
                    let _ = tx_clone
                        .send(PrepareProgress::Error {
                            message: line.clone(),
                        })
                        .await;
                }
            }
        });

        // Wait for the child to exit first, then collect stderr output.
        let status = child
            .wait()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to wait for `docker pull`: {}", e))?;

        // Wait for the stderr reader to drain completely.
        let _ = stderr_handle.await;

        if status.success() {
            info!(image, "docker pull: completed successfully");
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "`docker pull {}` failed with exit code {:?}",
                image,
                status.code()
            ))
        }
    }

    // ── Remove (docker rmi) ──────────────────────────────────────────────────

    async fn remove(&self, image: &str) -> RuntimeResult<()> {
        let out = Command::new("docker")
            .args(["rmi", image])
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to run `docker rmi`: {}", e))?;

        if out.status.success() {
            info!(image, "docker rmi: removed");
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr);
            error!(image, %stderr, "docker rmi failed");
            Err(anyhow::anyhow!("`docker rmi {}` failed: {}", image, stderr))
        }
    }

    // ── Run ──────────────────────────────────────────────────────────────────

    async fn run(
        &self,
        image: &str,
        args: &[String],
        mounts: &[Mount],
        env_vars: &[(&str, &str)],
    ) -> RuntimeResult<ProcessHandle> {
        let mut cmd = Command::new("docker");
        cmd.arg("run").arg("--rm").arg("-i");

        for mount in mounts {
            cmd.arg("-v").arg(format!(
                "{}:{}",
                mount.host_path.display(),
                mount.container_path
            ));
        }

        for (key, val) in env_vars {
            cmd.arg("-e").arg(format!("{}={}", key, val));
        }

        cmd.arg(image);
        cmd.args(args);

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn `docker run`: {}", e))?;

        Ok(ProcessHandle { child })
    }

    // ── List images ──────────────────────────────────────────────────────────

    async fn list_images(&self) -> RuntimeResult<Vec<PreparedImage>> {
        let out = Command::new("docker")
            .args(["images", "--format", "{{json .}}"])
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to run `docker images`: {}", e))?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(anyhow::anyhow!("`docker images` failed: {}", stderr));
        }

        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut images = Vec::new();

        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let json: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(e) => {
                    warn!("docker images: failed to parse JSON line: {} — {}", line, e);
                    continue;
                }
            };

            let repo = json
                .get("Repository")
                .and_then(|v| v.as_str())
                .unwrap_or("<none>")
                .to_string();
            let tag_str = json
                .get("Tag")
                .and_then(|v| v.as_str())
                .unwrap_or("<none>")
                .to_string();
            let size_str = json.get("Size").and_then(|v| v.as_str()).unwrap_or("0B");
            let created_at_str = json
                .get("CreatedAt")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Skip dangling / intermediate images
            if repo == "<none>" && tag_str == "<none>" {
                continue;
            }

            let full_tag = if tag_str == "<none>" {
                repo.clone()
            } else {
                format!("{}:{}", repo, tag_str)
            };

            let size = Self::parse_size(size_str);
            let build_date = created_at_str.as_deref().and_then(Self::parse_created_at);

            // Use the full tag string as the hash so that `docker rmi <hash>` works.
            images.push(PreparedImage::with_build_date(
                ImageTag::from_string(full_tag.clone()),
                ImageHash::new(full_tag),
                size,
                build_date,
            ));
        }

        Ok(images)
    }

    // ── Delete binary ────────────────────────────────────────────────────────

    async fn delete_binary(&self) -> RuntimeResult<()> {
        Err(anyhow::anyhow!(
            "Docker is a system-managed binary and cannot be removed from within this app. \
             Use your OS package manager to uninstall Docker."
        ))
    }
}
