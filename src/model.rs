use anyhow::{Context, Result, bail};

/// How a batch's texture alpha is used, from the M2 material blend mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blend {
    Opaque,
    AlphaKey,
    Alpha,
    /// Blend modes not needed for static rendering; raw value kept for diagnostics.
    Other(u16),
}

impl Blend {
    pub fn from_raw(raw_blend_mode: u16) -> Self {
        match raw_blend_mode {
            0 => Blend::Opaque,
            1 => Blend::AlphaKey,
            2 => Blend::Alpha,
            other => Blend::Other(other),
        }
    }

    pub fn is_transparent(self) -> bool {
        !matches!(self, Blend::Opaque | Blend::AlphaKey)
    }
}

/// One drawable submesh section: indices into the model vertex buffers plus
/// the texture and material state to draw them with.
#[derive(Debug, Clone)]
pub struct Batch {
    pub indices: Vec<u32>,
    pub texture_path: Option<String>,
    pub blend: Blend,
    pub unlit: bool,
    pub double_sided: bool,
    /// Submesh center depth, used to sort transparent batches back-to-front.
    pub depth_center: f32,
}

/// Renderer-ready static geometry for one M2 model (skin profile 0).
#[derive(Debug, Clone)]
pub struct RenderModel {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub batches: Vec<Batch>,
    pub bounding_center: [f32; 3],
    pub bounding_radius: f32,
    pub version: u32,
}

/// Bounds-checked little-endian reader over the raw M2 bytes.
/// Every file-derived offset goes through here so corrupt files error
/// instead of panicking or reading out of bounds.
struct ByteReader<'a> {
    bytes: &'a [u8],
}

impl<'a> ByteReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn read_u32(&self, offset: usize) -> Result<u32> {
        let chunk = self
            .bytes
            .get(offset..offset + 4)
            .context("u32 read out of bounds")?;
        Ok(u32::from_le_bytes(chunk.try_into().unwrap()))
    }

    fn read_u16(&self, offset: usize) -> Result<u16> {
        let chunk = self
            .bytes
            .get(offset..offset + 2)
            .context("u16 read out of bounds")?;
        Ok(u16::from_le_bytes(chunk.try_into().unwrap()))
    }

    fn read_f32(&self, offset: usize) -> Result<f32> {
        Ok(f32::from_bits(self.read_u32(offset)?))
    }

    /// An M2 array header is a (count, offset) pair of u32s.
    fn read_array_header(&self, offset: usize) -> Result<(usize, usize)> {
        Ok((
            self.read_u32(offset)? as usize,
            self.read_u32(offset + 4)? as usize,
        ))
    }

    fn read_string_at(&self, byte_count: usize, offset: usize) -> Result<String> {
        if byte_count == 0 {
            return Ok(String::new());
        }
        let chunk = self
            .bytes
            .get(offset..offset + byte_count)
            .context("string read out of bounds")?;
        let end = chunk.iter().position(|&c| c == 0).unwrap_or(chunk.len());
        Ok(String::from_utf8_lossy(&chunk[..end]).to_string())
    }

    fn require_range(&self, offset: usize, byte_count: usize, what: &str) -> Result<()> {
        if offset + byte_count > self.bytes.len() {
            bail!("{what} out of bounds");
        }
        Ok(())
    }
}

// Header field offsets in the 1.12.1 (version 256) MD20 layout.
const HEADER_NAME_OFFSET: usize = 8;
const HEADER_VERTICES_OFFSET: usize = 68;
const HEADER_VIEWS_OFFSET: usize = 76;
const HEADER_TEXTURES_OFFSET: usize = 92;
const HEADER_RENDER_FLAGS_OFFSET: usize = 132;
const HEADER_TEXTURE_LOOKUP_OFFSET: usize = 148;
const MIN_HEADER_LEN: usize = 336;
const SUPPORTED_VERSION: u32 = 256;
const VERTEX_BYTE_SIZE: usize = 48;
const TEXTURE_ENTRY_BYTE_SIZE: usize = 16;
const MATERIAL_ENTRY_BYTE_SIZE: usize = 4;
const MODEL_VIEW_BYTE_SIZE: usize = 44;
const SUBMESH_ENTRY_BYTE_SIZE: usize = 32;
const TEXTURE_UNIT_BYTE_SIZE: usize = 24;

/// Parse the static-rendering subset of a 1.12.1 MD20 model.
/// `internal_path` is the model's MPQ-internal path, used to derive
/// texture paths for replaceable (filename-less) textures.
pub fn parse_m2(model_bytes: &[u8], internal_path: &str) -> Result<RenderModel> {
    let reader = ByteReader::new(model_bytes);
    if model_bytes.len() < MIN_HEADER_LEN || &model_bytes[0..4] != b"MD20" {
        bail!("not an MD20 model");
    }
    let version = reader.read_u32(4)?;
    if version != SUPPORTED_VERSION {
        bail!("unsupported M2 version {version} (expected {SUPPORTED_VERSION} for 1.12.1)");
    }
    let (model_name_length, model_name_offset) = reader.read_array_header(HEADER_NAME_OFFSET)?;
    let model_name = reader
        .read_string_at(model_name_length, model_name_offset)
        .unwrap_or_else(|_| "model".into());

    let geometry = parse_vertices(&reader)?;
    let texture_paths = parse_texture_paths(&reader, internal_path, &model_name)?;
    let materials = parse_materials(&reader)?;
    let texture_lookup = parse_texture_lookup(&reader)?;
    let skin = parse_first_embedded_skin(&reader)?;
    let batches = assemble_batches(&skin, &materials, &texture_lookup, &texture_paths);
    if batches.is_empty() {
        bail!("model has no renderable batches");
    }

    let (bounding_center, bounding_radius) = compute_bounding_sphere(&geometry.positions);
    Ok(RenderModel {
        name: model_name,
        positions: geometry.positions,
        normals: geometry.normals,
        uvs: geometry.uvs,
        batches,
        bounding_center,
        bounding_radius,
        version,
    })
}

struct VertexGeometry {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
}

fn parse_vertices(reader: &ByteReader) -> Result<VertexGeometry> {
    let (vertex_count, vertices_offset) = reader.read_array_header(HEADER_VERTICES_OFFSET)?;
    let mut positions = Vec::with_capacity(vertex_count);
    let mut normals = Vec::with_capacity(vertex_count);
    let mut uvs = Vec::with_capacity(vertex_count);
    for vertex_index in 0..vertex_count {
        let vertex_offset = vertices_offset + vertex_index * VERTEX_BYTE_SIZE;
        reader.require_range(vertex_offset, VERTEX_BYTE_SIZE, "vertex")?;
        positions.push([
            reader.read_f32(vertex_offset)?,
            reader.read_f32(vertex_offset + 4)?,
            reader.read_f32(vertex_offset + 8)?,
        ]);
        normals.push([
            reader.read_f32(vertex_offset + 20)?,
            reader.read_f32(vertex_offset + 24)?,
            reader.read_f32(vertex_offset + 28)?,
        ]);
        uvs.push([
            reader.read_f32(vertex_offset + 32)?,
            reader.read_f32(vertex_offset + 36)?,
        ]);
    }
    Ok(VertexGeometry {
        positions,
        normals,
        uvs,
    })
}

fn parse_texture_paths(
    reader: &ByteReader,
    internal_path: &str,
    model_name: &str,
) -> Result<Vec<Option<String>>> {
    let (texture_count, textures_offset) = reader.read_array_header(HEADER_TEXTURES_OFFSET)?;
    let mut texture_paths = Vec::with_capacity(texture_count);
    for texture_index in 0..texture_count {
        let entry_offset = textures_offset + texture_index * TEXTURE_ENTRY_BYTE_SIZE;
        let texture_type = reader.read_u32(entry_offset)?;
        let (filename_length, filename_offset) = reader.read_array_header(entry_offset + 8)?;
        let stored_filename = if filename_length > 0 {
            reader.read_string_at(filename_length, filename_offset)?
        } else {
            String::new()
        };
        texture_paths.push(resolve_texture_path(
            internal_path,
            model_name,
            texture_type,
            &stored_filename,
        ));
    }
    Ok(texture_paths)
}

struct Material {
    render_flags: u16,
    blend: Blend,
}

fn parse_materials(reader: &ByteReader) -> Result<Vec<Material>> {
    let (material_count, materials_offset) =
        reader.read_array_header(HEADER_RENDER_FLAGS_OFFSET)?;
    let mut materials = Vec::with_capacity(material_count);
    for material_index in 0..material_count {
        let entry_offset = materials_offset + material_index * MATERIAL_ENTRY_BYTE_SIZE;
        materials.push(Material {
            render_flags: reader.read_u16(entry_offset)?,
            blend: Blend::from_raw(reader.read_u16(entry_offset + 2)?),
        });
    }
    Ok(materials)
}

fn parse_texture_lookup(reader: &ByteReader) -> Result<Vec<u16>> {
    let (lookup_count, lookup_offset) = reader.read_array_header(HEADER_TEXTURE_LOOKUP_OFFSET)?;
    let mut texture_lookup = Vec::with_capacity(lookup_count);
    for lookup_index in 0..lookup_count {
        texture_lookup.push(reader.read_u16(lookup_offset + lookup_index * 2)?);
    }
    Ok(texture_lookup)
}

struct SubmeshRange {
    triangle_start: usize,
    triangle_count: usize,
    center: [f32; 3],
}

struct EmbeddedSkin {
    vertex_remap: Vec<u16>,
    triangle_indices: Vec<u16>,
    submeshes: Vec<SubmeshRange>,
    texture_units: Vec<TextureUnit>,
}

struct TextureUnit {
    submesh_index: usize,
    material_index: usize,
    texture_combo_index: usize,
}

fn parse_first_embedded_skin(reader: &ByteReader) -> Result<EmbeddedSkin> {
    let (view_count, views_offset) = reader.read_array_header(HEADER_VIEWS_OFFSET)?;
    if view_count == 0 {
        bail!("model has no embedded skin views");
    }
    // First embedded ModelView only (skin profile 0); see README design choices.
    let model_view_offset = views_offset;
    reader.require_range(model_view_offset, MODEL_VIEW_BYTE_SIZE, "model view")?;
    let (index_count, indices_offset) = reader.read_array_header(model_view_offset)?;
    let (triangle_count, triangles_offset) = reader.read_array_header(model_view_offset + 8)?;
    let (submesh_count, submeshes_offset) = reader.read_array_header(model_view_offset + 24)?;
    let (unit_count, units_offset) = reader.read_array_header(model_view_offset + 32)?;

    let vertex_remap = read_u16_array(reader, index_count, indices_offset)?;
    let triangle_indices = read_u16_array(reader, triangle_count, triangles_offset)?;

    let mut submeshes = Vec::with_capacity(submesh_count);
    for submesh_index in 0..submesh_count {
        let entry_offset = submeshes_offset + submesh_index * SUBMESH_ENTRY_BYTE_SIZE;
        submeshes.push(SubmeshRange {
            triangle_start: reader.read_u16(entry_offset + 8)? as usize,
            triangle_count: reader.read_u16(entry_offset + 10)? as usize,
            center: [
                reader.read_f32(entry_offset + 16)?,
                reader.read_f32(entry_offset + 20)?,
                reader.read_f32(entry_offset + 24)?,
            ],
        });
    }

    let mut texture_units = Vec::with_capacity(unit_count);
    for unit_index in 0..unit_count {
        let entry_offset = units_offset + unit_index * TEXTURE_UNIT_BYTE_SIZE;
        texture_units.push(TextureUnit {
            submesh_index: reader.read_u16(entry_offset + 4)? as usize,
            material_index: reader.read_u16(entry_offset + 10)? as usize,
            texture_combo_index: reader.read_u16(entry_offset + 16)? as usize,
        });
    }

    Ok(EmbeddedSkin {
        vertex_remap,
        triangle_indices,
        submeshes,
        texture_units,
    })
}

fn read_u16_array(reader: &ByteReader, count: usize, offset: usize) -> Result<Vec<u16>> {
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        values.push(reader.read_u16(offset + index * 2)?);
    }
    Ok(values)
}

fn assemble_batches(
    skin: &EmbeddedSkin,
    materials: &[Material],
    texture_lookup: &[u16],
    texture_paths: &[Option<String>],
) -> Vec<Batch> {
    let mut batches = Vec::with_capacity(skin.texture_units.len());
    for unit in &skin.texture_units {
        let Some(submesh) = skin.submeshes.get(unit.submesh_index) else {
            continue;
        };
        let default_material = Material {
            render_flags: 0,
            blend: Blend::Opaque,
        };
        let material = materials
            .get(unit.material_index)
            .unwrap_or(&default_material);
        let texture_index = texture_lookup
            .get(unit.texture_combo_index)
            .copied()
            .unwrap_or(0) as usize;
        let texture_path = texture_paths.get(texture_index).cloned().flatten();

        let mut batch_indices = Vec::with_capacity(submesh.triangle_count);
        for triangle_position in
            submesh.triangle_start..submesh.triangle_start + submesh.triangle_count
        {
            let Some(&remap_index) = skin.triangle_indices.get(triangle_position) else {
                break;
            };
            let Some(&vertex_index) = skin.vertex_remap.get(remap_index as usize) else {
                continue;
            };
            batch_indices.push(vertex_index as u32);
        }
        if batch_indices.is_empty() {
            continue;
        }
        batches.push(Batch {
            indices: batch_indices,
            texture_path,
            blend: material.blend,
            unlit: material.render_flags & 0x01 != 0,
            // Foliage/alpha batches are authored as single planes viewed from both sides;
            // WoW-era flags do not reliably mark this, so never cull non-opaque batches.
            double_sided: material.render_flags & 0x04 != 0 || material.blend != Blend::Opaque,
            depth_center: submesh.center[2],
        });
    }
    batches
}

/// Resolve one texture entry to an MPQ-internal path. Hardcoded entries carry
/// their filename; replaceable entries (monster/body skins) derive it from
/// the model name, e.g. `DragonOnyxia` + type Monster1 -> `DragonOnyxia1.blp`.
fn resolve_texture_path(
    internal_path: &str,
    model_name: &str,
    texture_type: u32,
    stored_filename: &str,
) -> Option<String> {
    if !stored_filename.is_empty() {
        return Some(stored_filename.to_string());
    }
    let suffix = match texture_type {
        11 => "1",
        12 => "2",
        13 => "3",
        1..=3 => "",
        _ => return None,
    };
    let model_directory = internal_path
        .rsplit_once('\\')
        .map(|(directory, _)| directory)
        .unwrap_or("");
    let base_name = model_name.trim_end_matches('\0');
    if model_directory.is_empty() {
        Some(format!("{base_name}{suffix}.blp"))
    } else {
        Some(format!("{model_directory}\\{base_name}{suffix}.blp"))
    }
}

fn compute_bounding_sphere(positions: &[[f32; 3]]) -> ([f32; 3], f32) {
    if positions.is_empty() {
        return ([0.0; 3], 1.0);
    }
    let mut min_corner = positions[0];
    let mut max_corner = positions[0];
    for position in positions {
        for axis in 0..3 {
            min_corner[axis] = min_corner[axis].min(position[axis]);
            max_corner[axis] = max_corner[axis].max(position[axis]);
        }
    }
    let center = [
        (min_corner[0] + max_corner[0]) / 2.0,
        (min_corner[1] + max_corner[1]) / 2.0,
        (min_corner[2] + max_corner[2]) / 2.0,
    ];
    let mut max_distance_squared = 0.0f32;
    for position in positions {
        let distance_squared = (position[0] - center[0]).powi(2)
            + (position[1] - center[1]).powi(2)
            + (position[2] - center[2]).powi(2);
        max_distance_squared = max_distance_squared.max(distance_squared);
    }
    (center, max_distance_squared.sqrt().max(0.001))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the smallest v256 MD20 byte buffer `parse_m2` accepts:
    /// one triangle, one hardcoded texture, one opaque material.
    fn build_minimal_model() -> Vec<u8> {
        let mut bytes = vec![0u8; 4096];
        let put_u32 = |bytes: &mut Vec<u8>, offset: usize, value: u32| {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        };
        let put_u16 = |bytes: &mut Vec<u8>, offset: usize, value: u16| {
            bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        };
        let put_f32 = |bytes: &mut Vec<u8>, offset: usize, value: f32| {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        };
        let put_array = |bytes: &mut Vec<u8>, offset: usize, count: u32, data_offset: u32| {
            put_u32(bytes, offset, count);
            put_u32(bytes, offset + 4, data_offset);
        };

        bytes[0..4].copy_from_slice(b"MD20");
        put_u32(&mut bytes, 4, SUPPORTED_VERSION);
        // Name "TestModel" at offset 1000.
        put_array(&mut bytes, HEADER_NAME_OFFSET, 10, 1000);
        bytes[1000..1010].copy_from_slice(b"TestModel\0");
        // 3 vertices at 1100 (48 bytes each): (0,0,0) (1,0,0) (0,1,0), normal +Z, uv (0,0).
        put_array(&mut bytes, HEADER_VERTICES_OFFSET, 3, 1100);
        for (i, (x, y)) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)].iter().enumerate() {
            let base = 1100 + i * VERTEX_BYTE_SIZE;
            put_f32(&mut bytes, base, *x);
            put_f32(&mut bytes, base + 4, *y);
            put_f32(&mut bytes, base + 8, 0.0);
            put_f32(&mut bytes, base + 28, 1.0); // normal z
        }
        // One view at 1400; ModelView arrays point at the blocks below.
        put_array(&mut bytes, HEADER_VIEWS_OFFSET, 1, 1400);
        put_array(&mut bytes, 1400, 3, 1500); // vertex remap
        put_array(&mut bytes, 1408, 3, 1600); // triangle indices
        put_array(&mut bytes, 1416, 0, 0); // properties (unused)
        put_array(&mut bytes, 1424, 1, 1700); // submeshes
        put_array(&mut bytes, 1432, 1, 1800); // texture units
        for i in 0..3u16 {
            put_u16(&mut bytes, 1500 + i as usize * 2, i);
            put_u16(&mut bytes, 1600 + i as usize * 2, i);
        }
        // Submesh at 1700: triangle_start=0 (at +8), triangle_count=3 (at +10).
        put_u16(&mut bytes, 1700 + 8, 0);
        put_u16(&mut bytes, 1700 + 10, 3);
        // Texture unit at 1800: submesh 0, material 0, texture combo 0.
        // One texture at 1900: type 0 (hardcoded), filename at 2000.
        put_array(&mut bytes, HEADER_TEXTURES_OFFSET, 1, 1900);
        put_u32(&mut bytes, 1900, 0);
        put_array(&mut bytes, 1908, 12, 2000);
        bytes[2000..2012].copy_from_slice(b"TestSkin.blp");
        // One material at 2100: flags 0, blend 0 (opaque). Lookup [0] at 2200.
        put_array(&mut bytes, HEADER_RENDER_FLAGS_OFFSET, 1, 2100);
        put_array(&mut bytes, HEADER_TEXTURE_LOOKUP_OFFSET, 1, 2200);
        put_u16(&mut bytes, 2200, 0);
        bytes
    }

    #[test]
    fn parse_minimal_model_happy_path() {
        let model = parse_m2(&build_minimal_model(), "Creature\\Test\\TestModel.m2")
            .expect("minimal model parses");
        assert_eq!(model.name, "TestModel");
        assert_eq!(model.version, SUPPORTED_VERSION);
        assert_eq!(model.positions.len(), 3);
        assert_eq!(model.batches.len(), 1);
        assert_eq!(model.batches[0].indices, vec![0, 1, 2]);
        assert_eq!(
            model.batches[0].texture_path.as_deref(),
            Some("TestSkin.blp")
        );
        assert_eq!(model.batches[0].blend, Blend::Opaque);
        assert!(model.bounding_radius > 0.5 && model.bounding_radius < 1.0);
    }

    #[test]
    fn parse_rejects_bad_magic() {
        assert!(parse_m2(b"NOPE", "x.m2").is_err());
        let mut bytes = build_minimal_model();
        bytes[0..4].copy_from_slice(b"XXXX");
        assert!(parse_m2(&bytes, "x.m2").is_err());
    }

    #[test]
    fn parse_rejects_unsupported_version() {
        let mut bytes = build_minimal_model();
        bytes[4..8].copy_from_slice(&257u32.to_le_bytes());
        let error = parse_m2(&bytes, "x.m2").expect_err("version 257 must fail");
        assert!(
            error.to_string().contains("257"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn parse_rejects_truncated_vertex_buffer() {
        let mut bytes = build_minimal_model();
        bytes.truncate(1150); // cuts inside the vertex block at 1100
        assert!(parse_m2(&bytes, "x.m2").is_err());
    }

    #[test]
    fn parse_rejects_model_without_skin_views() {
        let mut bytes = build_minimal_model();
        bytes[HEADER_VIEWS_OFFSET..HEADER_VIEWS_OFFSET + 4].copy_from_slice(&0u32.to_le_bytes());
        assert!(parse_m2(&bytes, "x.m2").is_err());
    }

    #[test]
    fn blend_from_raw_maps_known_modes() {
        assert_eq!(Blend::from_raw(0), Blend::Opaque);
        assert_eq!(Blend::from_raw(1), Blend::AlphaKey);
        assert_eq!(Blend::from_raw(2), Blend::Alpha);
        assert_eq!(Blend::from_raw(7), Blend::Other(7));
        assert!(Blend::Alpha.is_transparent());
        assert!(!Blend::AlphaKey.is_transparent());
    }

    #[test]
    fn resolve_texture_path_prefers_stored_filename() {
        assert_eq!(
            resolve_texture_path("Creature\\X\\X.m2", "X", 0, "A\\B.blp").as_deref(),
            Some("A\\B.blp")
        );
    }

    #[test]
    fn resolve_texture_path_derives_monster_skin_names() {
        assert_eq!(
            resolve_texture_path("Creature\\Dragon\\DragonOnyxia.m2", "DragonOnyxia", 11, "")
                .as_deref(),
            Some("Creature\\Dragon\\DragonOnyxia1.blp")
        );
        assert_eq!(
            resolve_texture_path("Creature\\Dragon\\DragonOnyxia.m2", "DragonOnyxia", 13, "")
                .as_deref(),
            Some("Creature\\Dragon\\DragonOnyxia3.blp")
        );
    }

    #[test]
    fn resolve_texture_path_returns_none_for_unknown_types() {
        assert_eq!(resolve_texture_path("A\\B.m2", "B", 99, ""), None);
    }

    #[test]
    fn bounding_sphere_covers_all_positions() {
        let (center, radius) = compute_bounding_sphere(&[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]]);
        assert_eq!(center, [1.0, 0.0, 0.0]);
        assert!((radius - 1.0).abs() < 1e-6);
        assert_eq!(compute_bounding_sphere(&[]), ([0.0; 3], 1.0));
    }
}
