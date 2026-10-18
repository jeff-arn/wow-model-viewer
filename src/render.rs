use crate::model::{Blend, RenderModel};
use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    alpha_cutoff: f32,
    unlit: f32,
    _pad: [f32; 2],
}

const SHADER: &str = r#"
struct Uniforms { mvp: mat4x4<f32>, model: mat4x4<f32>, alpha_cutoff: f32, unlit: f32, pad: vec2<f32> }
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
struct In { @location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) uv: vec2<f32> }
struct Out { @builtin(position) clip: vec4<f32>, @location(0) normal: vec3<f32>, @location(1) uv: vec2<f32> }
@vertex fn vs(v: In) -> Out {
    var o: Out;
    o.clip = u.mvp * vec4<f32>(v.pos, 1.0);
    o.normal = normalize((u.model * vec4<f32>(v.normal, 0.0)).xyz);
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
    fn matrices(&self, aspect: f32) -> ([[f32; 4]; 4], [[f32; 4]; 4]) {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let dir = glam::Vec3::new(cy * cp, sy * cp, sp);
        let target = glam::Vec3::from_array(self.target);
        let eye = target + dir * self.distance;
        let view = look_at(eye, target, glam::Vec3::Y);
        let proj = perspective(45f32.to_radians(), aspect.max(0.01), 0.05, 5000.0);
        let model = glam::Mat4::IDENTITY;
        (
            (proj * view * model).to_cols_array_2d(),
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
    pub center: [f32; 3],
    pub radius: f32,
}

fn blend_state(blend: Blend) -> Option<wgpu::BlendState> {
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
        let tex = device.create_texture(&wgpu::TextureDescriptor {
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
        let fallback_view = tex.create_view(&Default::default());
        Self {
            pipeline_cache: HashMap::new(),
            layout,
            sampler,
            shader,
            fallback_view,
        }
    }

    fn pipeline(
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
                    blend: blend_state(blend),
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

    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &RenderModel,
        textures: &HashMap<String, wgpu::TextureView>,
    ) -> GpuModel {
        let verts: Vec<Vertex> = model
            .positions
            .iter()
            .enumerate()
            .map(|(i, p)| Vertex {
                pos: [p[0], p[2], -p[1]],
                normal: [
                    model.normals[i][0],
                    model.normals[i][2],
                    -model.normals[i][1],
                ],
                uv: model.uvs[i],
            })
            .collect();
        let vertex_buf = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("verts"),
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX,
            },
        );
        let mut batches = Vec::new();
        for b in &model.batches {
            let indices = wgpu::util::DeviceExt::create_buffer_init(
                device,
                &wgpu::util::BufferInitDescriptor {
                    label: Some("idx"),
                    contents: bytemuck::cast_slice(&b.indices),
                    usage: wgpu::BufferUsages::INDEX,
                },
            );
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("uni"),
                size: std::mem::size_of::<Uniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let view = b
                .texture
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
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            batches.push(GpuBatch {
                indices,
                count: b.indices.len() as u32,
                bind,
                uniform,
                unlit: b.unlit,
                blend: b.blend,
                double_sided: b.double_sided,
                depth_center: b.depth_center,
            });
            let _ = queue;
        }
        GpuModel {
            vertex_buf,
            batches,
            center: [model.center[0], model.center[2], -model.center[1]],
            radius: model.radius,
        }
    }

    pub fn upload_texture(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> wgpu::TextureView {
        // Base level only in v1 (documented PRD choice); single mip avoids sampling empty levels.
        let mip_count = 1;
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("blp"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: mip_count.max(1),
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
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        generate_mips(device, queue, &tex, w, h, mip_count.max(1));
        tex.create_view(&Default::default())
    }

    #[allow(clippy::too_many_arguments)] // GPU handles are passed explicitly; bundling them hides ownership
    pub fn draw(
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
        let cam = *camera;
        let (mvp, model_m) = cam.matrices(aspect);
        let mut order: Vec<usize> = (0..gpu.batches.len()).collect();
        order.sort_by(|&a, &b| {
            let ta = matches!(gpu.batches[a].blend, Blend::Opaque | Blend::AlphaKey);
            let tb = matches!(gpu.batches[b].blend, Blend::Opaque | Blend::AlphaKey);
            tb.cmp(&ta).then(
                gpu.batches[a]
                    .depth_center
                    .partial_cmp(&gpu.batches[b].depth_center)
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
        for &bi in &order {
            let b = &gpu.batches[bi];
            let cutoff = if matches!(b.blend, Blend::AlphaKey) {
                0.5
            } else {
                0.0
            };
            queue.write_buffer(
                &b.uniform,
                0,
                bytemuck::bytes_of(&Uniforms {
                    mvp,
                    model: model_m,
                    alpha_cutoff: cutoff,
                    unlit: if b.unlit { 1.0 } else { 0.0 },
                    _pad: [0.0; 2],
                }),
            );
        }
        pass.set_vertex_buffer(0, gpu.vertex_buf.slice(..));
        for &bi in &order {
            let b = &gpu.batches[bi];
            let pipe = self.pipeline(device, format, b.blend, b.double_sided);
            pass.set_pipeline(&pipe);
            pass.set_bind_group(0, &b.bind, &[]);
            pass.set_index_buffer(b.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..b.count, 0, 0..1);
        }
        drop(pass);
        let _ = queue;
    }
}

fn look_at(eye: glam::Vec3, target: glam::Vec3, up: glam::Vec3) -> glam::Mat4 {
    let f = (target - eye).normalize();
    let s = f.cross(up).normalize();
    let u = s.cross(f);
    glam::Mat4::from_cols(
        glam::Vec4::new(s.x, u.x, -f.x, 0.0),
        glam::Vec4::new(s.y, u.y, -f.y, 0.0),
        glam::Vec4::new(s.z, u.z, -f.z, 0.0),
        glam::Vec4::new(-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
    )
}

fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> glam::Mat4 {
    let f = 1.0 / (fov_y * 0.5).tan();
    // wgpu clip space: depth 0..1.
    glam::Mat4::from_cols(
        glam::Vec4::new(f / aspect, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, f, 0.0, 0.0),
        glam::Vec4::new(0.0, 0.0, far / (near - far), -1.0),
        glam::Vec4::new(0.0, 0.0, (near * far) / (near - far), 0.0),
    )
}

fn generate_mips(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    w: u32,
    h: u32,
    mips: u32,
) {
    if mips <= 1 {
        return;
    }
    // Simple CPU-side box filter would need readback; instead use wgpu's blit via render passes
    // is disproportionate here. Base level only is the documented v1 choice; extra levels
    // stay allocated but sampling clamps visually acceptably for the review tool.
    let _ = (device, queue, tex, w, h);
}
