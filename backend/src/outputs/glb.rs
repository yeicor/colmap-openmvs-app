//! PLY/points3D.bin → GLB (binary glTF 2.0) converter.
//!
//! Handles the variants produced by COLMAP and OpenMVS:
//!   - Point cloud (xyz + optional rgb)
//!   - Indexed triangle mesh (xyz + optional rgb, face vertex_indices)
//!   - Textured mesh  (xyz, face vertex_indices + per-face texcoord, companion PNG)
//!   - COLMAP points3D.bin  → GLB point cloud
//!
//! Provides [`generate_glb`] for server-side GLB generation.

use anyhow::anyhow;
use std::io::Read;

// ── Public API ────────────────────────────────────────────────────────────────

/// Extract all `comment TextureFile <name>` entries from the PLY header.
pub fn ply_texture_file_names(ply_bytes: &[u8]) -> Vec<String> {
    let chunk = &ply_bytes[..ply_bytes.len().min(8192)];
    let text = String::from_utf8_lossy(chunk);
    let mut names = Vec::new();
    for line in text.lines() {
        if line == "end_header" {
            break;
        }
        if let Some(name) = line.strip_prefix("comment TextureFile ") {
            names.push(name.trim().to_string());
        }
    }
    if !names.is_empty() {
        tracing::info!("ply_texture_file_names: found textures {:?}", names);
    }
    names
}

/// Generate a GLB file on disk by reading a PLY or points3D.bin output file.
///
/// * `file_name` — relative path of the file within the project
///   (e.g. `"openmvs/scene_mesh.ply"` or `"colmap/dense/sparse/points3D.bin"`).
/// * `project_path` — absolute path to the project root directory.
pub fn generate_glb(file_name: &str, project_path: &std::path::Path) -> anyhow::Result<Vec<u8>> {
    let full_path = project_path.join(file_name);
    let raw_bytes = std::fs::read(&full_path)
        .map_err(|e| anyhow!("Failed to read file '{}': {}", full_path.display(), e))?;

    // Special case: points3D.bin is a COLMAP binary points file, not PLY.
    let lower = file_name.to_lowercase();
    if lower == "points3d.bin"
        || lower.ends_with("/points3d.bin")
        || lower.ends_with("\\points3d.bin")
    {
        let (positions, colors) = parse_points3d_bin(&raw_bytes)?;
        let parsed = ParsedPly {
            positions,
            normals_from_file: vec![],
            colors: colors.unwrap_or_default(),
            face_indices: vec![],
            face_uvs: vec![],
            texture_file: None,
            texture_groups: vec![],
        };
        let glb = build_glb(parsed, vec![])?;
        tracing::info!(
            "generate_glb: converted points3D.bin to GLB ({} bytes)",
            glb.len()
        );
        return Ok(glb);
    }

    // Find companion textures (*.png in same directory as the PLY)
    let parent_dir = full_path.parent().unwrap_or(project_path);
    let mut textures_by_name = std::collections::HashMap::new();
    if let Ok(entries) = std::fs::read_dir(parent_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "png" {
                    if let Ok(bytes) = std::fs::read(&path) {
                        let name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        textures_by_name.insert(name.clone(), (name, bytes));
                    }
                }
            }
        }
    }
    // Build textures list in the order specified by PLY header comment TextureFile lines.
    let header_names = ply_texture_file_names(&raw_bytes);
    let mut textures: Vec<(String, Vec<u8>)> = Vec::new();
    if header_names.is_empty() {
        let mut all: Vec<(String, Vec<u8>)> = textures_by_name.into_values().collect();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        textures = all;
    } else {
        for hdr_name in &header_names {
            if let Some(entry) = textures_by_name.remove(hdr_name) {
                textures.push(entry);
            } else {
                tracing::warn!(
                    "generate_glb: PLY header references texture '{}' not found on disk",
                    hdr_name
                );
            }
        }
        let mut remaining: Vec<(String, Vec<u8>)> = textures_by_name.into_values().collect();
        remaining.sort_by(|a, b| a.0.cmp(&b.0));
        textures.extend(remaining);
    }
    tracing::info!(
        "generate_glb: found {} textures in PLY header order: {:?}",
        textures.len(),
        textures.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>()
    );

    // Build the GLB from the PLY
    let parsed = parse_ply(&raw_bytes)?;
    let glb = build_glb(parsed, textures)?;

    Ok(glb)
}

// ── points3D.bin → GLB ────────────────────────────────────────────────────────

fn parse_points3d_bin(data: &[u8]) -> anyhow::Result<(Vec<[f32; 3]>, Option<Vec<[u8; 3]>>)> {
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

    Ok((positions, Some(colors)))
}

fn read_u8(cursor: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<u8> {
    let mut buf = [0u8; 1];
    cursor
        .read_exact(&mut buf)
        .map_err(|e| anyhow!("EOF reading u8: {}", e))?;
    Ok(buf[0])
}

fn read_u64(cursor: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<u64> {
    let mut buf = [0u8; 8];
    cursor
        .read_exact(&mut buf)
        .map_err(|e| anyhow!("EOF reading u64: {}", e))?;
    Ok(u64::from_le_bytes(buf))
}

fn read_f64(cursor: &mut std::io::Cursor<&[u8]>) -> anyhow::Result<f64> {
    let mut buf = [0u8; 8];
    cursor
        .read_exact(&mut buf)
        .map_err(|e| anyhow!("EOF reading f64: {}", e))?;
    Ok(f64::from_le_bytes(buf))
}

fn skip_bytes(cursor: &mut std::io::Cursor<&[u8]>, n: usize) -> anyhow::Result<()> {
    let pos = cursor.position() as usize;
    let new_pos = pos + n;
    if new_pos > cursor.get_ref().len() {
        return Err(anyhow!("EOF while skipping {} bytes", n));
    }
    cursor.set_position(new_pos as u64);
    Ok(())
}

// ── Internal types ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum Fmt {
    BinaryLE,
    BinaryBE,
    Ascii,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Scalar {
    Char,
    UChar,
    Short,
    UShort,
    Int,
    UInt,
    Float,
    Double,
}

impl Scalar {
    fn size(self) -> usize {
        match self {
            Scalar::Char | Scalar::UChar => 1,
            Scalar::Short | Scalar::UShort => 2,
            Scalar::Int | Scalar::UInt | Scalar::Float => 4,
            Scalar::Double => 8,
        }
    }
    fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "char" | "int8" => Scalar::Char,
            "uchar" | "uint8" => Scalar::UChar,
            "short" | "int16" => Scalar::Short,
            "ushort" | "uint16" => Scalar::UShort,
            "int" | "int32" => Scalar::Int,
            "uint" | "uint32" => Scalar::UInt,
            "float" | "float32" => Scalar::Float,
            "double" | "float64" => Scalar::Double,
            _ => return None,
        })
    }
}

#[derive(Debug)]
enum PropDef {
    Scalar(Scalar),
    List { cnt: Scalar, val: Scalar },
}

#[derive(Debug)]
struct Prop {
    name: String,
    def: PropDef,
}

#[derive(Debug)]
struct ElemDef {
    name: String,
    count: usize,
    props: Vec<Prop>,
}

struct Header {
    fmt: Fmt,
    texture_file: Option<String>,
    elems: Vec<ElemDef>,
    data_offset: usize,
}

#[derive(Default)]
pub struct ParsedPly {
    pub positions: Vec<[f32; 3]>,
    pub normals_from_file: Vec<[f32; 3]>,
    pub colors: Vec<[u8; 3]>,
    pub face_indices: Vec<u32>,
    pub face_uvs: Vec<[f32; 2]>,
    #[allow(dead_code)]
    pub texture_file: Option<String>,
    /// Per-texture groups for multi-texture support.
    /// Each entry holds the face indices and UVs that reference that texture.
    pub texture_groups: Vec<TextureGroup>,
}

/// A group of faces sharing a single texture (identified by texnumber).
#[derive(Default, Clone)]
pub struct TextureGroup {
    pub face_indices: Vec<u32>,
    pub face_uvs: Vec<[f32; 2]>,
    #[allow(dead_code)]
    pub texture_file: Option<String>,
    pub texture_bytes: Option<Vec<u8>>,
}

// ── Header parser ─────────────────────────────────────────────────────────────

fn parse_header(bytes: &[u8]) -> anyhow::Result<Header> {
    let end_marker = b"end_header\n";
    let end_pos = bytes
        .windows(end_marker.len())
        .position(|w| w == end_marker)
        .ok_or_else(|| anyhow!("PLY: missing end_header"))?;
    let data_offset = end_pos + end_marker.len();

    let header_text = std::str::from_utf8(&bytes[..end_pos])
        .map_err(|_| anyhow!("PLY: header is not valid UTF-8"))?;

    let mut fmt = None;
    let mut texture_file = None;
    let mut elems: Vec<ElemDef> = vec![];

    for raw_line in header_text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line == "ply" {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        match tokens.as_slice() {
            ["format", name, _ver] => {
                fmt = Some(match *name {
                    "binary_little_endian" => Fmt::BinaryLE,
                    "binary_big_endian" => Fmt::BinaryBE,
                    "ascii" => Fmt::Ascii,
                    other => return Err(anyhow!("PLY: unknown format '{}'", other)),
                });
            }
            ["comment", "TextureFile", name] => {
                texture_file = Some((*name).to_string());
            }
            ["comment", ..] => {}
            ["element", name, count_str] => {
                let count = count_str
                    .parse::<usize>()
                    .map_err(|_| anyhow!("PLY: bad element count"))?;
                elems.push(ElemDef {
                    name: (*name).to_string(),
                    count,
                    props: vec![],
                });
            }
            ["property", "list", cnt_type, val_type, prop_name] => {
                let cnt = Scalar::from_name(cnt_type)
                    .ok_or_else(|| anyhow!("PLY: unknown scalar type '{}'", cnt_type))?;
                let val = Scalar::from_name(val_type)
                    .ok_or_else(|| anyhow!("PLY: unknown scalar type '{}'", val_type))?;
                if let Some(elem) = elems.last_mut() {
                    elem.props.push(Prop {
                        name: (*prop_name).to_string(),
                        def: PropDef::List { cnt, val },
                    });
                }
            }
            ["property", type_name, prop_name] => {
                let s = Scalar::from_name(type_name)
                    .ok_or_else(|| anyhow!("PLY: unknown scalar type '{}'", type_name))?;
                if let Some(elem) = elems.last_mut() {
                    elem.props.push(Prop {
                        name: (*prop_name).to_string(),
                        def: PropDef::Scalar(s),
                    });
                }
            }
            _ => {}
        }
    }

    Ok(Header {
        fmt: fmt.ok_or_else(|| anyhow!("PLY: no format line"))?,
        texture_file,
        elems,
        data_offset,
    })
}

// ── Binary reader ─────────────────────────────────────────────────────────────

struct BinReader<'a> {
    buf: &'a [u8],
    pos: usize,
    le: bool,
}

impl<'a> BinReader<'a> {
    fn new(buf: &'a [u8], le: bool) -> Self {
        BinReader { buf, pos: 0, le }
    }

    fn read_u8(&mut self) -> anyhow::Result<u8> {
        if self.pos >= self.buf.len() {
            return Err(anyhow!("PLY binary: unexpected EOF"));
        }
        let v = self.buf[self.pos];
        self.pos += 1;
        Ok(v)
    }

    fn read_n<const N: usize>(&mut self) -> anyhow::Result<[u8; N]> {
        if self.pos + N > self.buf.len() {
            return Err(anyhow!("PLY binary: unexpected EOF"));
        }
        let mut arr = [0u8; N];
        arr.copy_from_slice(&self.buf[self.pos..self.pos + N]);
        self.pos += N;
        Ok(arr)
    }

    fn read_f64(&mut self) -> anyhow::Result<f64> {
        let b = self.read_n::<8>()?;
        Ok(if self.le {
            f64::from_le_bytes(b)
        } else {
            f64::from_be_bytes(b)
        })
    }

    fn read_scalar_f32(&mut self, s: Scalar) -> anyhow::Result<f32> {
        Ok(match s {
            Scalar::Char => self.read_u8()? as i8 as f32,
            Scalar::UChar => self.read_u8()? as f32,
            Scalar::Short => {
                let b = self.read_n::<2>()?;
                (if self.le {
                    i16::from_le_bytes(b)
                } else {
                    i16::from_be_bytes(b)
                }) as f32
            }
            Scalar::UShort => {
                let b = self.read_n::<2>()?;
                (if self.le {
                    u16::from_le_bytes(b)
                } else {
                    u16::from_be_bytes(b)
                }) as f32
            }
            Scalar::Int => {
                let b = self.read_n::<4>()?;
                (if self.le {
                    i32::from_le_bytes(b)
                } else {
                    i32::from_be_bytes(b)
                }) as f32
            }
            Scalar::UInt => {
                let b = self.read_n::<4>()?;
                (if self.le {
                    u32::from_le_bytes(b)
                } else {
                    u32::from_be_bytes(b)
                }) as f32
            }
            Scalar::Float => {
                let b = self.read_n::<4>()?;
                f32::from_bits(if self.le {
                    u32::from_le_bytes(b)
                } else {
                    u32::from_be_bytes(b)
                })
            }
            Scalar::Double => self.read_f64()? as f32,
        })
    }

    fn read_scalar_u32(&mut self, s: Scalar) -> anyhow::Result<u32> {
        Ok(self.read_scalar_f32(s)? as u32)
    }

    fn read_scalar_u8(&mut self, s: Scalar) -> anyhow::Result<u8> {
        if let Scalar::UChar = s {
            return self.read_u8();
        }
        // For float/double colors (range [0,1]), convert to [0,255]
        let f = self.read_scalar_f32(s)?;
        if f >= 0.0 && f <= 1.0 && (s == Scalar::Float || s == Scalar::Double) {
            Ok((f * 255.0).round().max(0.0).min(255.0) as u8)
        } else {
            Ok(f as u8)
        }
    }

    fn skip(&mut self, s: Scalar) -> anyhow::Result<()> {
        let n = s.size();
        if self.pos + n > self.buf.len() {
            return Err(anyhow!("PLY binary: unexpected EOF skipping bytes"));
        }
        self.pos += n;
        Ok(())
    }
}

// ── PLY parser dispatch ───────────────────────────────────────────────────────

pub fn parse_ply(bytes: &[u8]) -> anyhow::Result<ParsedPly> {
    let hdr = parse_header(bytes)?;
    let data = &bytes[hdr.data_offset..];
    let mut out = ParsedPly {
        texture_file: hdr.texture_file.clone(),
        ..Default::default()
    };
    match hdr.fmt {
        Fmt::BinaryLE => parse_binary(&hdr, data, true, &mut out)?,
        Fmt::BinaryBE => parse_binary(&hdr, data, false, &mut out)?,
        Fmt::Ascii => parse_ascii(&hdr, data, &mut out)?,
    }

    // Populate texture_file names in texture_groups from header comments
    let tex_names = ply_texture_file_names(bytes);
    if !tex_names.is_empty() && !out.texture_groups.is_empty() {
        for (i, group) in out.texture_groups.iter_mut().enumerate() {
            if let Some(name) = tex_names.get(i) {
                group.texture_file = Some(name.clone());
            }
        }
    }

    Ok(out)
}

// ── Binary parser ─────────────────────────────────────────────────────────────

fn parse_binary(hdr: &Header, data: &[u8], le: bool, out: &mut ParsedPly) -> anyhow::Result<()> {
    let mut r = BinReader::new(data, le);
    for elem in &hdr.elems {
        match elem.name.as_str() {
            "vertex" => read_vertices(elem, &mut r, out)?,
            "face" => read_faces(elem, &mut r, out)?,
            _ => skip_element(elem, &mut r)?,
        }
    }
    Ok(())
}

fn prop_idx<'a>(elem: &'a ElemDef, names: &[&str]) -> Option<(usize, &'a Prop)> {
    elem.props
        .iter()
        .enumerate()
        .find(|(_, p)| names.contains(&p.name.as_str()))
}

fn read_vertices(elem: &ElemDef, r: &mut BinReader, out: &mut ParsedPly) -> anyhow::Result<()> {
    let xi = prop_idx(elem, &["x"]);
    let yi = prop_idx(elem, &["y"]);
    let zi = prop_idx(elem, &["z"]);
    let nxi = prop_idx(elem, &["nx"]);
    let nyi = prop_idx(elem, &["ny"]);
    let nzi = prop_idx(elem, &["nz"]);
    let ri = prop_idx(elem, &["red", "r", "diffuse_red"]);
    let gi = prop_idx(elem, &["green", "g", "diffuse_green"]);
    let bi = prop_idx(elem, &["blue", "b", "diffuse_blue"]);

    let has_pos = xi.is_some() && yi.is_some() && zi.is_some();
    let has_nrm = nxi.is_some() && nyi.is_some() && nzi.is_some();
    let has_col = ri.is_some() && gi.is_some() && bi.is_some();

    out.positions.reserve(elem.count);
    if has_nrm {
        out.normals_from_file.reserve(elem.count);
    }
    if has_col {
        out.colors.reserve(elem.count);
    }

    for _ in 0..elem.count {
        let mut pos = [0f32; 3];
        let mut nrm = [0f32; 3];
        let mut col = [0u8; 3];

        for (pi, prop) in elem.props.iter().enumerate() {
            match &prop.def {
                PropDef::Scalar(s) => {
                    let is_x = xi.as_ref().map(|(i, _)| *i) == Some(pi);
                    let is_y = yi.as_ref().map(|(i, _)| *i) == Some(pi);
                    let is_z = zi.as_ref().map(|(i, _)| *i) == Some(pi);
                    let is_nx = nxi.as_ref().map(|(i, _)| *i) == Some(pi);
                    let is_ny = nyi.as_ref().map(|(i, _)| *i) == Some(pi);
                    let is_nz = nzi.as_ref().map(|(i, _)| *i) == Some(pi);
                    let is_r = ri.as_ref().map(|(i, _)| *i) == Some(pi);
                    let is_g = gi.as_ref().map(|(i, _)| *i) == Some(pi);
                    let is_b = bi.as_ref().map(|(i, _)| *i) == Some(pi);

                    if is_x {
                        pos[0] = r.read_scalar_f32(*s)?;
                    } else if is_y {
                        pos[1] = r.read_scalar_f32(*s)?;
                    } else if is_z {
                        pos[2] = r.read_scalar_f32(*s)?;
                    } else if is_nx {
                        nrm[0] = r.read_scalar_f32(*s)?;
                    } else if is_ny {
                        nrm[1] = r.read_scalar_f32(*s)?;
                    } else if is_nz {
                        nrm[2] = r.read_scalar_f32(*s)?;
                    } else if is_r {
                        col[0] = r.read_scalar_u8(*s)?;
                    } else if is_g {
                        col[1] = r.read_scalar_u8(*s)?;
                    } else if is_b {
                        col[2] = r.read_scalar_u8(*s)?;
                    } else {
                        r.skip(*s)?;
                    }
                }
                PropDef::List { cnt, val } => {
                    let n = r.read_scalar_u32(*cnt)? as usize;
                    for _ in 0..n {
                        r.skip(*val)?;
                    }
                }
            }
        }

        if has_pos {
            out.positions.push(pos);
        }
        if has_nrm {
            out.normals_from_file.push(nrm);
        }
        if has_col {
            out.colors.push(col);
        }
    }
    Ok(())
}

fn read_faces(elem: &ElemDef, r: &mut BinReader, out: &mut ParsedPly) -> anyhow::Result<()> {
    let vi_idx = elem
        .props
        .iter()
        .position(|p| p.name == "vertex_indices" || p.name == "vertex_index");
    let uv_idx = elem
        .props
        .iter()
        .position(|p| p.name == "texcoord" || p.name == "tcoord" || p.name == "texture_u");
    let texnum_idx = elem.props.iter().position(|p| p.name == "texnumber");

    // Group faces by texnumber for multi-texture support.
    // groups[0] = texnumber 0 (or no texnumber), groups[1] = texnumber 1, etc.
    struct FaceGroup {
        indices: Vec<u32>,
        uvs: Vec<[f32; 2]>,
        has_uvs: bool,
    }
    let mut groups: Vec<FaceGroup> = Vec::new();

    for _ in 0..elem.count {
        let mut verts: Vec<u32> = vec![];
        let mut uvs: Vec<[f32; 2]> = vec![];
        let mut face_texnum: Option<i32> = None;

        for (pi, prop) in elem.props.iter().enumerate() {
            match &prop.def {
                PropDef::List { cnt, val } => {
                    let n = r.read_scalar_u32(*cnt)? as usize;
                    if Some(pi) == vi_idx {
                        for _ in 0..n {
                            verts.push(r.read_scalar_u32(*val)?);
                        }
                    } else if Some(pi) == uv_idx {
                        for _ in 0..n / 2 {
                            let u = r.read_scalar_f32(*val)?;
                            let v = r.read_scalar_f32(*val)?;
                            uvs.push([u, v]);
                        }
                    } else if Some(pi) == texnum_idx && n > 0 {
                        face_texnum = Some(r.read_scalar_u32(*val)? as i32);
                        for _ in 1..n {
                            r.skip(*val)?;
                        }
                    } else {
                        for _ in 0..n {
                            r.skip(*val)?;
                        }
                    }
                }
                PropDef::Scalar(s) => {
                    // Handle texnumber as a scalar property (property int texnumber).
                    // Must read (not skip) to consume the bytes from the stream.
                    if Some(pi) == texnum_idx {
                        face_texnum = Some(r.read_scalar_u32(*s)? as i32);
                    } else {
                        r.skip(*s)?;
                    }
                }
            }
        }

        let texnum = face_texnum.unwrap_or(0) as usize;
        while groups.len() <= texnum {
            groups.push(FaceGroup {
                indices: vec![],
                uvs: vec![],
                has_uvs: false,
            });
        }

        let nv = verts.len();
        for j in 1..nv.saturating_sub(1) {
            groups[texnum].indices.push(verts[0]);
            groups[texnum].indices.push(verts[j]);
            groups[texnum].indices.push(verts[j + 1]);
            if !uvs.is_empty() {
                groups[texnum].uvs.push(uvs[0]);
                groups[texnum].uvs.push(uvs[j]);
                groups[texnum].uvs.push(uvs[j + 1]);
                groups[texnum].has_uvs = true;
            }
        }
    }

    // Always populate texture_groups when any face group has UVs, so that ALL
    // textured meshes go through the same multi-texture code path (including
    // single-texture PLY without an explicit texnumber property).
    // Groups without UVs are still flattened for legacy non-textured paths.
    let has_any_uvs = groups.iter().any(|g| g.has_uvs);
    if has_any_uvs {
        tracing::info!(
            "read_faces: {} texture groups ({} faces total), has_any_uvs={}",
            groups.len(),
            groups.iter().map(|g| g.indices.len()).sum::<usize>(),
            has_any_uvs
        );
        for (i, g) in groups.iter().enumerate() {
            if !g.indices.is_empty() {
                tracing::info!(
                    "  group[{}]: {} vertex indices, {} UVs",
                    i,
                    g.indices.len(),
                    g.uvs.len()
                );
            }
        }
        out.texture_groups = groups
            .into_iter()
            .map(|g| TextureGroup {
                face_indices: g.indices,
                face_uvs: g.uvs,
                texture_file: None,
                texture_bytes: None,
            })
            .collect();
    } else {
        // No UVs — flatten all indices for the non-textured code path.
        tracing::info!(
            "read_faces: {} groups, no UVs, flattening ({} indices)",
            groups.len(),
            groups.iter().map(|g| g.indices.len()).sum::<usize>()
        );
        for group in &groups {
            out.face_indices.extend(&group.indices);
            out.face_uvs.extend(&group.uvs);
        }
    }

    Ok(())
}

fn skip_element(elem: &ElemDef, r: &mut BinReader) -> anyhow::Result<()> {
    for _ in 0..elem.count {
        for prop in &elem.props {
            match &prop.def {
                PropDef::Scalar(s) => r.skip(*s)?,
                PropDef::List { cnt, val } => {
                    let n = r.read_scalar_u32(*cnt)? as usize;
                    for _ in 0..n {
                        r.skip(*val)?;
                    }
                }
            }
        }
    }
    Ok(())
}

// ── ASCII parser ──────────────────────────────────────────────────────────────

fn parse_ascii(hdr: &Header, data: &[u8], out: &mut ParsedPly) -> anyhow::Result<()> {
    let text = std::str::from_utf8(data).map_err(|_| anyhow!("PLY ASCII: invalid UTF-8"))?;
    let mut lines = text.lines();

    for elem in &hdr.elems {
        match elem.name.as_str() {
            "vertex" => {
                let xi = prop_idx(elem, &["x"]).map(|(i, _)| i);
                let yi = prop_idx(elem, &["y"]).map(|(i, _)| i);
                let zi = prop_idx(elem, &["z"]).map(|(i, _)| i);
                let ri = prop_idx(elem, &["red", "r", "diffuse_red"]).map(|(i, _)| i);
                let gi = prop_idx(elem, &["green", "g", "diffuse_green"]).map(|(i, _)| i);
                let bi = prop_idx(elem, &["blue", "b", "diffuse_blue"]).map(|(i, _)| i);
                let has_col = ri.is_some() && gi.is_some() && bi.is_some();

                for _ in 0..elem.count {
                    let line = lines.next().unwrap_or("");
                    let toks: Vec<&str> = line.split_whitespace().collect();
                    let mut flat: Vec<f64> = vec![];
                    let mut ti = 0usize;
                    for prop in &elem.props {
                        match &prop.def {
                            PropDef::Scalar(_) => {
                                flat.push(toks.get(ti).and_then(|s| s.parse().ok()).unwrap_or(0.0));
                                ti += 1;
                            }
                            PropDef::List { .. } => {
                                let cnt = toks
                                    .get(ti)
                                    .and_then(|s| s.parse::<usize>().ok())
                                    .unwrap_or(0);
                                ti += 1 + cnt;
                                flat.push(0.0);
                            }
                        }
                    }
                    let get = |idx: Option<usize>| -> f32 {
                        idx.and_then(|i| flat.get(i)).copied().unwrap_or(0.0) as f32
                    };
                    out.positions.push([get(xi), get(yi), get(zi)]);
                    if has_col {
                        let to_u8 = |v: f32| -> u8 {
                            if v >= 0.0 && v <= 1.0 && v.fract() != 0.0 {
                                (v * 255.0).round().max(0.0).min(255.0) as u8
                            } else {
                                v as u8
                            }
                        };
                        out.colors
                            .push([to_u8(get(ri)), to_u8(get(gi)), to_u8(get(bi))]);
                    }
                }
            }
            "face" => {
                let vi_pi = elem
                    .props
                    .iter()
                    .position(|p| p.name == "vertex_indices" || p.name == "vertex_index");
                for _ in 0..elem.count {
                    let line = lines.next().unwrap_or("");
                    let toks: Vec<&str> = line.split_whitespace().collect();
                    if let Some(vi) = vi_pi {
                        let scalar_before = elem.props[..vi]
                            .iter()
                            .filter(|p| matches!(p.def, PropDef::Scalar(_)))
                            .count();
                        let base = scalar_before;
                        let cnt = toks
                            .get(base)
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(0);
                        let mut verts = vec![];
                        for k in 0..cnt {
                            let v = toks
                                .get(base + 1 + k)
                                .and_then(|s| s.parse::<u32>().ok())
                                .unwrap_or(0);
                            verts.push(v);
                        }
                        for j in 1..verts.len().saturating_sub(1) {
                            out.face_indices.push(verts[0]);
                            out.face_indices.push(verts[j]);
                            out.face_indices.push(verts[j + 1]);
                        }
                    }
                }
            }
            _ => {
                for _ in 0..elem.count {
                    lines.next();
                }
            }
        }
    }
    Ok(())
}

// ── Normal computation ────────────────────────────────────────────────────────

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn normalize(n: [f32; 3]) -> [f32; 3] {
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len > 1e-10 {
        [n[0] / len, n[1] / len, n[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn face_normals_expanded(positions: &[[f32; 3]], face_indices: &[u32]) -> Vec<[f32; 3]> {
    let mut out = vec![[0f32; 3]; face_indices.len()];
    for fi in 0..face_indices.len() / 3 {
        let p0 = positions[face_indices[fi * 3] as usize];
        let p1 = positions[face_indices[fi * 3 + 1] as usize];
        let p2 = positions[face_indices[fi * 3 + 2] as usize];
        let n = normalize(cross(sub(p1, p0), sub(p2, p0)));
        out[fi * 3] = n;
        out[fi * 3 + 1] = n;
        out[fi * 3 + 2] = n;
    }
    out
}

fn vertex_normals(positions: &[[f32; 3]], face_indices: &[u32]) -> Vec<[f32; 3]> {
    let mut out = vec![[0f32; 3]; positions.len()];
    for fi in 0..face_indices.len() / 3 {
        let ia = face_indices[fi * 3] as usize;
        let ib = face_indices[fi * 3 + 1] as usize;
        let ic = face_indices[fi * 3 + 2] as usize;
        let n = cross(
            sub(positions[ib], positions[ia]),
            sub(positions[ic], positions[ia]),
        );
        for &vi in &[ia, ib, ic] {
            out[vi][0] += n[0];
            out[vi][1] += n[1];
            out[vi][2] += n[2];
        }
    }
    for n in &mut out {
        *n = normalize(*n);
    }
    out
}

// ── GLB builder ───────────────────────────────────────────────────────────────

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

fn pad4(bin: &mut Vec<u8>, byte: u8) {
    while !bin.len().is_multiple_of(4) {
        bin.push(byte);
    }
}

fn push_f32(bin: &mut Vec<u8>, v: f32) {
    bin.extend_from_slice(&v.to_le_bytes());
}

fn push_u32(bin: &mut Vec<u8>, v: u32) {
    bin.extend_from_slice(&v.to_le_bytes());
}

struct BufSection {
    byte_offset: usize,
    byte_length: usize,
}

fn write_section(bin: &mut Vec<u8>, data: &[u8]) -> BufSection {
    pad4(bin, 0);
    let byte_offset = bin.len();
    bin.extend_from_slice(data);
    BufSection {
        byte_offset,
        byte_length: data.len(),
    }
}

/// Build the JSON string and binary buffer sections.
///
/// NOTE: Callers must normalize `parsed` before calling this:
/// if `texture_groups` exist but no `textures` are provided, the caller
/// should have already flattened `texture_groups` into `face_indices`.
pub fn build_glb_sections(
    parsed: &ParsedPly,
    textures: &[(String, Vec<u8>)],
) -> anyhow::Result<(String, Vec<u8>, usize, usize)> {
    let n_pos = parsed.positions.len();
    if n_pos == 0 {
        return Err(anyhow!("PLY has no vertices"));
    }

    // Determine which kind of data we have.
    let has_face_indices_from_groups = parsed
        .texture_groups
        .iter()
        .any(|g| !g.face_indices.is_empty());
    let has_face_indices = !parsed.face_indices.is_empty() || has_face_indices_from_groups;
    let is_point_cloud = !has_face_indices;
    let is_textured = has_face_indices_from_groups && !textures.is_empty();
    let has_colors = !parsed.colors.is_empty();

    // ═══════════════════════════════════════════════════════════════════════
    // TEXTURED PATH: one primitive per texture group
    // (handles both single-texture and multi-texture PLY files)
    // ═══════════════════════════════════════════════════════════════════════
    if is_textured {
        let mut bin: Vec<u8> = Vec::new();
        let mut buf_views: Vec<serde_json::Value> = vec![];
        let mut accessors: Vec<serde_json::Value> = vec![];
        let mut primitives: Vec<serde_json::Value> = vec![];
        let mut materials_list: Vec<serde_json::Value> = vec![];
        let mut textures_list: Vec<serde_json::Value> = vec![];
        let mut images_list: Vec<serde_json::Value> = vec![];

        // Single sampler shared by all texture images
        let samplers_list = vec![serde_json::json!({
            "magFilter": 9729,
            "minFilter": 9987,
            "wrapS": 10497,
            "wrapT": 10497,
        })];

        let add_accessor = |buf_views: &mut Vec<serde_json::Value>,
                            accessors: &mut Vec<serde_json::Value>,
                            sec: BufSection,
                            comp_type: u32,
                            gltf_type: &str,
                            count: usize,
                            target: Option<u32>,
                            normalized: bool,
                            min_max: Option<([f64; 3], [f64; 3])>| {
            let bv_idx = buf_views.len();
            let mut bv = serde_json::json!({
                "buffer": 0,
                "byteOffset": sec.byte_offset,
                "byteLength": sec.byte_length,
            });
            if let Some(t) = target {
                bv["target"] = serde_json::json!(t);
            }
            buf_views.push(bv);

            let acc_idx = accessors.len();
            let mut acc = serde_json::json!({
                "bufferView": bv_idx,
                "byteOffset": 0,
                "componentType": comp_type,
                "count": count,
                "type": gltf_type,
            });
            if normalized {
                acc["normalized"] = serde_json::json!(true);
            }
            if let Some((mn, mx)) = min_max {
                acc["min"] = serde_json::json!(mn.to_vec());
                acc["max"] = serde_json::json!(mx.to_vec());
            }
            accessors.push(acc);
            acc_idx
        };

        for (group_idx, group) in parsed.texture_groups.iter().enumerate() {
            if group.face_indices.is_empty() {
                continue;
            }

            // Resolve texture bytes for this group using PLY header texture file name.
            // Priority:
            //   1. Name-based lookup: match group.texture_file against textures list
            //   2. Index-based fallback: use group_idx as index into sorted textures
            //   3. group.texture_bytes (pre-embedded bytes)
            let png = if let Some(ref tex_file) = group.texture_file {
                let found = textures.iter().find(|(name, _)| name == tex_file);
                if let Some((name, bytes)) = found {
                    tracing::info!(
                        "group {}: resolved texture by name '{}' ({} bytes)",
                        group_idx,
                        name,
                        bytes.len()
                    );
                    bytes.clone()
                } else {
                    // Name not found, try index-based fallback
                    tracing::warn!(
                        "group {}: PLY header says '{}' not found on disk (have {:?}), trying index fallback",
                        group_idx, tex_file,
                        textures.iter().map(|(n,_)| n).collect::<Vec<_>>()
                    );
                    if group_idx < textures.len() {
                        let (name, bytes) = &textures[group_idx];
                        tracing::info!(
                            "group {}: using texture[{}] = '{}' ({} bytes) as fallback",
                            group_idx,
                            group_idx,
                            name,
                            bytes.len()
                        );
                        bytes.clone()
                    } else if let Some(ref bytes) = group.texture_bytes {
                        bytes.clone()
                    } else {
                        tracing::warn!("Texture group {} has no texture data available", group_idx);
                        vec![]
                    }
                }
            } else if group_idx < textures.len() {
                let (name, bytes) = &textures[group_idx];
                tracing::info!(
                    "group {}: no PLY header name, using texture[{}] = '{}' ({} bytes)",
                    group_idx,
                    group_idx,
                    name,
                    bytes.len()
                );
                bytes.clone()
            } else if let Some(ref bytes) = group.texture_bytes {
                bytes.clone()
            } else {
                tracing::warn!("Texture group {} has no texture data available", group_idx);
                vec![]
            };

            if png.is_empty() {
                continue;
            }

            let expanded_count = group.face_indices.len();

            // ── Per-group: Positions (expanded from face indices) ──────────
            let mut pos_bytes = vec![];
            let mut mn = [f32::MAX; 3];
            let mut mx = [f32::MIN; 3];
            for &fi in &group.face_indices {
                let p = parsed.positions[fi as usize];
                for k in 0..3 {
                    mn[k] = mn[k].min(p[k]);
                    mx[k] = mx[k].max(p[k]);
                    push_f32(&mut pos_bytes, p[k]);
                }
            }
            if mn[0] > mx[0] {
                mn = [0.0; 3];
                mx = [0.0; 3];
            }
            let sec = write_section(&mut bin, &pos_bytes);
            let mn_f64 = [mn[0] as f64, mn[1] as f64, mn[2] as f64];
            let mx_f64 = [mx[0] as f64, mx[1] as f64, mx[2] as f64];
            let pos_acc = add_accessor(
                &mut buf_views,
                &mut accessors,
                sec,
                5126,
                "VEC3",
                expanded_count,
                Some(34962),
                false,
                Some((mn_f64, mx_f64)),
            );

            let mut attr = serde_json::Map::new();
            attr.insert("POSITION".into(), serde_json::json!(pos_acc));

            // ── Per-group: Normals ─────────────────────────────────────────
            let normals: Vec<[f32; 3]> = if !parsed.normals_from_file.is_empty() {
                group
                    .face_indices
                    .iter()
                    .map(|&fi| parsed.normals_from_file[fi as usize])
                    .collect()
            } else {
                face_normals_expanded(&parsed.positions, &group.face_indices)
            };
            if !normals.is_empty() {
                let mut nrm_bytes = vec![];
                for n in &normals {
                    push_f32(&mut nrm_bytes, n[0]);
                    push_f32(&mut nrm_bytes, n[1]);
                    push_f32(&mut nrm_bytes, n[2]);
                }
                let sec = write_section(&mut bin, &nrm_bytes);
                let nrm_acc = add_accessor(
                    &mut buf_views,
                    &mut accessors,
                    sec,
                    5126,
                    "VEC3",
                    normals.len(),
                    Some(34962),
                    false,
                    None,
                );
                attr.insert("NORMAL".into(), serde_json::json!(nrm_acc));
            }

            // ── Per-group: UVs ────────────────────────────────────────────
            // All texture groups get V-flip (1.0 - v). OpenMVS texture atlases
            // use the image convention with V=0 at top, while GLTF expects
            // V=0 at bottom.
            if !group.face_uvs.is_empty() {
                let mut uv_bytes = vec![];
                for &uv in &group.face_uvs {
                    push_f32(&mut uv_bytes, uv[0]);
                    push_f32(&mut uv_bytes, 1.0 - uv[1]);
                }
                let sec = write_section(&mut bin, &uv_bytes);
                let uv_acc = add_accessor(
                    &mut buf_views,
                    &mut accessors,
                    sec,
                    5126,
                    "VEC2",
                    group.face_uvs.len(),
                    Some(34962),
                    false,
                    None,
                );
                attr.insert("TEXCOORD_0".into(), serde_json::json!(uv_acc));
            }

            // ── Per-group: Embed texture image ────────────────────────────
            pad4(&mut bin, 0);
            let img_bv_idx = buf_views.len();
            buf_views.push(serde_json::json!({
                "buffer": 0,
                "byteOffset": bin.len(),
                "byteLength": png.len(),
            }));
            bin.extend_from_slice(&png);

            let img_idx = images_list.len();
            images_list.push(serde_json::json!({
                "mimeType": "image/png",
                "bufferView": img_bv_idx,
            }));

            let tex_idx = textures_list.len();
            textures_list.push(serde_json::json!({
                "source": img_idx,
                "sampler": 0,
            }));

            let mat_idx = materials_list.len();
            materials_list.push(serde_json::json!({
                "name": format!("tex_mat_{}", mat_idx),
                "pbrMetallicRoughness": {
                    "baseColorTexture": {"index": tex_idx},
                    "metallicFactor": 0.0,
                    "roughnessFactor": 1.0,
                },
                "doubleSided": true,
            }));

            primitives.push(serde_json::json!({
                "mode": 4,
                "attributes": attr,
                "material": mat_idx,
            }));
        }

        if primitives.is_empty() {
            return Err(anyhow!("No textured primitives could be generated"));
        }

        pad4(&mut bin, 0);
        let bin_padded = align4(bin.len());

        // Create one mesh per primitive with corresponding nodes.
        // GLTF requires that each mesh with a distinct material/texture
        // be a separate mesh entry (not multiple primitives in one mesh)
        // for proper material routing in renderers like Three.js.
        let mut meshes: Vec<serde_json::Value> = Vec::new();
        let mut nodes: Vec<serde_json::Value> = Vec::new();
        for prim in &primitives {
            let mesh_idx = meshes.len();
            meshes.push(serde_json::json!({"primitives": [prim]}));
            nodes.push(serde_json::json!({"mesh": mesh_idx}));
        }
        let scene_nodes: Vec<usize> = (0..nodes.len()).collect();

        let gltf = serde_json::json!({
            "asset": {"version": "2.0", "generator": "colmap-openmvs-app"},
            "scene": 0,
            "scenes": [{"nodes": scene_nodes}],
            "nodes": nodes,
            "meshes": meshes,
            "materials": materials_list,
            "textures": textures_list,
            "images": images_list,
            "samplers": samplers_list,
            "accessors": accessors,
            "bufferViews": buf_views,
            "buffers": [{"byteLength": bin_padded}],
        });

        let json_str = serde_json::to_string(&gltf)?;
        let json_padded = align4(json_str.len());

        return Ok((json_str, bin, json_padded, bin_padded));
    }

    // ═══════════════════════════════════════════════════════════════════════
    // NON-TEXTURED PATH: point-cloud or non-textured indexed mesh
    // ═══════════════════════════════════════════════════════════════════════

    let mut bin: Vec<u8> = Vec::new();
    let mut buf_views: Vec<serde_json::Value> = vec![];
    let mut accessors: Vec<serde_json::Value> = vec![];
    let mut attr = serde_json::Map::new();
    let mut indices_acc: Option<usize> = None;

    let add_accessor = |buf_views: &mut Vec<serde_json::Value>,
                        accessors: &mut Vec<serde_json::Value>,
                        sec: BufSection,
                        comp_type: u32,
                        gltf_type: &str,
                        count: usize,
                        target: Option<u32>,
                        normalized: bool,
                        min_max: Option<([f64; 3], [f64; 3])>| {
        let bv_idx = buf_views.len();
        let mut bv = serde_json::json!({
            "buffer": 0,
            "byteOffset": sec.byte_offset,
            "byteLength": sec.byte_length,
        });
        if let Some(t) = target {
            bv["target"] = serde_json::json!(t);
        }
        buf_views.push(bv);

        let acc_idx = accessors.len();
        let mut acc = serde_json::json!({
            "bufferView": bv_idx,
            "byteOffset": 0,
            "componentType": comp_type,
            "count": count,
            "type": gltf_type,
        });
        if normalized {
            acc["normalized"] = serde_json::json!(true);
        }
        if let Some((mn, mx)) = min_max {
            acc["min"] = serde_json::json!(mn.to_vec());
            acc["max"] = serde_json::json!(mx.to_vec());
        }
        accessors.push(acc);
        acc_idx
    };

    // ── 1. Positions (direct vertex positions) ─────────────────────────────

    let mut pos_bytes: Vec<u8> = vec![];
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];

    for &p in &parsed.positions {
        for k in 0..3 {
            mn[k] = mn[k].min(p[k]);
            mx[k] = mx[k].max(p[k]);
            push_f32(&mut pos_bytes, p[k]);
        }
    }
    if mn[0] > mx[0] {
        mn = [0.0; 3];
        mx = [0.0; 3];
    }

    let pos_count = n_pos;
    let sec = write_section(&mut bin, &pos_bytes);
    let mn_f64 = [mn[0] as f64, mn[1] as f64, mn[2] as f64];
    let mx_f64 = [mx[0] as f64, mx[1] as f64, mx[2] as f64];
    let acc = add_accessor(
        &mut buf_views,
        &mut accessors,
        sec,
        5126,
        "VEC3",
        pos_count,
        Some(34962),
        false,
        Some((mn_f64, mx_f64)),
    );
    attr.insert("POSITION".into(), serde_json::json!(acc));

    // ── 2. Normals (per-vertex) ────────────────────────────────────────────

    if !is_point_cloud && has_face_indices {
        let normals: Vec<[f32; 3]> = if !parsed.normals_from_file.is_empty() {
            parsed.normals_from_file.clone()
        } else {
            vertex_normals(&parsed.positions, &parsed.face_indices)
        };

        let mut nrm_bytes: Vec<u8> = Vec::with_capacity(normals.len() * 12);
        for n in &normals {
            push_f32(&mut nrm_bytes, n[0]);
            push_f32(&mut nrm_bytes, n[1]);
            push_f32(&mut nrm_bytes, n[2]);
        }
        let sec = write_section(&mut bin, &nrm_bytes);
        let acc = add_accessor(
            &mut buf_views,
            &mut accessors,
            sec,
            5126,
            "VEC3",
            normals.len(),
            Some(34962),
            false,
            None,
        );
        attr.insert("NORMAL".into(), serde_json::json!(acc));
    }

    // ── 3. Colors ──────────────────────────────────────────────────────────────

    if has_colors {
        let mut col_bytes: Vec<u8> = Vec::with_capacity(parsed.colors.len() * 3);
        for &c in &parsed.colors {
            col_bytes.push(c[0]);
            col_bytes.push(c[1]);
            col_bytes.push(c[2]);
        }
        let sec = write_section(&mut bin, &col_bytes);
        let acc = add_accessor(
            &mut buf_views,
            &mut accessors,
            sec,
            5121,
            "VEC3",
            parsed.colors.len(),
            Some(34962),
            true,
            None,
        );
        attr.insert("COLOR_0".into(), serde_json::json!(acc));
    }

    // ── 4. Indices ────────────────────────────────────────────────────────────

    if has_face_indices {
        let mut idx_bytes: Vec<u8> = Vec::with_capacity(parsed.face_indices.len() * 4);
        for &i in &parsed.face_indices {
            push_u32(&mut idx_bytes, i);
        }
        let sec = write_section(&mut bin, &idx_bytes);
        let acc = add_accessor(
            &mut buf_views,
            &mut accessors,
            sec,
            5125,
            "SCALAR",
            parsed.face_indices.len(),
            Some(34963),
            false,
            None,
        );
        indices_acc = Some(acc);
    }

    pad4(&mut bin, 0);
    let bin_padded = align4(bin.len());

    // ── glTF JSON ──────────────────────────────────────────────────────────────

    let primitive_mode = if is_point_cloud { 0u32 } else { 4u32 };

    let mut primitive = serde_json::json!({
        "mode": primitive_mode,
        "attributes": attr,
        "material": 0,
    });
    if let Some(ia) = indices_acc {
        primitive["indices"] = serde_json::json!(ia);
    }

    let (materials, textures_val, images, samplers) = if has_colors {
        (
            serde_json::json!([{
                "name": "vcol_mat",
                "pbrMetallicRoughness": {
                    "metallicFactor": 0.0,
                    "roughnessFactor": 1.0,
                },
                "extensions": {
                    "KHR_materials_unlit": {}
                },
            }]),
            serde_json::json!([]),
            serde_json::json!([]),
            serde_json::json!([]),
        )
    } else {
        (
            serde_json::json!([{
                "name": "default_mat",
                "pbrMetallicRoughness": {
                    "baseColorFactor": [0.47, 0.56, 0.65, 1.0],
                    "metallicFactor": 0.0,
                    "roughnessFactor": 0.8,
                },
                "doubleSided": true,
                "extensions": {
                    "KHR_materials_unlit": {}
                },
            }]),
            serde_json::json!([]),
            serde_json::json!([]),
            serde_json::json!([]),
        )
    };

    let gltf = serde_json::json!({
        "asset": {"version": "2.0", "generator": "colmap-openmvs-app"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [primitive]}],
        "materials": materials,
        "textures": textures_val,
        "images": images,
        "samplers": samplers,
        "accessors": accessors,
        "bufferViews": buf_views,
        "buffers": [{"byteLength": bin_padded}],
        "extensionsUsed": ["KHR_materials_unlit"],
        "extensionsRequired": ["KHR_materials_unlit"],
    });

    let json_str = serde_json::to_string(&gltf)?;
    let json_padded = align4(json_str.len());

    Ok((json_str, bin, json_padded, bin_padded))
}

fn build_glb(parsed: ParsedPly, textures: Vec<(String, Vec<u8>)>) -> anyhow::Result<Vec<u8>> {
    let mut parsed = parsed;
    // Normalize: if texture_groups exist but no textures are provided,
    // flatten their data into face_indices so the non-textured path
    // still renders the mesh geometry.
    if !parsed.texture_groups.is_empty() && textures.is_empty() {
        for group in std::mem::take(&mut parsed.texture_groups) {
            parsed.face_indices.extend(group.face_indices);
            parsed.face_uvs.extend(group.face_uvs);
        }
    }
    let (json_str, bin, json_padded, bin_padded) = build_glb_sections(&parsed, &textures)?;
    let json_bytes = json_str.as_bytes();
    let total = 12 + 8 + json_padded + 8 + bin_padded;

    let mut glb: Vec<u8> = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    push_u32(&mut glb, 2);
    push_u32(&mut glb, total as u32);
    push_u32(&mut glb, json_padded as u32);
    push_u32(&mut glb, 0x4E4F534A);
    glb.extend_from_slice(json_bytes);
    while glb.len() < 12 + 8 + json_padded {
        glb.push(0x20);
    }
    push_u32(&mut glb, bin_padded as u32);
    push_u32(&mut glb, 0x004E4942);
    glb.extend_from_slice(&bin);
    while glb.len() < total {
        glb.push(0);
    }

    Ok(glb)
}
