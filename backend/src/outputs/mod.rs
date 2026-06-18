pub mod glb;
pub use glb::generate_glb;

use anyhow::anyhow;
use dioxus::fullstack::ByteStream;
use dioxus::Result as DioxusResult;
use tracing::debug;

/// Write bytes to an output file inside a project's work directory.
/// Creates parent directories as needed.
pub async fn write_project_output(
    project_name: String,
    relative_path: String,
    mut body: ByteStream,
) -> DioxusResult<()> {
    let project_path = crate::project::resolve_project_path(&project_name).await?;
    let full_path = crate::project::resolve_project_relative_path(&project_path, &relative_path)?;

    debug!("Writing output file: {}", full_path.display());

    if let Some(parent) = full_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| anyhow!("create output dir: {e}"))?;
    }

    let mut bytes = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        bytes.extend_from_slice(&chunk);
    }

    tokio::fs::write(&full_path, &bytes)
        .await
        .map_err(|e| anyhow!("write output file: {e}"))?;

    debug!("Wrote {} bytes to {}", bytes.len(), full_path.display());
    Ok(())
}
