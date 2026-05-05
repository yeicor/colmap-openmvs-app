//! Output viewer helpers – serves project output files in a viewer-friendly format.
//!
//! * `.ply` files are returned as-is.
//! * COLMAP `points3D.bin` files are converted to ASCII PLY point-clouds on the fly.

use dioxus::Result as DioxusResult;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Return `relative_path` from the project directory in a format suitable for
/// the 3-D viewer (Three.js PLYLoader).
///
/// * PLY files  → passed through unchanged.
/// * `points3D.bin` → converted to an ASCII PLY point-cloud.
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

    // Sanitise path – strip leading slashes, reject traversal.
    let sanitised = relative_path.trim_start_matches('/');
    for component in std::path::Path::new(sanitised).components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(anyhow::anyhow!("Path traversal is not allowed").into());
        }
    }

    let full_path = std::path::Path::new(&project_path).join(sanitised);
    let file_name = full_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    if file_name == "points3d.bin" {
        let bytes = tokio::fs::read(&full_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read points3D.bin: {}", e))?;
        let ply = convert_points3d_bin_to_ply(&bytes)
            .map_err(|e| anyhow::anyhow!("Failed to convert points3D.bin: {}", e))?;
        Ok(ply)
    } else {
        // PLY or anything else – pass through.
        let bytes = tokio::fs::read(&full_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read output file: {}", e))?;
        Ok(bytes)
    }
}

// ---------------------------------------------------------------------------
// points3D.bin → ASCII PLY converter
// ---------------------------------------------------------------------------

/// COLMAP `points3D.bin` binary layout (little-endian):
///
/// ```text
/// uint64  num_points3D
/// for each point:
///   uint64  point3D_id
///   f64     x, y, z
///   uint8   r, g, b
///   f64     error
///   uint64  track_length
///   for each track element:
///     uint32  image_id
///     uint32  point2D_idx
/// ```
fn convert_points3d_bin_to_ply(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut cursor = std::io::Cursor::new(data);

    let num_points = read_u64(&mut cursor)? as usize;

    // Collect points first so we know the count for the PLY header.
    let mut points: Vec<(f64, f64, f64, u8, u8, u8)> = Vec::with_capacity(num_points);

    for _ in 0..num_points {
        let _id = read_u64(&mut cursor)?;
        let x = read_f64(&mut cursor)?;
        let y = read_f64(&mut cursor)?;
        let z = read_f64(&mut cursor)?;
        let r = read_u8(&mut cursor)?;
        let g = read_u8(&mut cursor)?;
        let b = read_u8(&mut cursor)?;
        let _error = read_f64(&mut cursor)?;
        let track_length = read_u64(&mut cursor)? as usize;
        // Skip the track entries (image_id u32 + point2D_idx u32 each).
        skip_bytes(&mut cursor, track_length * 8)?;

        points.push((x, y, z, r, g, b));
    }

    // Build ASCII PLY.
    let mut out = String::with_capacity(256 + points.len() * 40);
    out.push_str("ply\n");
    out.push_str("format ascii 1.0\n");
    out.push_str(&format!("element vertex {}\n", points.len()));
    out.push_str("property float x\n");
    out.push_str("property float y\n");
    out.push_str("property float z\n");
    out.push_str("property uchar red\n");
    out.push_str("property uchar green\n");
    out.push_str("property uchar blue\n");
    out.push_str("end_header\n");

    for (x, y, z, r, g, b) in &points {
        out.push_str(&format!("{:.6} {:.6} {:.6} {} {} {}\n", x, y, z, r, g, b));
    }

    Ok(out.into_bytes())
}

// ---------------------------------------------------------------------------
// Binary reading helpers
// ---------------------------------------------------------------------------

use std::io::Read;

fn read_u8(cursor: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<u8> {
    let mut buf = [0u8; 1];
    cursor
        .read_exact(&mut buf)
        .map_err(|e| anyhow::anyhow!("Unexpected end of file reading u8: {}", e))?;
    Ok(buf[0])
}

fn read_u64(cursor: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<u64> {
    let mut buf = [0u8; 8];
    cursor
        .read_exact(&mut buf)
        .map_err(|e| anyhow::anyhow!("Unexpected end of file reading u64: {}", e))?;
    Ok(u64::from_le_bytes(buf))
}

fn read_f64(cursor: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<f64> {
    let mut buf = [0u8; 8];
    cursor
        .read_exact(&mut buf)
        .map_err(|e| anyhow::anyhow!("Unexpected end of file reading f64: {}", e))?;
    Ok(f64::from_le_bytes(buf))
}

fn skip_bytes(cursor: &mut std::io::Cursor<&[u8]>, n: usize) -> anyhow::Result<()> {
    let pos = cursor.position() as usize;
    let new_pos = pos + n;
    if new_pos > cursor.get_ref().len() {
        return Err(anyhow::anyhow!(
            "Unexpected end of file while skipping {} bytes",
            n
        ));
    }
    cursor.set_position(new_pos as u64);
    Ok(())
}
