use crate::model::{Blend, RenderModel};
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    model_view_projection: [[f32; 4]; 4],
    model_matrix: [[f32; 4]; 4],
    alpha_cutoff: f32,
    unlit: f32,
    _pad: [f32; 2],
}

const SHADER: &str = r#"
struct Uniforms { model_view_projection: mat4x4<f32>, model_matrix: mat4x4<f32>, alpha_cutoff: f32, unlit: f32, pad: vec2<f32> }
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
struct In { @location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32> }
struct Out { @builtin(position) clip: vec4<f32>, @location(0) normal: vec3<f32>, @location(1) uv: vec2<f32> }
@vertex fn vs(v: In) -> Out {
    var o: Out;
    o.clip = u.model_view_projection * vec4<f32>(v.pos, 1.0);
    o.normal = normalize((u.model_matrix * vec4<f32>(v.normal, 0.0)).xyz);
    o.uv = v.uv;
    return o;
}
@fragment fn fs(f: Out) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, f.uv);
    if (c.a < u.alpha_cutoff) { discard; }
    if (u.unlit > 0.5) { return vec4<f32>(c.rgb, c.a); }
    let light_dir = normalize(vec3<f32>(0.4, 0.75, 0.55));
    var n = normalize(f.normal);
    if (dot(n, light_dir) < 0.0) { n = -n; }
    let diff = max(dot(n, light_dir), 0.0);
    let lit = c.rgb * (0.55 + 0.45 * diff);
    return vec4<f32>(lit, c.a);
}
"#;

#[derive(Clone, Copy)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: [f32; 3],
}

impl Camera {
    pub fn new(center: [f32; 3], radius: f32, yaw: f32, pitch: f32) -> Self {
        Self {
            yaw,
            pitch,
            distance: radius * 2.8,
            target: center,
        }
    }
    fn view_projection_matrices(&self, aspect_ratio: f32) -> ([[f32; 4]; 4], [[f32; 4]; 4]) {
        let (yaw_sin, yaw_cos) = self.yaw.sin_cos();
        let (pitch_sin, pitch_cos) = self.pitch.sin_cos();
        let view_direction = glam::Vec3::new(yaw_cos * pitch_cos, yaw_sin * pitch_cos, pitch_sin);
        let target = glam::Vec3::from_array(self.target);
        let eye = target + view_direction * self.distance;
        let view = look_at(eye, target, glam::Vec3::Y);
        let projection = perspective(45f32.to_radians(), aspect_ratio.max(0.01), 0.05, 5000.0);
        let model = glam::Mat4::IDENTITY;
        (
            (projection * view * model).to_cols_array_2d(),
            model.to_cols_array_2d(),
        )
    }
}

struct GpuBatch {
    indices: wgpu::Buffer,
    count: u32,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    unlit: bool,
    blend: Blend,
    double_sided: bool,
    depth_center: f32,
}

pub struct GpuModel {
    vertex_buf: wgpu::Buffer,
    batches: Vec<GpuBatch>,
    pub bounding_center: [f32; 3],
    pub bounding_radius: f32,
}

fn wgpu_blend_state(blend: Blend) -> Option<wgpu::BlendState> {
    match blend {
        Blend::Opaque | Blend::AlphaKey => None,
        _ => Some(wgpu::BlendState::ALPHA_BLENDING),
    }
}

pub struct Renderer {
    pipeline_cache: HashMap<(bool, bool), wgpu::RenderPipeline>,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    shader: wgpu::ShaderModule,
    fallback_view: wgpu::TextureView,
}

impl Renderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("m2"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("m2-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let fallback_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("magenta"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let fallback_view = fallback_texture.create_view(&Default::default());
        Self {
            pipeline_cache: HashMap::new(),
            layout,
            sampler,
            shader,
            fallback_view,
        }
    }

    fn get_or_create_pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        blend: Blend,
        double_sided: bool,
    ) -> wgpu::RenderPipeline {
        let transparent = !matches!(blend, Blend::Opaque | Blend::AlphaKey);
        let key = (transparent, double_sided);
        if let Some(p) = self.pipeline_cache.get(&key) {
            return p.clone();
        }
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&self.layout)],
            immediate_size: 0,
        });
        let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("m2-pipe"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 24,
                            shader_location: 2,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: wgpu_blend_state(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: if double_sided {
                    None
                } else {
                    Some(wgpu::Face::Back)
                },
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(!transparent),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let _ = transparent;
        self.pipeline_cache.insert(key, p.clone());
        p
    }

    pub fn upload_model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &RenderModel,
        textures: &HashMap<String, wgpu::TextureView>,
    ) -> GpuModel {
        let gpu_vertices: Vec<Vertex> = model
            .positions
            .iter()
            .enumerate()
            .map(|(vertex_index, position)| {
                convert_to_gpu_vertex(
                    position,
                    &model.normals[vertex_index],
                    model.uvs[vertex_index],
                )
            })
            .collect();
        let vertex_buf = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("verts"),
                contents: bytemuck::cast_slice(&gpu_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            },
        );
        let mut batches = Vec::new();
        for batch in &model.batches {
            let indices = wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some("idx"),
                    contents: bytemuck::cast_slice(&batch.indices),
                    usage: wgpu::BufferUsages::INDEX,
                },
            );
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("uni"),
                size: std::mem::size_of::<Uniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let texture_view = batch
                .texture_path
                .as_ref()
                .and_then(|t| textures.get(t))
                .unwrap_or(&self.fallback_view);
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            batches.push(GpuBatch {
                indices,
                count: batch.indices.len() as u32,
                bind,
                uniform,
                unlit: batch.unlit,
                blend: batch.blend,
                double_sided: batch.double_sided,
                depth_center: batch.depth_center,
            });
            let _ = queue;
        }
        GpuModel {
            vertex_buf,
            batches,
            bounding_center: [
                model.bounding_center[0],
                model.bounding_center[2],
                -model.bounding_center[1],
            ],
            bounding_radius: model.bounding_radius,
        }
    }

    pub fn upload_texture(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba_pixels: &[u8],
        texture_width: u32,
        texture_height: u32,
    ) -> wgpu::TextureView {
        // Base level only in v1 (documented PRD choice); a single mip level
        // avoids sampling uninitialized smaller levels.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("blp"),
            size: wgpu::Extent3d {
                width: texture_width,
                height: texture_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba_pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * texture_width),
                rows_per_image: Some(texture_height),
            },
            wgpu::Extent3d {
                width: texture_width,
                height: texture_height,
                depth_or_array_layers: 1,
            },
        );
        texture.create_view(&Default::default())
    }

    #[allow(clippy::too_many_arguments)] // GPU handles are passed explicitly; bundling them hides ownership
    pub fn draw_model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        gpu: &GpuModel,
        camera: &Camera,
        width: u32,
        height: u32,
    ) {
        let aspect = width as f32 / height.max(1) as f32;
        // Respect the caller's target (GUI pan offsets it from the model center).
        let active_camera = *camera;
        let (model_view_projection, model_matrix) = active_camera.view_projection_matrices(aspect);
        let mut draw_order: Vec<usize> = (0..gpu.batches.len()).collect();
        draw_order.sort_by(|&first, &second| {
            let first_is_opaque =
                matches!(gpu.batches[first].blend, Blend::Opaque | Blend::AlphaKey);
            let second_is_opaque =
                matches!(gpu.batches[second].blend, Blend::Opaque | Blend::AlphaKey);
            second_is_opaque.cmp(&first_is_opaque).then(
                gpu.batches[first]
                    .depth_center
                    .partial_cmp(&gpu.batches[second].depth_center)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        // Per-batch uniforms written through a fresh buffer each draw (models are small).
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("m2-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.08,
                        g: 0.09,
                        b: 0.11,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        for &batch_index in &draw_order {
            let batch = &gpu.batches[batch_index];
            let alpha_cutoff = if matches!(batch.blend, Blend::AlphaKey) {
                0.5
            } else {
                0.0
            };
            queue.write_buffer(
                &batch.uniform,
                0,
                bytemuck::bytes_of(&Uniforms {
                    model_view_projection,
                    model_matrix,
                    alpha_cutoff,
                    unlit: if batch.unlit { 1.0 } else { 0.0 },
                    _pad: [0.0; 2],
                }),
            );
        }
        pass.set_vertex_buffer(0, gpu.vertex_buf.slice(..));
        for &batch_index in &draw_order {
            let batch = &gpu.batches[batch_index];
            let pipe = self.get_or_create_pipeline(device, format, batch.blend, batch.double_sided);
            pass.set_pipeline(&pipe);
            pass.set_bind_group(0, &batch.bind, &[]);
            pass.set_index_buffer(batch.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..batch.count, 0, 0..1);
        }
        drop(pass);
        let _ = queue;
    }
}

fn look_at(eye: glam::Vec3, target: glam::Vec3, world_up: glam::Vec3) -> glam::Mat4 {
    let forward = (target - eye).normalize();
    let side = forward.cross(world_up).normalize();
    let corrected_up = side.cross(forward);
    glam::Mat4::from_cols(
        glam::Vec4::new(side.x, corrected_up.x, -forward.x, 0.0),
        glam::Vec4::new(side.y, corrected_up.y, -forward.y, 0.0),
        glam::Vec4::new(side.z, corrected_up.z, -forward.z, 0.0),
        glam::Vec4::new(
            -side.dot(eye),
            -corrected_up.dot(eye),
            forward.dot(eye),
            1.0,
        ),
    )
}

fn perspective(
    vertical_fov: f32,
    aspect_ratio: f32,
    near_plane: f32,
    far_plane: f32,
) -> glam::Mat4 {
    let focal_length = 1.0 / (vertical_fov * 0.5).tan();
    // wgpu clip space: depth 0..1.
    glam::Mat4::from_cols(
        glam::Vec4::new(focal_length / aspect_ratio, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, focal_length, 0.0, 0.0),
        glam::Vec4::new(0.0, 0.0, far_plane / (near_plane - far_plane), -1.0),
        glam::Vec4::new(
            0.0,
            0.0,
            (near_plane * far_plane) / (near_plane - far_plane),
            0.0,
        ),
    )
}

/// Convert one WoW-space vertex (Z-up, right-handed) to GPU space (Y-up):
/// (x, y, z) -> (x, z, -y). Normals get the same rotation.
fn convert_to_gpu_vertex(position: &[f32; 3], normal: &[f32; 3], uv: [f32; 2]) -> Vertex {
    Vertex {
        position: [position[0], position[2], -position[1]],
        normal: [normal[0], normal[2], -normal[1]],
        uv,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_frames_model_at_proportional_distance() {
        let camera = Camera::new([1.0, 2.0, 3.0], 10.0, 0.5, 0.25);
        assert_eq!(camera.target, [1.0, 2.0, 3.0]);
        assert!((camera.distance - 28.0).abs() < 1e-5);
    }

    #[test]
    fn gpu_vertex_conversion_swaps_up_axis() {
        let vertex = convert_to_gpu_vertex(&[1.0, 2.0, 3.0], &[0.0, 0.0, 1.0], [0.25, 0.75]);
        assert_eq!(vertex.position, [1.0, 3.0, -2.0]);
        assert_eq!(vertex.normal, [0.0, 1.0, 0.0]);
        assert_eq!(vertex.uv, [0.25, 0.75]);
    }

    #[test]
    fn blend_state_only_enables_blending_for_transparent_modes() {
        assert!(wgpu_blend_state(Blend::Opaque).is_none());
        assert!(wgpu_blend_state(Blend::AlphaKey).is_none());
        assert!(wgpu_blend_state(Blend::Alpha).is_some());
        assert!(wgpu_blend_state(Blend::Other(4)).is_some());
    }

    #[test]
    fn perspective_matrix_maps_near_and_far_planes() {
        let projection = perspective(45f32.to_radians(), 1.0, 0.05, 5000.0);
        // Center of the near plane must land at depth 0, far plane at depth 1 (wgpu clip space).
        let near_clip = projection * glam::Vec4::new(0.0, 0.0, -0.05, 1.0);
        let far_clip = projection * glam::Vec4::new(0.0, 0.0, -5000.0, 1.0);
        assert!((near_clip.z / near_clip.w).abs() < 1e-4);
        assert!(((far_clip.z / far_clip.w) - 1.0).abs() < 1e-3);
    }
}
