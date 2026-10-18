use crate::model::RenderModel;
use crate::mpq::DataDir;
use anyhow::{Context, Result};
use std::collections::HashMap;

/// Decode a BLP2 texture to RGBA8. Returns (pixels, width, height).
/// Only the base mip level is decoded (see README design choices).
pub fn decode_blp_rgba(blp_bytes: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let blp_image = wow_blp::parser::load_blp_from_buf(blp_bytes).context("parsing BLP")?;
    let decoded_image = wow_blp::convert::blp_to_image(&blp_image, 0).context("decoding BLP")?;
    let rgba_image = decoded_image.to_rgba8();
    let (width, height) = rgba_image.dimensions();
    Ok((rgba_image.into_raw(), width, height))
}

/// One CPU-decoded texture: (RGBA8 pixels, width, height).
pub type DecodedTexture = (Vec<u8>, u32, u32);

/// CPU-decoded textures keyed by internal BLP path.
pub type DecodedTextures = HashMap<String, DecodedTexture>;

/// A parsed model plus its decoded textures and non-fatal load warnings.
pub type LoadedModel = (RenderModel, DecodedTextures, Vec<String>);

/// Pick the first copy a validator accepts. Returns the copy's source
/// label, its bytes, and how many copies were skipped before it.
pub fn first_usable_copy(
    copies: &[(String, Vec<u8>)],
    mut is_usable: impl FnMut(&[u8]) -> bool,
) -> Option<(&str, &[u8], usize)> {
    for (copy_index, (source_label, copy_bytes)) in copies.iter().enumerate() {
        if is_usable(copy_bytes) {
            return Some((source_label.as_str(), copy_bytes.as_slice(), copy_index));
        }
    }
    None
}

/// Load one model and every texture its batches reference, through the
/// patch chain. Missing/undecodable textures become warnings (the renderer
/// substitutes a magenta placeholder), never load failures.
pub fn load_model(data_directory: &mut DataDir, internal_model_path: &str) -> Result<LoadedModel> {
    // Patch archives can shadow a healthy model with a 0-byte tombstone
    // (patch.MPQ does this to OgreWarlord); walk every copy until one parses.
    let model_copies = data_directory.read_copies(internal_model_path);
    if model_copies.is_empty() {
        anyhow::bail!("reading {internal_model_path}: file not found in any archive");
    }
    let mut model = None;
    let mut skipped_copies = 0usize;
    let mut first_parse_error = String::new();
    for (source_label, copy_bytes) in &model_copies {
        match crate::model::parse_m2(copy_bytes, internal_model_path) {
            Ok(parsed_model) => {
                if skipped_copies > 0 {
                    eprintln!(
                        "note: {internal_model_path}: skipped {skipped_copies} unusable copy/copies, using {source_label}"
                    );
                }
                model = Some(parsed_model);
                break;
            }
            Err(parse_error) => {
                if skipped_copies == 0 {
                    first_parse_error = format!("{parse_error:#}");
                }
                skipped_copies += 1;
            }
        }
    }
    let model = model.with_context(|| {
        format!(
            "parsing {internal_model_path} (no usable copy in any archive: {first_parse_error})"
        )
    })?;
    let (textures, warnings) = load_model_textures(data_directory, &model);
    Ok((model, textures, warnings))
}

fn load_model_textures(
    data_directory: &mut DataDir,
    model: &RenderModel,
) -> (DecodedTextures, Vec<String>) {
    let mut decoded_textures = HashMap::new();
    let mut warnings = Vec::new();
    let mut wanted_texture_paths: Vec<String> = model
        .batches
        .iter()
        .filter_map(|batch| batch.texture_path.clone())
        .collect();
    wanted_texture_paths.sort();
    wanted_texture_paths.dedup();

    // Lazily fetched archive listing, only needed for skin-variant fallback.
    let mut archive_file_list: Option<Vec<String>> = None;
    for texture_path in wanted_texture_paths {
        match load_one_texture(data_directory, &texture_path, &mut archive_file_list) {
            Ok((resolved_path, decoded)) => {
                if resolved_path != texture_path {
                    warnings.push(format!("texture {texture_path} -> using {resolved_path}"));
                }
                decoded_textures.insert(texture_path, decoded);
            }
            Err(warning) => warnings.push(warning),
        }
    }
    (decoded_textures, warnings)
}

/// Load and decode one texture, falling back to a same-directory skin
/// variant (e.g. `RagnarosSkin.blp`) when the derived name is absent.
/// The `Err` value is a human-readable warning, not a fatal error.
fn load_one_texture(
    data_directory: &mut DataDir,
    texture_path: &str,
    archive_file_list: &mut Option<Vec<String>>,
) -> std::result::Result<(String, DecodedTexture), String> {
    // Walk every copy of the texture: a shadowing copy can be a 0-byte
    // tombstone just like models (see load_model), so decode-test each one.
    let texture_copies = data_directory.read_copies(texture_path);
    let usable_copy = first_usable_copy(&texture_copies, |copy_bytes| {
        decode_blp_rgba(copy_bytes).is_ok()
    });
    if let Some((_, usable_bytes, _)) = usable_copy {
        let decoded = decode_blp_rgba(usable_bytes)
            .map_err(|error| format!("texture {texture_path}: {error:#}"))?;
        return Ok((texture_path.to_string(), decoded));
    }
    if !texture_copies.is_empty() {
        return Err(format!(
            "texture {texture_path}: no decodable copy in any archive"
        ));
    }
    if archive_file_list.is_none() {
        *archive_file_list = data_directory.list_all_files().ok();
    }
    let fallback_path = archive_file_list
        .as_ref()
        .and_then(|file_names| find_skin_fallback(file_names, texture_path))
        .ok_or_else(|| format!("texture not found: {texture_path}"))?;
    // The fallback skin can itself be shadowed by a tombstone copy,
    // so walk its copies too (OgreWarlord's skins are 0-byte in patch.MPQ).
    let fallback_copies = data_directory.read_copies(&fallback_path);
    let usable_fallback = first_usable_copy(&fallback_copies, |copy_bytes| {
        decode_blp_rgba(copy_bytes).is_ok()
    })
    .ok_or_else(|| format!("texture not found: {texture_path}"))?;
    let decoded = decode_blp_rgba(usable_fallback.1)
        .map_err(|error| format!("texture {texture_path}: {error:#}"))?;
    Ok((fallback_path, decoded))
}

/// Preference score for skin-variant candidates: light skins first,
/// then anything with "skin" in the name.
fn skin_candidate_score(file_name: &str) -> i32 {
    let lowercased_name = file_name.to_ascii_lowercase();
    let mut score = 0;
    if lowercased_name.contains("light") {
        score += 4;
    }
    if lowercased_name.contains("skin") {
        score += 2;
    }
    score
}

/// Find the best same-directory `.blp` whose name starts with the wanted
/// texture's base name (numeric suffix stripped), e.g. wanting
/// `Creature\Ragnaros\Ragnaros1.blp` finds `Creature\Ragnaros\RagnarosSkin.blp`.
pub fn find_skin_fallback(
    archive_file_names: &[String],
    wanted_texture_path: &str,
) -> Option<String> {
    let (wanted_directory, wanted_file_name) = wanted_texture_path.rsplit_once('\\')?;
    let wanted_stem = wanted_file_name
        .to_ascii_lowercase()
        .trim_end_matches(".blp")
        .to_string();
    let wanted_base_name = wanted_stem.trim_end_matches(|c: char| c.is_ascii_digit());
    let mut candidates: Vec<&String> = archive_file_names
        .iter()
        .filter(|candidate_path| {
            let Some((candidate_directory, candidate_file_name)) = candidate_path.rsplit_once('\\')
            else {
                return false;
            };
            candidate_directory.eq_ignore_ascii_case(wanted_directory)
                && candidate_file_name
                    .to_ascii_lowercase()
                    .starts_with(wanted_base_name)
                && candidate_file_name.to_ascii_lowercase().ends_with(".blp")
        })
        .collect();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(skin_candidate_score(candidate)));
    candidates.first().map(|candidate| candidate.to_string())
}

/// Headless `wgpu` renderer for CLI screenshots (no window or display).
pub struct Headless {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub renderer: crate::render::Renderer,
}

impl Headless {
    pub fn new() -> Result<Self> {
        let mut instance_descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_descriptor.backends = wgpu::Backends::VULKAN | wgpu::Backends::GL;
        let instance = wgpu::Instance::new(instance_descriptor);
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .context("no GPU adapter available")?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("modelview"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: Default::default(),
            memory_hints: Default::default(),
            trace: Default::default(),
        }))
        .context("creating device")?;
        let renderer = crate::render::Renderer::new(&device);
        Ok(Self {
            device,
            queue,
            renderer,
        })
    }

    #[allow(clippy::too_many_arguments)] // render target + camera are independent inputs
    pub fn screenshot(
        &mut self,
        model: &RenderModel,
        textures: &DecodedTextures,
        width: u32,
        height: u32,
        camera_yaw: f32,
        camera_pitch: f32,
        output_path: &std::path::Path,
    ) -> Result<()> {
        let mut texture_views = HashMap::new();
        for (texture_path, (rgba_pixels, texture_width, texture_height)) in textures {
            texture_views.insert(
                texture_path.clone(),
                self.renderer.upload_texture(
                    &self.device,
                    &self.queue,
                    rgba_pixels,
                    *texture_width,
                    *texture_height,
                ),
            );
        }
        let gpu_model =
            self.renderer
                .upload_model(&self.device, &self.queue, model, &texture_views);
        let color_format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let color_texture = create_render_target(
            &self.device,
            "screenshot-color",
            width,
            height,
            color_format,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let depth_texture = create_render_target(
            &self.device,
            "screenshot-depth",
            width,
            height,
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let camera = crate::render::Camera::new(
            gpu_model.bounding_center,
            gpu_model.bounding_radius,
            camera_yaw,
            camera_pitch,
        );
        let mut command_encoder = self.device.create_command_encoder(&Default::default());
        self.renderer.draw_model(
            &self.device,
            &self.queue,
            &mut command_encoder,
            &color_texture.create_view(&Default::default()),
            &depth_texture.create_view(&Default::default()),
            color_format,
            &gpu_model,
            &camera,
            width,
            height,
        );
        let pixels = read_back_pixels(self, command_encoder, &color_texture, width, height)?;
        let screenshot_image =
            image::RgbaImage::from_raw(width, height, pixels).context("assembling image")?;
        screenshot_image
            .save(output_path)
            .with_context(|| format!("writing {}", output_path.display()))?;
        Ok(())
    }
}

fn create_render_target(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// Copy a rendered texture into CPU memory, stripping row padding.
fn read_back_pixels(
    headless: &Headless,
    mut command_encoder: wgpu::CommandEncoder,
    color_texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    // wgpu requires buffer rows padded to 256-byte multiples.
    let padded_row_byte_count = (width * 4).next_multiple_of(256);
    let readback_buffer = headless.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("screenshot-readback"),
        size: (padded_row_byte_count * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    command_encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: color_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row_byte_count),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    headless.queue.submit([command_encoder.finish()]);
    let buffer_slice = readback_buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    headless
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .context("polling GPU")?;
    receiver
        .recv()
        .context("map callback")?
        .context("mapping readback")?;
    let mapped_bytes = buffer_slice.get_mapped_range().context("reading pixels")?;
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row_index in 0..height {
        let row_start = (row_index * padded_row_byte_count) as usize;
        pixels.extend_from_slice(&mapped_bytes[row_start..row_start + (width * 4) as usize]);
    }
    drop(mapped_bytes);
    readback_buffer.unmap();
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_file_names() -> Vec<String> {
        vec![
            "Creature\\Ragnaros\\RagnarosHammer.blp".to_string(),
            "Creature\\Ragnaros\\RagnarosSkin.blp".to_string(),
            "Creature\\RAGNAROS\\RAGNAROSSKIN.BLP".to_string(),
            "Creature\\Other\\RagnarosSkin.blp".to_string(),
            "Creature\\Ragnaros\\Ragnaros.txt".to_string(),
        ]
    }

    #[test]
    fn first_usable_copy_skips_tombstones() {
        let copies = vec![
            ("patch.MPQ".to_string(), Vec::new()),
            ("model.MPQ".to_string(), b"MD20-good".to_vec()),
        ];
        let (source, bytes, skipped) =
            first_usable_copy(&copies, |copy| copy.starts_with(b"MD20")).expect("usable copy");
        assert_eq!(source, "model.MPQ");
        assert_eq!(bytes, b"MD20-good");
        assert_eq!(skipped, 1);
    }

    #[test]
    fn first_usable_copy_returns_none_when_all_unusable() {
        let copies = vec![("patch.MPQ".to_string(), Vec::new())];
        assert!(first_usable_copy(&copies, |copy| !copy.is_empty()).is_none());
        assert!(first_usable_copy(&[], |_| true).is_none());
    }

    #[test]
    fn decode_blp_rejects_invalid_bytes() {
        assert!(decode_blp_rgba(b"not a blp").is_err());
        assert!(decode_blp_rgba(b"").is_err());
    }

    #[test]
    fn find_skin_fallback_finds_same_directory_skin() {
        let fallback =
            find_skin_fallback(&sample_file_names(), "Creature\\Ragnaros\\Ragnaros1.blp")
                .expect("skin fallback exists");
        assert!(fallback.to_ascii_lowercase().ends_with("ragnarosskin.blp"));
        assert!(fallback.starts_with("Creature\\"));
    }

    #[test]
    fn find_skin_fallback_prefers_light_skin_variants() {
        let names = vec![
            "Creature\\Human\\HumanSkinDark.blp".to_string(),
            "Creature\\Human\\HumanLightSkin.blp".to_string(),
        ];
        let fallback = find_skin_fallback(&names, "Creature\\Human\\Human1.blp").unwrap();
        assert!(fallback.contains("LightSkin"));
    }

    #[test]
    fn find_skin_fallback_returns_none_without_candidates() {
        assert_eq!(
            find_skin_fallback(&sample_file_names(), "Creature\\Missing\\Missing1.blp"),
            None
        );
        assert_eq!(
            find_skin_fallback(&[], "Creature\\Ragnaros\\Ragnaros1.blp"),
            None
        );
        assert_eq!(
            find_skin_fallback(&sample_file_names(), "NoDirectory1.blp"),
            None
        );
    }
}
