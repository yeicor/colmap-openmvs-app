//! PLY → GLB (binary glTF 2.0) converter.
//!
//! Handles the variants produced by COLMAP and OpenMVS:
//!   - Point cloud (xyz + optional rgb)
//!   - Indexed triangle mesh (xyz + optional rgb, face vertex_indices)
//!   - Textured mesh  (xyz, face vertex_indices + per-face texcoord, companion PNG)

use anyhow::anyhow;

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Convert PLY bytes + optional companion texture PNG → GLB bytes.
pub fn ply_to_glb(ply_bytes: &[u8], companion_png: Option<Vec<u8>>) -> anyhow::Result<Vec<u8>> {
    let parsed = parse_ply(ply_bytes)?;
    build_glb(parsed, companion_png)
}

/// Extract `comment TextureFile <name>` from the PLY header.
pub fn ply_texture_file_name(ply_bytes: &[u8]) -> Option<String> {
    let chunk = &ply_bytes[..ply_bytes.len().min(4096)];
    let text = String::from_utf8_lossy(chunk);
    for line in text.lines() {
        if line == "end_header" {
            break;
        }
        if let Some(name) = line.strip_prefix("comment TextureFile ") {
            return Some(name.trim().to_string());
        }
    }
    None
}

/// Build a GLB point-cloud from raw positions + optional RGB colors.
pub fn points_to_glb(
    positions: &[[f32; 3]],
    colors_rgb: Option<&[[u8; 3]]>,
) -> anyhow::Result<Vec<u8>> {
    let parsed = ParsedPly {
        positions: positions.to_vec(),
        normals_from_file: vec![],
        colors: colors_rgb.map(|c| c.to_vec()).unwrap_or_default(),
        face_indices: vec![],
        face_uvs: vec![],
        texture_file: None,
    };
    build_glb(parsed, None)
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum Fmt {
    BinaryLE,
    BinaryBE,
    Ascii,
}

#[derive(Clone, Copy, Debug)]
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
struct ParsedPly {
    positions: Vec<[f32; 3]>,
    normals_from_file: Vec<[f32; 3]>,
    colors: Vec<[u8; 3]>,
    face_indices: Vec<u32>,  // multiple of 3
    face_uvs: Vec<[f32; 2]>, // same length as face_indices when present
    texture_file: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Header parser
// ─────────────────────────────────────────────────────────────────────────────

fn parse_header(bytes: &[u8]) -> anyhow::Result<Header> {
    // Find end_header to determine data_offset
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

// ─────────────────────────────────────────────────────────────────────────────
// Binary reader
// ─────────────────────────────────────────────────────────────────────────────

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
        Ok(self.read_scalar_f32(s)? as u8)
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

// ─────────────────────────────────────────────────────────────────────────────
// PLY parser dispatch
// ─────────────────────────────────────────────────────────────────────────────

fn parse_ply(bytes: &[u8]) -> anyhow::Result<ParsedPly> {
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
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Binary parser
// ─────────────────────────────────────────────────────────────────────────────

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
    let uv_idx = elem.props.iter().position(|p| p.name == "texcoord");

    out.face_indices.reserve(elem.count * 3);
    if uv_idx.is_some() {
        out.face_uvs.reserve(elem.count * 3);
    }

    for _ in 0..elem.count {
        let mut verts: Vec<u32> = vec![];
        let mut uvs: Vec<[f32; 2]> = vec![];

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
                    } else {
                        for _ in 0..n {
                            r.skip(*val)?;
                        }
                    }
                }
                PropDef::Scalar(s) => {
                    r.skip(*s)?;
                }
            }
        }

        // Fan-triangulate
        let nv = verts.len();
        for j in 1..nv.saturating_sub(1) {
            out.face_indices.push(verts[0]);
            out.face_indices.push(verts[j]);
            out.face_indices.push(verts[j + 1]);
            if !uvs.is_empty() {
                out.face_uvs.push(uvs[0]);
                out.face_uvs.push(uvs[j]);
                out.face_uvs.push(uvs[j + 1]);
            }
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

// ─────────────────────────────────────────────────────────────────────────────
// ASCII parser (vertices only; faces with vertex_indices, no texcoord)
// ─────────────────────────────────────────────────────────────────────────────

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
                    // Expand tokens for list properties (just skip them in ASCII)
                    // For scalar-only vertex elements this works directly
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
                                flat.push(0.0); // placeholder
                            }
                        }
                    }
                    let get = |idx: Option<usize>| -> f32 {
                        idx.and_then(|i| flat.get(i)).copied().unwrap_or(0.0) as f32
                    };
                    out.positions.push([get(xi), get(yi), get(zi)]);
                    if has_col {
                        out.colors
                            .push([get(ri) as u8, get(gi) as u8, get(bi) as u8]);
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
                        // Before vi_pi there may be other scalar props
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

// ─────────────────────────────────────────────────────────────────────────────
// Normal computation
// ─────────────────────────────────────────────────────────────────────────────

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

/// Per-face normals for an expanded (textured) mesh.
/// Output length == face_indices.len()
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

/// Smooth vertex normals for an indexed mesh.
/// Output length == positions.len()
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

// ─────────────────────────────────────────────────────────────────────────────
// GLB builder
// ─────────────────────────────────────────────────────────────────────────────

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

fn pad4(bin: &mut Vec<u8>, byte: u8) {
    while bin.len() % 4 != 0 {
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

fn build_glb(parsed: ParsedPly, companion_png: Option<Vec<u8>>) -> anyhow::Result<Vec<u8>> {
    let n_pos = parsed.positions.len();
    if n_pos == 0 {
        return Err(anyhow!("PLY has no vertices"));
    }

    let is_point_cloud = parsed.face_indices.is_empty();
    let has_uvs = !parsed.face_uvs.is_empty() && companion_png.is_some();
    let has_indices = !is_point_cloud && !has_uvs;
    let has_colors = !parsed.colors.is_empty();

    let mut bin: Vec<u8> = Vec::new();
    let mut buf_views: Vec<serde_json::Value> = vec![];
    let mut accessors: Vec<serde_json::Value> = vec![];
    let mut attr = serde_json::Map::new();
    let mut indices_acc: Option<usize> = None;
    let mut tex_bv_idx: Option<usize> = None;

    // Helper: add bufferView + accessor, return accessor index
    let mut add_accessor = |buf_views: &mut Vec<serde_json::Value>,
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

    // ── 1. Positions ─────────────────────────────────────────────────────────

    let mut pos_bytes: Vec<u8> = vec![];
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];

    if has_uvs {
        // Expanded positions (one per face-vertex)
        for &fi in &parsed.face_indices {
            let p = parsed.positions[fi as usize];
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
                push_f32(&mut pos_bytes, p[k]);
            }
        }
    } else {
        // Original vertex positions
        for &p in &parsed.positions {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
                push_f32(&mut pos_bytes, p[k]);
            }
        }
    }
    if mn[0] > mx[0] {
        mn = [0.0; 3];
        mx = [0.0; 3];
    }

    let pos_count = if has_uvs {
        parsed.face_indices.len()
    } else {
        n_pos
    };
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

    // ── 2. Normals ────────────────────────────────────────────────────────────

    if !is_point_cloud {
        let normals: Vec<[f32; 3]> = if !parsed.normals_from_file.is_empty() {
            if has_uvs {
                // expand file normals
                parsed
                    .face_indices
                    .iter()
                    .map(|&fi| parsed.normals_from_file[fi as usize])
                    .collect()
            } else {
                parsed.normals_from_file.clone()
            }
        } else if has_uvs {
            face_normals_expanded(&parsed.positions, &parsed.face_indices)
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

    // ── 3. UVs ────────────────────────────────────────────────────────────────

    if has_uvs {
        let mut uv_bytes: Vec<u8> = Vec::with_capacity(parsed.face_uvs.len() * 8);
        tracing::info!("Expanding {} UVs", parsed.face_uvs.len());
        for &uv in &parsed.face_uvs {
            push_f32(&mut uv_bytes, uv[0]);
            push_f32(&mut uv_bytes, 1.0 - uv[1]);
        }
        let sec = write_section(&mut bin, &uv_bytes);
        let acc = add_accessor(
            &mut buf_views,
            &mut accessors,
            sec,
            5126,
            "VEC2",
            parsed.face_uvs.len(),
            Some(34962),
            false,
            None,
        );
        attr.insert("TEXCOORD_0".into(), serde_json::json!(acc));
    }

    // ── 4. Colors ─────────────────────────────────────────────────────────────

    if has_colors && !has_uvs {
        let mut col_bytes: Vec<u8> = Vec::with_capacity(parsed.colors.len() * 4);
        for &c in &parsed.colors {
            col_bytes.push(c[0]);
            col_bytes.push(c[1]);
            col_bytes.push(c[2]);
            col_bytes.push(255);
        }
        let sec = write_section(&mut bin, &col_bytes);
        let acc = add_accessor(
            &mut buf_views,
            &mut accessors,
            sec,
            5121,
            "VEC4",
            parsed.colors.len(),
            Some(34962),
            true,
            None,
        );
        attr.insert("COLOR_0".into(), serde_json::json!(acc));
    }

    // ── 5. Indices ────────────────────────────────────────────────────────────

    if has_indices {
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

    // ── 6. Texture PNG ────────────────────────────────────────────────────────

    if has_uvs {
        if let Some(png) = companion_png {
            pad4(&mut bin, 0);
            let byte_offset = bin.len();
            let byte_length = png.len();
            bin.extend_from_slice(&png);
            let bv_idx = buf_views.len();
            buf_views.push(serde_json::json!({
                "buffer": 0,
                "byteOffset": byte_offset,
                "byteLength": byte_length,
            }));
            tex_bv_idx = Some(bv_idx);
        }
    }

    pad4(&mut bin, 0);

    // ── glTF JSON ─────────────────────────────────────────────────────────────

    let primitive_mode = if is_point_cloud { 0u32 } else { 4u32 };

    let mut primitive = serde_json::json!({
        "mode": primitive_mode,
        "attributes": attr,
        "material": 0,
    });
    if let Some(ia) = indices_acc {
        primitive["indices"] = serde_json::json!(ia);
    }

    // Material / texture
    let (materials, textures, images, samplers) = if let Some(bv_idx) = tex_bv_idx {
        (
            serde_json::json!([{
                "name": "tex_mat",
                "pbrMetallicRoughness": {
                    "baseColorTexture": {"index": 0},
                    "metallicFactor": 0.0,
                    "roughnessFactor": 1.0,
                },
                "doubleSided": true,
            }]),
            serde_json::json!([{"source": 0, "sampler": 0}]),
            serde_json::json!([{"mimeType": "image/png", "bufferView": bv_idx}]),
            serde_json::json!([{
                "magFilter": 9729,
                "minFilter": 9987,
                "wrapS": 10497,
                "wrapT": 10497,
            }]),
        )
    } else if has_colors {
        (
            serde_json::json!([{
                "name": "vcol_mat",
                "pbrMetallicRoughness": {
                    "metallicFactor": 0.0,
                    "roughnessFactor": 1.0,
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
        "textures": textures,
        "images": images,
        "samplers": samplers,
        "accessors": accessors,
        "bufferViews": buf_views,
        "buffers": [{"byteLength": bin.len()}],
    });

    let json_str = serde_json::to_string(&gltf)?;
    let json_bytes = json_str.as_bytes();
    let json_padded = align4(json_bytes.len());
    let bin_padded = align4(bin.len());
    let total = 12 + 8 + json_padded + 8 + bin_padded;

    let mut glb: Vec<u8> = Vec::with_capacity(total);
    // Header
    glb.extend_from_slice(b"glTF");
    push_u32(&mut glb, 2);
    push_u32(&mut glb, total as u32);
    // JSON chunk
    push_u32(&mut glb, json_padded as u32);
    push_u32(&mut glb, 0x4E4F534A);
    glb.extend_from_slice(json_bytes);
    while glb.len() < 12 + 8 + json_padded {
        glb.push(0x20);
    }
    // BIN chunk
    push_u32(&mut glb, bin_padded as u32);
    push_u32(&mut glb, 0x004E4942);
    glb.extend_from_slice(&bin);
    while glb.len() < total {
        glb.push(0);
    }

    Ok(glb)
}
