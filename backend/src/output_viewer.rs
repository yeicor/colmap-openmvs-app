//! Output viewer: converts project output files to GLB for the 3-D viewer.
//!
//! * `.ply`        → parsed and re-encoded as GLB (handles textured/untextured mesh + point cloud)
//! * `points3D.bin` → decoded and written as a GLB point cloud

use dioxus::Result as DioxusResult;
use tracing::{debug, warn};

// ─────────────────────────────────────────────────────────────────────────────
// Public entry point
// ─────────────────────────────────────────────────────────────────────────────

pub async fn get_project_output_for_viewer(
    project_name: String,
    relative_path: String,
) -> DioxusResult<Vec<u8>> {
    let project_path = {
        let projects = crate::get_projects().await?;
        projects
            .into_iter()
            .find(|p| p.name == project_name)
            .map(|p| p.path)
            .ok_or_else(|| anyhow::anyhow!("Project not found: {}", project_name))?
    };

    let sanitised = relative_path.trim_start_matches('/');
    for component in std::path::Path::new(sanitised).components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(anyhow::anyhow!("Path traversal is not allowed").into());
        }
    }

    let full_path = std::path::Path::new(&project_path).join(sanitised);
    if !full_path.exists() {
        return Err(anyhow::anyhow!("Output file not found: {:?}", full_path).into());
    }

    let file_name = full_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    if file_name == "points3d.bin" {
        debug!("Converting points3D.bin → GLB");
        let bytes = tokio::fs::read(&full_path).await.map_err(|e| {
            anyhow::anyhow!("Failed to read points3D.bin: {}", e)
        })?;
        let glb = points3d_bin_to_glb(&bytes)
            .map_err(|e| anyhow::anyhow!("Failed to convert points3D.bin: {}", e))?;
        Ok(glb)
    } else {
        // PLY (or any other supported file)
        debug!("Converting PLY → GLB: {:?}", full_path);
        let bytes = tokio::fs::read(&full_path).await.map_err(|e| {
            anyhow::anyhow!("Failed to read output file: {}", e)
        })?;

        // Check for a companion texture in the same directory
        let companion_png = if let Some(tex_name) =
            crate::ply_to_glb::ply_texture_file_name(&bytes)
        {
            let tex_path = full_path
                .parent()
                .map(|d| d.join(&tex_name))
                .filter(|p| p.exists());

            match tex_path {
                Some(p) => match tokio::fs::read(&p).await {
                    Ok(png) => {
                        debug!("Loaded companion texture: {:?}", p);
                        Some(png)
                    }
                    Err(e) => {
                        warn!("Could not read companion texture {:?}: {}", p, e);
                        None
                    }
                },
                None => {
                    warn!("Companion texture '{}' not found next to PLY", tex_name);
                    None
                }
            }
        } else {
            None
        };

        let glb = crate::ply_to_glb::ply_to_glb(&bytes, companion_png)
            .map_err(|e| anyhow::anyhow!("Failed to convert PLY to GLB: {}", e))?;
        Ok(glb)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// points3D.bin → GLB
// ─────────────────────────────────────────────────────────────────────────────

fn points3d_bin_to_glb(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut cursor = std::io::Cursor::new(data);
    let num_points = read_u64(&mut cursor)? as usize;

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(num_points);
    let mut colors: Vec<[u8; 3]> = Vec::with_capacity(num_points);

    for _ in 0..num_points {
        let _id = read_u64(&mut cursor)?;
        let x = read_f64(&mut cursor)? as f32;
        let y = read_f64(&mut cursor)? as f32;
        let z = read_f64(&mut cursor)? as f32;
        let r = read_u8(&mut cursor)?;
        let g = read_u8(&mut cursor)?;
        let b = read_u8(&mut cursor)?;
        let _error = read_f64(&mut cursor)?;
        let track_length = read_u64(&mut cursor)? as usize;
        skip_bytes(&mut cursor, track_length * 8)?;

        positions.push([x, y, z]);
        colors.push([r, g, b]);
    }

    crate::ply_to_glb::points_to_glb(&positions, Some(&colors))
}

// ─────────────────────────────────────────────────────────────────────────────
// Binary reading helpers
// ─────────────────────────────────────────────────────────────────────────────

use std::io::Read;

fn read_u8(cursor: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<u8> {
    let mut buf = [0u8; 1];
    cursor.read_exact(&mut buf).map_err(|e| anyhow::anyhow!("EOF reading u8: {}", e))?;
    Ok(buf[0])
}

fn read_u64(cursor: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<u64> {
    let mut buf = [0u8; 8];
    cursor.read_exact(&mut buf).map_err(|e| anyhow::anyhow!("EOF reading u64: {}", e))?;
    Ok(u64::from_le_bytes(buf))
}

fn read_f64(cursor: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<f64> {
    let mut buf = [0u8; 8];
    cursor.read_exact(&mut buf).map_err(|e| anyhow::anyhow!("EOF reading f64: {}", e))?;
    Ok(f64::from_le_bytes(buf))
}

fn skip_bytes(cursor: &mut std::io::Cursor<&[u8]>, n: usize) -> anyhow::Result<()> {
    let pos = cursor.position() as usize;
    let new_pos = pos + n;
    if new_pos > cursor.get_ref().len() {
        return Err(anyhow::anyhow!("EOF while skipping {} bytes", n));
    }
    cursor.set_position(new_pos as u64);
    Ok(())
}
