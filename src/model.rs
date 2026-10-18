use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blend {
    Opaque,
    AlphaKey,
    Alpha,
    Other(u16),
}

#[derive(Debug, Clone)]
pub struct Batch {
    pub indices: Vec<u32>,
    pub texture: Option<String>,
    pub blend: Blend,
    pub unlit: bool,
    pub double_sided: bool,
    pub depth_center: f32,
}

#[derive(Debug, Clone)]
pub struct RenderModel {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub batches: Vec<Batch>,
    pub center: [f32; 3],
    pub radius: f32,
    pub version: u32,
}

struct Rd<'a> {
    d: &'a [u8],
}
impl<'a> Rd<'a> {
    fn u32(&self, o: usize) -> Result<u32> {
        let b = self.d.get(o..o + 4).context("u32 out of bounds")?;
        Ok(u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u16(&self, o: usize) -> Result<u16> {
        let b = self.d.get(o..o + 2).context("u16 out of bounds")?;
        Ok(u16::from_le_bytes(b.try_into().unwrap()))
    }
    fn f32(&self, o: usize) -> Result<f32> {
        Ok(f32::from_bits(self.u32(o)?))
    }
    fn arr(&self, o: usize) -> Result<(usize, usize)> {
        Ok((self.u32(o)? as usize, self.u32(o + 4)? as usize))
    }
    fn str_at(&self, count: usize, off: usize) -> Result<String> {
        if count == 0 {
            return Ok(String::new());
        }
        let b = self
            .d
            .get(off..off + count)
            .context("string out of bounds")?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        Ok(String::from_utf8_lossy(&b[..end]).to_string())
    }
}

const HDR_VIEWS: usize = 76;
const HDR_TEXTURES: usize = 92;
const HDR_RENDER_FLAGS: usize = 132;
const HDR_TEX_LOOKUP: usize = 148;

pub fn parse_m2(data: &[u8], internal_path: &str) -> Result<RenderModel> {
    let r = Rd { d: data };
    if data.len() < 336 || &data[0..4] != b"MD20" {
        bail!("not an MD20 model");
    }
    let version = r.u32(4)?;
    if version != 256 {
        bail!("unsupported M2 version {version} (expected 256 for 1.12.1)");
    }
    let (name_len, name_off) = r.arr(8)?;
    let name = r
        .str_at(name_len, name_off)
        .unwrap_or_else(|_| "model".into());

    let (n_verts, v_off) = r.arr(68)?;
    let (n_views, views_off) = r.arr(HDR_VIEWS)?;
    let (n_tex, tex_off) = r.arr(HDR_TEXTURES)?;
    let (n_mats, mat_off) = r.arr(HDR_RENDER_FLAGS)?;
    let (n_lookup, lookup_off) = r.arr(HDR_TEX_LOOKUP)?;
    if n_views == 0 {
        bail!("model has no embedded skin views");
    }

    let mut positions = Vec::with_capacity(n_verts);
    let mut normals = Vec::with_capacity(n_verts);
    let mut uvs = Vec::with_capacity(n_verts);
    for i in 0..n_verts {
        let o = v_off + i * 48;
        if o + 48 > data.len() {
            bail!("vertex {i} out of bounds");
        }
        positions.push([r.f32(o)?, r.f32(o + 4)?, r.f32(o + 8)?]);
        normals.push([r.f32(o + 20)?, r.f32(o + 24)?, r.f32(o + 28)?]);
        uvs.push([r.f32(o + 32)?, r.f32(o + 36)?]);
    }

    let mut tex_files: Vec<Option<String>> = Vec::with_capacity(n_tex);
    for i in 0..n_tex {
        let o = tex_off + i * 16;
        let ty = r.u32(o)?;
        let (flen, foff) = r.arr(o + 8)?;
        let file = if flen > 0 {
            r.str_at(flen, foff)?
        } else {
            String::new()
        };
        tex_files.push(resolve_texture(internal_path, &name, ty, &file));
    }
    let mut lookup: Vec<u16> = Vec::with_capacity(n_lookup);
    for i in 0..n_lookup {
        lookup.push(r.u16(lookup_off + i * 2)?);
    }
    let mut mats: Vec<(u16, u16)> = Vec::with_capacity(n_mats);
    for i in 0..n_mats {
        let o = mat_off + i * 4;
        mats.push((r.u16(o)?, r.u16(o + 2)?));
    }

    // First embedded ModelView (44 bytes).
    let mv = views_off;
    let (n_idx, idx_off) = r.arr(mv)?;
    let (n_tri, tri_off) = r.arr(mv + 8)?;
    let (_n_prop, _prop_off) = r.arr(mv + 16)?;
    let (n_sub, sub_off) = r.arr(mv + 24)?;
    let (n_units, unit_off) = r.arr(mv + 32)?;

    let mut remap: Vec<u16> = Vec::with_capacity(n_idx);
    for i in 0..n_idx {
        remap.push(r.u16(idx_off + i * 2)?);
    }
    let mut tris: Vec<u16> = Vec::with_capacity(n_tri);
    for i in 0..n_tri {
        tris.push(r.u16(tri_off + i * 2)?);
    }
    struct Sub {
        t_start: usize,
        t_count: usize,
        center: [f32; 3],
    }
    let mut subs: Vec<Sub> = Vec::with_capacity(n_sub);
    for i in 0..n_sub {
        let o = sub_off + i * 32;
        subs.push(Sub {
            t_start: r.u16(o + 8)? as usize,
            t_count: r.u16(o + 10)? as usize,
            center: [r.f32(o + 16)?, r.f32(o + 20)?, r.f32(o + 24)?],
        });
    }

    let mut batches: Vec<Batch> = Vec::with_capacity(n_units);
    for i in 0..n_units {
        let o = unit_off + i * 24;
        let sub_idx = r.u16(o + 4)? as usize;
        let mat_idx = r.u16(o + 10)? as usize;
        let combo = r.u16(o + 16)? as usize;
        let Some(sub) = subs.get(sub_idx) else {
            continue;
        };
        let (flags, blend_raw) = mats.get(mat_idx).copied().unwrap_or((0, 0));
        let tex_idx = lookup.get(combo).copied().unwrap_or(0) as usize;
        let texture = tex_files.get(tex_idx).cloned().flatten();
        let mut indices: Vec<u32> = Vec::with_capacity(sub.t_count);
        for t in sub.t_start..sub.t_start + sub.t_count {
            let Some(&ti) = tris.get(t) else { break };
            let Some(&vi) = remap.get(ti as usize) else {
                continue;
            };
            indices.push(vi as u32);
        }
        if indices.is_empty() {
            continue;
        }
        batches.push(Batch {
            indices,
            texture,
            blend: match blend_raw {
                0 => Blend::Opaque,
                1 => Blend::AlphaKey,
                2 => Blend::Alpha,
                other => Blend::Other(other),
            },
            unlit: flags & 0x01 != 0,
            // Foliage/alpha batches are authored as single planes viewed from both sides;
            // WoW-era flags do not reliably mark this, so never cull non-opaque batches.
            double_sided: flags & 0x04 != 0 || blend_raw != 0,
            depth_center: sub.center[2],
        });
    }
    if batches.is_empty() {
        bail!("model has no renderable batches");
    }

    let (center, radius) = bounds(&positions);
    Ok(RenderModel {
        name,
        positions,
        normals,
        uvs,
        batches,
        center,
        radius,
        version,
    })
}

fn resolve_texture(internal_path: &str, model_name: &str, ty: u32, file: &str) -> Option<String> {
    if !file.is_empty() {
        return Some(file.to_string());
    }
    let suffix = match ty {
        11 => "1",
        12 => "2",
        13 => "3",
        1..=3 => "",
        _ => return None,
    };
    let dir = internal_path
        .rsplit_once('\\')
        .map(|(d, _)| d)
        .unwrap_or("");
    let base = model_name.trim_end_matches('\0');
    if dir.is_empty() {
        Some(format!("{base}{suffix}.blp"))
    } else {
        Some(format!("{dir}\\{base}{suffix}.blp"))
    }
}

fn bounds(positions: &[[f32; 3]]) -> ([f32; 3], f32) {
    if positions.is_empty() {
        return ([0.0; 3], 1.0);
    }
    let mut min = positions[0];
    let mut max = positions[0];
    for p in positions {
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    let center = [
        (min[0] + max[0]) / 2.0,
        (min[1] + max[1]) / 2.0,
        (min[2] + max[2]) / 2.0,
    ];
    let mut radius = 0.0f32;
    for p in positions {
        let d =
            (p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2) + (p[2] - center[2]).powi(2);
        radius = radius.max(d);
    }
    (center, radius.sqrt().max(0.001))
}
