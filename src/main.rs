use anyhow::{Context, Result, bail};
use modelview::mpq::DataDir;
use std::path::PathBuf;

struct Cli {
    data: Option<PathBuf>,
    model: Option<String>,
    screenshot: Option<PathBuf>,
    width: u32,
    height: u32,
    yaw: f32,
    pitch: f32,
}

fn parse_cli() -> Result<Cli> {
    let mut cli = Cli {
        data: None,
        model: None,
        screenshot: None,
        width: 1024,
        height: 768,
        yaw: 0.6,
        pitch: 0.35,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                println!(
                    "Usage: modelview --data <Data-dir> [--model <internal M2 path>] [--screenshot out.png] [--width W --height H --angle yaw,pitch]\nWithout --screenshot, opens the GUI browser."
                );
                std::process::exit(0);
            }
            "--data" => {
                i += 1;
                cli.data = Some(PathBuf::from(args.get(i).context("--data needs a value")?));
            }
            "--model" => {
                i += 1;
                cli.model = Some(args.get(i).context("--model needs a value")?.clone());
            }
            "--screenshot" => {
                i += 1;
                cli.screenshot = Some(PathBuf::from(
                    args.get(i).context("--screenshot needs a value")?,
                ));
            }
            "--width" => {
                i += 1;
                cli.width = args
                    .get(i)
                    .context("--width needs a value")?
                    .parse()
                    .context("bad --width")?;
            }
            "--height" => {
                i += 1;
                cli.height = args
                    .get(i)
                    .context("--height needs a value")?
                    .parse()
                    .context("bad --height")?;
            }
            "--angle" => {
                i += 1;
                let v = args.get(i).context("--angle needs yaw,pitch")?;
                let mut parts = v.split(',');
                cli.yaw = parts
                    .next()
                    .context("bad --angle")?
                    .trim()
                    .parse()
                    .context("bad yaw")?;
                cli.pitch = parts
                    .next()
                    .context("bad --angle")?
                    .trim()
                    .parse()
                    .context("bad pitch")?;
            }
            other => bail!("unknown argument: {other} (try --help)"),
        }
        i += 1;
    }
    Ok(cli)
}

fn run_headless(cli: &Cli) -> Result<()> {
    let data_dir = cli.data.clone().context("--data is required")?;
    let internal = cli
        .model
        .clone()
        .context("--model is required with --screenshot")?;
    let out = cli.screenshot.clone().unwrap();
    let mut data = DataDir::open(&data_dir)?;
    let (model, textures, warnings) = modelview::app::load_model(&mut data, &internal)?;
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    let mut headless = modelview::app::Headless::new()?;
    headless.screenshot(
        &model, &textures, cli.width, cli.height, cli.yaw, cli.pitch, &out,
    )?;
    println!(
        "wrote {} ({} batches, {} textures)",
        out.display(),
        model.batches.len(),
        textures.len()
    );
    Ok(())
}

fn main() {
    let cli = match parse_cli() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(2);
        }
    };
    if cli.screenshot.is_some() {
        if let Err(e) = run_headless(&cli) {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(e) = run_gui(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run_gui(cli: Cli) -> Result<()> {
    let data_dir = cli
        .data
        .context("--data <Data-dir> is required (or use --screenshot for headless mode)")?;
    let mut data = DataDir::open(&data_dir)?;
    let models = data.list_models()?;
    let app = gui::ViewerApp::new(data, models, cli.model);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("WoW 1.12.1 Model Viewer"),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("modelview", options, Box::new(|_cc| Ok(Box::new(app))))
        .map_err(|e| anyhow::anyhow!("GUI failed: {e}"))
}

mod gui {
    use modelview::mpq::DataDir;
    use modelview::render::{Camera, GpuModel, Renderer};
    use std::collections::HashMap;

    pub struct ViewerApp {
        data: Option<DataDir>,
        models: Vec<String>,
        filter: String,
        selected: Option<String>,
        status: String,
        renderer: Option<Renderer>,
        gpu: Option<GpuModel>,
        render_target: Option<(
            u32,
            u32,
            wgpu::Texture,
            wgpu::Texture,
            eframe::egui::TextureId,
        )>,
        pending: Option<modelview::model::RenderModel>,
        textures: HashMap<String, (Vec<u8>, u32, u32)>,
        yaw: f32,
        pitch: f32,
        distance_scale: f32,
        pan: [f32; 3],
        screenshot_request: bool,
    }

    impl ViewerApp {
        pub fn new(data: DataDir, models: Vec<String>, initial: Option<String>) -> Self {
            let mut app = Self {
                status: format!("{} models", models.len()),
                data: Some(data),
                models,
                filter: String::new(),
                selected: None,
                renderer: None,
                gpu: None,
                render_target: None,
                pending: None,
                textures: HashMap::new(),
                yaw: 0.6,
                pitch: 0.35,
                distance_scale: 1.0,
                pan: [0.0; 3],
                screenshot_request: false,
            };
            if let Some(m) = initial {
                app.load(&m);
            }
            app
        }

        fn load(&mut self, internal: &str) {
            let Some(data) = self.data.as_mut() else {
                return;
            };
            match modelview::app::load_model(data, internal) {
                Ok((model, textures, warnings)) => {
                    self.status = format!(
                        "{} — {} batches, {} textures{}",
                        model.name,
                        model.batches.len(),
                        textures.len(),
                        if warnings.is_empty() {
                            String::new()
                        } else {
                            format!(" ({} warnings)", warnings.len())
                        }
                    );
                    self.textures = textures;
                    self.selected = Some(internal.to_string());
                    self.gpu = None;
                    self.pan = [0.0; 3];
                    self.distance_scale = 1.0;
                    // Stash CPU model for upload on next frame via pending field reuse.
                    self.pending = Some(model);
                }
                Err(e) => self.status = format!("failed to load {internal}: {e:#}"),
            }
        }
    }

    // Pending CPU model awaiting GPU upload.
    // (Declared via extension field below.)

    impl eframe::App for ViewerApp {
        fn ui(&mut self, ui: &mut eframe::egui::Ui, frame: &mut eframe::Frame) {
            let ctx = ui.ctx().clone();
            eframe::egui::Panel::left("browser")
                .resizable(true)
                .default_size(340.0)
                .show(ui, |ui| {
                    ui.heading("Models");
                    ui.text_edit_singleline(&mut self.filter);
                    ui.label(&self.status);
                    if ui.button("Screenshot PNG").clicked() {
                        self.screenshot_request = true;
                    }
                    ui.separator();
                    let filter = self.filter.to_ascii_lowercase();
                    let mut clicked: Option<String> = None;
                    eframe::egui::ScrollArea::vertical().show(ui, |ui| {
                        for m in &self.models {
                            if !filter.is_empty() && !m.to_ascii_lowercase().contains(&filter) {
                                continue;
                            }
                            let is_sel = self.selected.as_deref() == Some(m.as_str());
                            if ui.selectable_label(is_sel, m).clicked() {
                                clicked = Some(m.clone());
                            }
                        }
                    });
                    if let Some(m) = clicked {
                        self.load(&m);
                    }
                });

            eframe::egui::CentralPanel::default().show(ui, |ui| {
                let (rect, response) = ui.allocate_exact_size(ui.available_size(), eframe::egui::Sense::drag());
                if response.dragged_by(eframe::egui::PointerButton::Primary) {
                    let d = response.drag_delta();
                    self.yaw -= d.x * 0.008;
                    self.pitch = (self.pitch + d.y * 0.008).clamp(-1.4, 1.4);
                }
                if response.dragged_by(eframe::egui::PointerButton::Secondary) || response.dragged_by(eframe::egui::PointerButton::Middle) {
                    let d = response.drag_delta();
                    self.pan[0] -= d.x * 0.01;
                    self.pan[1] += d.y * 0.01;
                }
                if response.hovered() {
                    let zoom = ctx.input(|i| i.smooth_scroll_delta.y);
                    if zoom != 0.0 {
                        self.distance_scale = (self.distance_scale * (1.0 - zoom * 0.001)).clamp(0.05, 20.0);
                    }
                }
                if ctx.input(|i| i.key_pressed(eframe::egui::Key::R)) {
                    self.yaw = 0.6;
                    self.pitch = 0.35;
                    self.distance_scale = 1.0;
                    self.pan = [0.0; 3];
                }

                if let Some(state) = frame.wgpu_render_state() {
                    if self.renderer.is_none() {
                        self.renderer = Some(Renderer::new(&state.device));
                    }
                    if let Some(model) = self.pending.take() {
                        let mut views = HashMap::new();
                        if let Some(renderer) = self.renderer.as_ref() {
                            for (name, (rgba, w, h)) in &self.textures {
                                views.insert(name.clone(), renderer.upload_texture(&state.device, &state.queue, rgba, *w, *h));
                            }
                        }
                        if let Some(renderer) = self.renderer.as_mut() {
                            self.gpu = Some(renderer.upload(&state.device, &state.queue, &model, &views));
                        }
                    }
                    if let (Some(renderer), Some(gpu)) = (self.renderer.as_mut(), self.gpu.as_ref()) {
                        let w = (rect.width() as u32).max(1);
                        let h = (rect.height() as u32).max(1);
                        let format = state.target_format;
                        // Reuse the offscreen target across frames; recreating it (and
                        // registering a new egui texture) every frame leaks GPU memory.
                        let stale = self.render_target.as_ref().map(|(tw, th, ..)| (*tw, *th)) != Some((w, h));
                        if stale {
                            if let Some((_, _, _, _, old_id)) = self.render_target.take() {
                                state.renderer.write().free_texture(&old_id);
                            }
                            let color = state.device.create_texture(&wgpu::TextureDescriptor {
                                label: Some("gui-color"), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
                                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
                            });
                            let depth = state.device.create_texture(&wgpu::TextureDescriptor {
                                label: Some("gui-depth"), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                                format: wgpu::TextureFormat::Depth32Float, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
                            });
                            let view = color.create_view(&Default::default());
                            let tex_id = state.renderer.write().register_native_texture(&state.device, &view, wgpu::FilterMode::Linear);
                            self.render_target = Some((w, h, color, depth, tex_id));
                        }
                        let (_, _, color, depth, tex_id) = self.render_target.as_ref().unwrap();
                        let mut camera = Camera::new(gpu.center, gpu.radius, self.yaw, self.pitch);
                        camera.distance *= self.distance_scale;
                        camera.target = [gpu.center[0] + self.pan[0], gpu.center[1] + self.pan[1], gpu.center[2]];
                        let mut encoder = state.device.create_command_encoder(&Default::default());
                        renderer.draw(&state.device, &state.queue, &mut encoder, &color.create_view(&Default::default()), &depth.create_view(&Default::default()), format, gpu, &camera, w, h);
                        state.queue.submit([encoder.finish()]);
                        ui.painter().image(*tex_id, rect, eframe::egui::Rect::from_min_max(eframe::egui::pos2(0.0, 0.0), eframe::egui::pos2(1.0, 1.0)), eframe::egui::Color32::WHITE);
                        if self.screenshot_request {
                            self.screenshot_request = false;
                            self.status = "Use CLI --screenshot for PNG export (GUI capture not wired)".into();
                        }
                        ctx.request_repaint();
                        return;
                    }
                }
                ui.centered_and_justified(|ui| {
                    ui.label("Select a model from the browser.\nDrag to orbit, wheel to zoom, right-drag to pan, R to reset.");
                });
            });
        }
    }
}
