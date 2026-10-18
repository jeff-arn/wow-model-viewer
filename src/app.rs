use crate::model::RenderModel;
use crate::mpq::DataDir;
use anyhow::{Context, Result};
use std::collections::HashMap;

pub fn decode_blp_rgba(data: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let img = wow_blp::parser::load_blp_from_buf(data).context("parsing BLP")?;
    let dyn_img = wow_blp::convert::blp_to_image(&img, 0).context("decoding BLP")?;
    let rgba = dyn_img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((rgba.into_raw(), w, h))
}

/// CPU-decoded textures keyed by internal BLP path: (RGBA8 pixels, width, height).
pub type DecodedTextures = HashMap<String, (Vec<u8>, u32, u32)>;

/// A parsed model plus its decoded textures and non-fatal load warnings.
pub type LoadedModel = (RenderModel, DecodedTextures, Vec<String>);

pub fn load_model(data: &mut DataDir, internal: &str) -> Result<LoadedModel> {
    let bytes = data.read_file(internal)?;
    let model = crate::model::parse_m2(&bytes, internal)?;
    let mut textures = HashMap::new();
    let mut warnings = Vec::new();
    let mut wanted: Vec<String> = model
        .batches
        .iter()
        .filter_map(|b| b.texture.clone())
        .collect();
    wanted.sort();
    wanted.dedup();
    let mut all_names: Option<Vec<String>> = None;
    for tex in wanted {
        match data.read_file(&tex) {
            Ok(raw) => match decode_blp_rgba(&raw) {
                Ok(decoded) => {
                    textures.insert(tex, decoded);
                }
                Err(e) => warnings.push(format!("texture {tex}: {e:#}")),
            },
            Err(_) => {
                // Creature skins use variant names (e.g. RagnarosSkin.blp), not the
                // derived Name1.blp guess. Fall back to a same-directory skin file.
                if all_names.is_none() {
                    all_names = data.list_all().ok();
                }
                let fallback = all_names
                    .as_ref()
                    .and_then(|names| find_skin_fallback(names, &tex));
                match fallback.and_then(|f| data.read_file(&f).ok().map(|r| (f, r))) {
                    Some((found, raw)) => match decode_blp_rgba(&raw) {
                        Ok(decoded) => {
                            warnings.push(format!("texture {tex} -> using {found}"));
                            textures.insert(tex, decoded);
                        }
                        Err(e) => warnings.push(format!("texture {tex}: {e:#}")),
                    },
                    None => warnings.push(format!("texture not found: {tex}")),
                }
            }
        }
    }
    Ok((model, textures, warnings))
}

fn find_skin_fallback(names: &[String], wanted: &str) -> Option<String> {
    let (dir, file) = wanted.rsplit_once('\\')?;
    let stem = file
        .to_ascii_lowercase()
        .trim_end_matches(".blp")
        .to_string();
    let base = stem.trim_end_matches(|c: char| c.is_ascii_digit());
    let mut candidates: Vec<&String> = names
        .iter()
        .filter(|n| {
            let Some((d, f)) = n.rsplit_once('\\') else {
                return false;
            };
            d.eq_ignore_ascii_case(dir)
                && f.to_ascii_lowercase()
                    .starts_with(&base.to_ascii_lowercase())
                && f.to_ascii_lowercase().ends_with(".blp")
        })
        .collect();
    candidates.sort_by_key(|f| {
        let lower = f.to_ascii_lowercase();
        let score = (if lower.contains("light") { 4 } else { 0 })
            + (if lower.contains("skin") { 2 } else { 0 });
        std::cmp::Reverse(score)
    });
    candidates.first().map(|c| c.to_string())
}

pub struct Headless {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub renderer: crate::render::Renderer,
}

impl Headless {
    pub fn new() -> Result<Self> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::VULKAN | wgpu::Backends::GL;
        let instance = wgpu::Instance::new(desc);
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
        yaw: f32,
        pitch: f32,
        out: &std::path::Path,
    ) -> Result<()> {
        let mut views = HashMap::new();
        for (name, (rgba, w, h)) in textures {
            views.insert(
                name.clone(),
                self.renderer
                    .upload_texture(&self.device, &self.queue, rgba, *w, *h),
            );
        }
        let gpu = self
            .renderer
            .upload(&self.device, &self.queue, model, &views);
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let color_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shot"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let camera = crate::render::Camera::new(gpu.center, gpu.radius, yaw, pitch);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.renderer.draw(
            &self.device,
            &self.queue,
            &mut encoder,
            &color_tex.create_view(&Default::default()),
            &depth_tex.create_view(&Default::default()),
            format,
            &gpu,
            &camera,
            width,
            height,
        );
        let padded_row = (width * 4).next_multiple_of(256);
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &color_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .context("polling GPU")?;
        rx.recv()
            .context("map callback")?
            .context("mapping readback")?;
        let data = slice.get_mapped_range().context("reading pixels")?;
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let start = (row * padded_row) as usize;
            pixels.extend_from_slice(&data[start..start + (width * 4) as usize]);
        }
        drop(data);
        buf.unmap();
        let img = image::RgbaImage::from_raw(width, height, pixels).context("assembling image")?;
        img.save(out)
            .with_context(|| format!("writing {}", out.display()))?;
        Ok(())
    }
}
