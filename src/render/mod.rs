mod geometry;
mod gpu;
pub(crate) use gpu::GpuContext;
mod picker;
mod text;

use geometry::DrawCommand;
pub(crate) use geometry::{Geometry, LocalGeometry};
pub(crate) use text::TextSpec;

use kurbo::Affine;
use picker::PickerState;
use std::borrow::Cow;
use text::TextState;

pub(crate) struct Viewport {
    pub origin: [f32; 2],
    pub scale: [f64; 2],
}

pub(crate) enum SceneItem<'a> {
    Geometry(Cow<'a, Geometry>, [f32; 2]),
    Text(TextSpec<'a>),
}

pub(crate) struct WgpuState {
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    main_renderer: vello_hybrid::Renderer,
    main_resources: vello_hybrid::Resources,
    main_scene: vello_hybrid::Scene,
    texture_bindings: vello_hybrid::TextureBindings,
    picker: PickerState,
    text: TextState,
}

impl WgpuState {
    pub(crate) fn size(&self) -> [u32; 2] {
        [self.surface_config.width, self.surface_config.height]
    }

    pub(crate) fn new(
        gpu: &GpuContext,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        checked_target_size(&gpu.device, [width, height], "annotation")?;
        let capabilities = surface.get_capabilities(&gpu.adapter);
        let format = capabilities
            .formats
            .iter()
            .find(|format| format.is_srgb())
            .copied()
            .or_else(|| capabilities.formats.first().copied())
            .ok_or("GPU adapter does not support the Wayland surface")?;
        let alpha_mode = capabilities
            .alpha_modes
            .iter()
            .find(|mode| matches!(mode, wgpu::CompositeAlphaMode::PreMultiplied))
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);
        let device = gpu.device.clone();
        let queue = gpu.queue.clone();
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 1,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        let target_config = vello_hybrid::RenderTargetConfig {
            format,
            width: 1,
            height: 1,
        };
        let mut main_settings = vello_hybrid::RenderSettings::default();
        // Text is the main scene's only atlas user; 1024px avoids a 64 MiB first-use allocation.
        main_settings.memory_settings.image_atlas_config.atlas_size = (1024, 1024);
        let (main_renderer, main_resources) =
            vello_hybrid::Renderer::new_with(&device, &target_config, main_settings);
        let picker = PickerState::new(&device, format);

        Ok(Self {
            surface,
            surface_config,
            device,
            queue,
            main_renderer,
            main_resources,
            main_scene: vello_hybrid::Scene::new(1, 1),
            texture_bindings: vello_hybrid::TextureBindings::new(),
            picker,
            text: TextState::default(),
        })
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        if width == 0
            || height == 0
            || (width == self.surface_config.width && height == self.surface_config.height)
        {
            return Ok(());
        }
        checked_target_size(&self.device, [width, height], "annotation")?;
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        Ok(())
    }

    pub(crate) fn render(
        &mut self,
        items: &[SceneItem<'_>],
        previews: &[Geometry],
        picker: Option<&LocalGeometry>,
        viewport: Viewport,
        active_text: Option<(u64, &parley::Layout<()>)>,
        before_present: impl FnOnce(),
    ) -> Result<bool, String> {
        let Viewport {
            origin: viewport_origin,
            scale,
        } = viewport;
        let mut status = self.surface.get_current_texture();
        if matches!(
            status,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost
        ) {
            self.surface.configure(&self.device, &self.surface_config);
            status = self.surface.get_current_texture();
        }
        let output = match status {
            wgpu::CurrentSurfaceTexture::Success(output)
            | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Outdated
            | wgpu::CurrentSurfaceTexture::Lost => return Ok(false),
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("surface acquisition validation error".into());
            }
        };

        // Creation and resize validate these dimensions.
        let main_size = self.size().map(|dimension| dimension as u16);
        self.main_scene.reset_and_resize(main_size[0], main_size[1]);
        let scene_transform = Affine::scale_non_uniform(scale[0], scale[1])
            * Affine::translate((
                -f64::from(viewport_origin[0]),
                -f64::from(viewport_origin[1]),
            ));
        self.main_scene.set_transform(scene_transform);
        let target_is_srgb = self.surface_config.format.is_srgb();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let mut target = text::TextTarget {
            scene: &mut self.main_scene,
            resources: &mut self.main_resources,
            renderer: &mut self.main_renderer,
            device: &self.device,
            queue: &self.queue,
            encoder: &mut encoder,
            is_srgb: target_is_srgb,
        };
        for item in items {
            match item {
                SceneItem::Geometry(geometry, offset) => {
                    if *offset != [0.0; 2] {
                        target.scene.set_transform(
                            scene_transform
                                * Affine::translate((f64::from(offset[0]), f64::from(offset[1]))),
                        );
                    }
                    replay_geometry(target.scene, geometry, target_is_srgb);
                    if *offset != [0.0; 2] {
                        target.scene.set_transform(scene_transform);
                    }
                }
                SceneItem::Text(spec) => self.text.append_to_scene(&mut target, spec, active_text),
            }
        }
        self.text.finish_frame(&mut target);
        for geometry in previews {
            replay_geometry(&mut self.main_scene, geometry, target_is_srgb);
        }

        if let Some(picker) = picker {
            self.picker.prepare(
                &self.device,
                &mut self.texture_bindings,
                &mut self.main_scene,
                &self.surface_config,
                picker,
                &viewport,
            )?;
            self.picker.render(
                &self.device,
                &self.queue,
                &mut encoder,
                &self.texture_bindings,
            )?;
        }

        let swapchain_view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let render_size = vello_hybrid::RenderSize {
            width: u32::from(main_size[0]),
            height: u32::from(main_size[1]),
        };
        self.main_renderer
            .render(
                &self.main_scene,
                &mut self.main_resources,
                &self.device,
                &self.queue,
                &mut encoder,
                &render_size,
                &swapchain_view,
                &self.texture_bindings,
            )
            .map_err(|error| format!("Vello annotation render failed: {error}"))?;
        self.queue.submit(Some(encoder.finish()));
        before_present();
        output.present();
        Ok(true)
    }

    pub(crate) fn release_picker_target(&mut self) {
        self.picker.release_target(&mut self.texture_bindings);
    }
}

fn checked_target_size(
    device: &wgpu::Device,
    size: [u32; 2],
    label: &str,
) -> Result<[u16; 2], String> {
    let limit = device
        .limits()
        .max_texture_dimension_2d
        .min(u32::from(u16::MAX));
    if size
        .iter()
        .any(|dimension| *dimension == 0 || *dimension > limit)
    {
        return Err(format!(
            "{label} target {}x{} must have dimensions in 1..={limit}",
            size[0], size[1]
        ));
    }
    Ok([size[0] as u16, size[1] as u16])
}

fn replay_geometry(scene: &mut vello_hybrid::Scene, geometry: &Geometry, target_is_srgb: bool) {
    for command in &geometry.commands {
        match command {
            DrawCommand::Fill {
                path,
                fill_rule,
                color,
            } => {
                scene.set_paint(vello_color(*color, target_is_srgb));
                scene.set_fill_rule(*fill_rule);
                scene.fill_path(path);
            }
            DrawCommand::Stroke {
                path,
                stroke,
                color,
            } => {
                scene.set_paint(vello_color(*color, target_is_srgb));
                scene.set_stroke(stroke.clone());
                scene.stroke_path(path);
            }
        }
    }
}

fn srgb_to_linear(component: f32) -> f32 {
    if component <= 0.04045 {
        component / 12.92
    } else {
        ((component + 0.055) / 1.055).powf(2.4)
    }
}

fn vello_color([red, green, blue, alpha]: [f32; 4], target_is_srgb: bool) -> peniko::Color {
    let convert = |component| {
        if target_is_srgb {
            srgb_to_linear(component)
        } else {
            component
        }
    };
    peniko::Color::new([convert(red), convert(green), convert(blue), alpha])
}
