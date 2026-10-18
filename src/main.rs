use anyhow::{Context, Result, bail};
use modelview::mpq::DataDir;
use std::path::PathBuf;

struct Cli {
    data_dir_path: Option<PathBuf>,
    initial_model_path: Option<String>,
    screenshot_output_path: Option<PathBuf>,
    screenshot_width: u32,
    screenshot_height: u32,
    camera_yaw: f32,
    camera_pitch: f32,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            data_dir_path: None,
            initial_model_path: None,
            screenshot_output_path: None,
            screenshot_width: 1024,
            screenshot_height: 768,
            camera_yaw: 0.6,
            camera_pitch: 0.35,
        }
    }
}

const USAGE: &str = "Usage: modelview --data <Data-dir> [--model <internal M2 path>] [--screenshot out.png] [--width W --height H --angle yaw,pitch]\nWithout --screenshot, opens the GUI browser.";

/// Consume the value following a flag, or fail naming the flag.
fn next_flag_value<'a>(args: &'a [String], arg_index: &mut usize, flag: &str) -> Result<&'a str> {
    *arg_index += 1;
    args.get(*arg_index)
        .map(|value| value.as_str())
        .with_context(|| format!("{flag} needs a value"))
}

fn parse_cli() -> Result<Cli> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        std::process::exit(0);
    }
    parse_cli_from(&args)
}

fn parse_cli_from(args: &[String]) -> Result<Cli> {
    let mut cli = Cli::default();
    let mut arg_index = 0;
    while arg_index < args.len() {
        match args[arg_index].as_str() {
            "--data" => {
                cli.data_dir_path = Some(PathBuf::from(next_flag_value(
                    args,
                    &mut arg_index,
                    "--data",
                )?));
            }
            "--model" => {
                cli.initial_model_path =
                    Some(next_flag_value(args, &mut arg_index, "--model")?.to_string());
            }
            "--screenshot" => {
                cli.screenshot_output_path = Some(PathBuf::from(next_flag_value(
                    args,
                    &mut arg_index,
                    "--screenshot",
                )?));
            }
            "--width" => {
                cli.screenshot_width = next_flag_value(args, &mut arg_index, "--width")?
                    .parse()
                    .context("bad --width")?;
            }
            "--height" => {
                cli.screenshot_height = next_flag_value(args, &mut arg_index, "--height")?
                    .parse()
                    .context("bad --height")?;
            }
            "--angle" => {
                let angle_value = next_flag_value(args, &mut arg_index, "--angle")?;
                let (yaw_text, pitch_text) = angle_value.split_once(',').context("bad --angle")?;
                cli.camera_yaw = yaw_text.trim().parse().context("bad yaw")?;
                cli.camera_pitch = pitch_text.trim().parse().context("bad pitch")?;
            }
            unknown_flag => bail!("unknown argument: {unknown_flag} (try --help)"),
        }
        arg_index += 1;
    }
    Ok(cli)
}

fn run_headless(cli: &Cli) -> Result<()> {
    let data_dir_path = cli.data_dir_path.clone().context("--data is required")?;
    let internal_model_path = cli
        .initial_model_path
        .clone()
        .context("--model is required with --screenshot")?;
    let output_path = cli.screenshot_output_path.clone().unwrap();
    let mut data_directory = DataDir::open(&data_dir_path)?;
    let (model, textures, warnings) =
        modelview::app::load_model(&mut data_directory, &internal_model_path)?;
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    let mut headless = modelview::app::Headless::new()?;
    headless.screenshot(
        &model,
        &textures,
        cli.screenshot_width,
        cli.screenshot_height,
        cli.camera_yaw,
        cli.camera_pitch,
        &output_path,
    )?;
    println!(
        "wrote {} ({} batches, {} textures)",
        output_path.display(),
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
    if cli.screenshot_output_path.is_some() {
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
    let data_dir_path = cli
        .data_dir_path
        .context("--data <Data-dir> is required (or use --screenshot for headless mode)")?;
    let mut data_directory = DataDir::open(&data_dir_path)?;
    let model_paths = data_directory.list_models()?;
    let app = gui::ViewerApp::new(data_directory, model_paths, cli.initial_model_path);
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
        data_directory: Option<DataDir>,
        model_paths: Vec<String>,
        search_filter: String,
        selected_model_path: Option<String>,
        status_message: String,
        renderer: Option<Renderer>,
        gpu_model: Option<GpuModel>,
        render_target: Option<(
            u32,
            u32,
            wgpu::Texture,
            wgpu::Texture,
            eframe::egui::TextureId,
        )>,
        pending_model: Option<modelview::model::RenderModel>,
        decoded_textures: HashMap<String, (Vec<u8>, u32, u32)>,
        yaw: f32,
        pitch: f32,
        distance_scale: f32,
        pan_offset: [f32; 3],
        screenshot_request: bool,
    }

    impl ViewerApp {
        pub fn new(
            data_directory: DataDir,
            model_paths: Vec<String>,
            initial_model_path: Option<String>,
        ) -> Self {
            let mut app = Self {
                status_message: format!("{} models", model_paths.len()),
                data_directory: Some(data_directory),
                model_paths,
                search_filter: String::new(),
                selected_model_path: None,
                renderer: None,
                gpu_model: None,
                render_target: None,
                pending_model: None,
                decoded_textures: HashMap::new(),
                yaw: 0.6,
                pitch: 0.35,
                distance_scale: 1.0,
                pan_offset: [0.0; 3],
                screenshot_request: false,
            };
            if let Some(model_path) = initial_model_path {
                app.load_selected_model(&model_path);
            }
            app
        }

        fn load_selected_model(&mut self, internal_model_path: &str) {
            let Some(data_directory) = self.data_directory.as_mut() else {
                return;
            };
            match modelview::app::load_model(data_directory, internal_model_path) {
                Ok((model, textures, warnings)) => {
                    self.status_message = format!(
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
                    self.decoded_textures = textures;
                    self.selected_model_path = Some(internal_model_path.to_string());
                    self.gpu_model = None;
                    self.pan_offset = [0.0; 3];
                    self.distance_scale = 1.0;
                    // Stash CPU model for upload on next frame via pending field reuse.
                    self.pending_model = Some(model);
                }
                Err(error) => {
                    self.status_message = format!("failed to load {internal_model_path}: {error:#}")
                }
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
                    ui.text_edit_singleline(&mut self.search_filter);
                    ui.label(&self.status_message);
                    if ui.button("Screenshot PNG").clicked() {
                        self.screenshot_request = true;
                    }
                    ui.separator();
                    let search_filter = self.search_filter.to_ascii_lowercase();
                    let mut clicked_model_path: Option<String> = None;
                    eframe::egui::ScrollArea::vertical().show(ui, |ui| {
                        for model_path in &self.model_paths {
                            if !search_filter.is_empty()
                                && !model_path.to_ascii_lowercase().contains(&search_filter)
                            {
                                continue;
                            }
                            let is_selected =
                                self.selected_model_path.as_deref() == Some(model_path.as_str());
                            if ui.selectable_label(is_selected, model_path).clicked() {
                                clicked_model_path = Some(model_path.clone());
                            }
                        }
                    });
                    if let Some(model_path) = clicked_model_path {
                        self.load_selected_model(&model_path);
                    }
                });

            eframe::egui::CentralPanel::default().show(ui, |ui| {
                let (rect, response) = ui.allocate_exact_size(ui.available_size(), eframe::egui::Sense::drag());
                if response.dragged_by(eframe::egui::PointerButton::Primary) {
                    let drag_delta = response.drag_delta();
                    self.yaw -= drag_delta.x * 0.008;
                    self.pitch = (self.pitch + drag_delta.y * 0.008).clamp(-1.4, 1.4);
                }
                if response.dragged_by(eframe::egui::PointerButton::Secondary) || response.dragged_by(eframe::egui::PointerButton::Middle) {
                    let drag_delta = response.drag_delta();
                    self.pan_offset[0] -= drag_delta.x * 0.01;
                    self.pan_offset[1] += drag_delta.y * 0.01;
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
                    self.pan_offset = [0.0; 3];
                }

                if let Some(state) = frame.wgpu_render_state() {
                    if self.renderer.is_none() {
                        self.renderer = Some(Renderer::new(&state.device));
                    }
                    if let Some(model) = self.pending_model.take() {
                        let mut texture_views = HashMap::new();
                        if let Some(renderer) = self.renderer.as_ref() {
                            for (texture_path, (rgba_pixels, texture_width, texture_height)) in &self.decoded_textures {
                                texture_views.insert(texture_path.clone(), renderer.upload_texture(&state.device, &state.queue, rgba_pixels, *texture_width, *texture_height));
                            }
                        }
                        if let Some(renderer) = self.renderer.as_mut() {
                            self.gpu_model = Some(renderer.upload_model(&state.device, &state.queue, &model, &texture_views));
                        }
                    }
                    if let (Some(renderer), Some(gpu_model)) = (self.renderer.as_mut(), self.gpu_model.as_ref()) {
                        let viewport_width = (rect.width() as u32).max(1);
                        let viewport_height = (rect.height() as u32).max(1);
                        let surface_format = state.target_format;
                        // Reuse the offscreen target across frames; recreating it (and
                        // registering a new egui texture) every frame leaks GPU memory.
                        let render_target_is_stale = self.render_target.as_ref().map(|(target_width, target_height, ..)| (*target_width, *target_height)) != Some((viewport_width, viewport_height));
                        if render_target_is_stale {
                            if let Some((_, _, _, _, old_id)) = self.render_target.take() {
                                state.renderer.write().free_texture(&old_id);
                            }
                            let color_texture = state.device.create_texture(&wgpu::TextureDescriptor {
                                label: Some("gui-color"), size: wgpu::Extent3d { width: viewport_width, height: viewport_height, depth_or_array_layers: 1 },
                                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: surface_format,
                                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
                            });
                            let depth_texture = state.device.create_texture(&wgpu::TextureDescriptor {
                                label: Some("gui-depth"), size: wgpu::Extent3d { width: viewport_width, height: viewport_height, depth_or_array_layers: 1 },
                                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                                format: wgpu::TextureFormat::Depth32Float, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
                            });
                            let color_view = color_texture.create_view(&Default::default());
                            let egui_texture_id = state.renderer.write().register_native_texture(&state.device, &color_view, wgpu::FilterMode::Linear);
                            self.render_target = Some((viewport_width, viewport_height, color_texture, depth_texture, egui_texture_id));
                        }
                        let (_, _, color_texture, depth_texture, egui_texture_id) = self.render_target.as_ref().unwrap();
                        let mut camera = Camera::new(gpu_model.bounding_center, gpu_model.bounding_radius, self.yaw, self.pitch);
                        camera.distance *= self.distance_scale;
                        camera.target = [gpu_model.bounding_center[0] + self.pan_offset[0], gpu_model.bounding_center[1] + self.pan_offset[1], gpu_model.bounding_center[2]];
                        let mut encoder = state.device.create_command_encoder(&Default::default());
                        renderer.draw_model(&state.device, &state.queue, &mut encoder, &color_texture.create_view(&Default::default()), &depth_texture.create_view(&Default::default()), surface_format, gpu_model, &camera, viewport_width, viewport_height);
                        state.queue.submit([encoder.finish()]);
                        ui.painter().image(*egui_texture_id, rect, eframe::egui::Rect::from_min_max(eframe::egui::pos2(0.0, 0.0), eframe::egui::pos2(1.0, 1.0)), eframe::egui::Color32::WHITE);
                        if self.screenshot_request {
                            self.screenshot_request = false;
                            self.status_message = "Use CLI --screenshot for PNG export (GUI capture not wired)".into();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parse_cli_defaults_to_gui_at_1024x768() {
        let cli = parse_cli_from(&[]).expect("empty args parse");
        assert!(cli.data_dir_path.is_none());
        assert!(cli.screenshot_output_path.is_none());
        assert_eq!((cli.screenshot_width, cli.screenshot_height), (1024, 768));
        assert!((cli.camera_yaw - 0.6).abs() < 1e-6);
    }

    #[test]
    fn parse_cli_reads_full_screenshot_invocation() {
        let cli = parse_cli_from(&args(&[
            "--data",
            "/wow/Data",
            "--model",
            "Creature\\Dragon\\DragonOnyxia.m2",
            "--screenshot",
            "out.png",
            "--width",
            "800",
            "--height",
            "600",
            "--angle",
            "1.25, -0.5",
        ]))
        .expect("full invocation parses");
        assert_eq!(cli.data_dir_path.unwrap().to_str().unwrap(), "/wow/Data");
        assert_eq!(
            cli.initial_model_path.as_deref(),
            Some("Creature\\Dragon\\DragonOnyxia.m2")
        );
        assert_eq!(
            cli.screenshot_output_path.unwrap().to_str().unwrap(),
            "out.png"
        );
        assert_eq!((cli.screenshot_width, cli.screenshot_height), (800, 600));
        assert!((cli.camera_yaw - 1.25).abs() < 1e-6);
        assert!((cli.camera_pitch + 0.5).abs() < 1e-6);
    }

    #[test]
    fn parse_cli_rejects_unknown_flag() {
        assert!(parse_cli_from(&args(&["--bogus"])).is_err());
    }

    #[test]
    fn parse_cli_rejects_missing_flag_value() {
        assert!(parse_cli_from(&args(&["--data"])).is_err());
        assert!(parse_cli_from(&args(&["--width"])).is_err());
    }

    #[test]
    fn parse_cli_rejects_malformed_numbers_and_angles() {
        assert!(parse_cli_from(&args(&["--width", "wide"])).is_err());
        assert!(parse_cli_from(&args(&["--angle", "1.0"])).is_err());
        assert!(parse_cli_from(&args(&["--angle", "left,right"])).is_err());
    }
}
