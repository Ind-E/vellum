use super::{LocalGeometry, Viewport, checked_target_size, replay_geometry};
use kurbo::Affine;
use wgpu::util::DeviceExt;

const PICKER_RENDER_SCALE: u32 = 2;

struct PickerTarget {
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    composite_buffer: wgpu::Buffer,
    size: [u16; 2],
    origin: [f32; 2],
}

fn composite_bytes(origin: [f32; 2]) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..4].copy_from_slice(&origin[0].to_ne_bytes());
    bytes[4..8].copy_from_slice(&origin[1].to_ne_bytes());
    bytes
}

impl PickerTarget {
    fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        size: [u16; 2],
        origin: [f32; 2],
        layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("picker target"),
            size: wgpu::Extent3d {
                width: u32::from(size[0]),
                height: u32::from(size[1]),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let composite_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("picker composite origin"),
            contents: &composite_bytes(origin),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("picker composite"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: composite_buffer.as_entire_binding(),
                },
            ],
        });
        Self {
            view,
            bind_group,
            composite_buffer,
            size,
            origin,
        }
    }
}

pub(super) struct PickerState {
    renderer: vello_hybrid::Renderer,
    resources: vello_hybrid::Resources,
    scene: vello_hybrid::Scene,
    composite_pipeline: wgpu::RenderPipeline,
    composite_layout: wgpu::BindGroupLayout,
    target: Option<PickerTarget>,
}

impl PickerState {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("picker composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let picker_composite_shader =
            device.create_shader_module(wgpu::include_wgsl!("picker_composite.wgsl"));
        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("picker composite pipeline"),
                bind_group_layouts: &[Some(&composite_layout)],
                immediate_size: 0,
            });
        let composite_pipeline = create_composite_pipeline(
            device,
            &composite_pipeline_layout,
            &picker_composite_shader,
            format,
        );
        let target_config = vello_hybrid::RenderTargetConfig {
            format,
            width: 1,
            height: 1,
        };
        let (renderer, resources) = vello_hybrid::Renderer::new(device, &target_config);
        Self {
            renderer,
            resources,
            scene: vello_hybrid::Scene::new(1, 1),
            composite_pipeline,
            composite_layout,
            target: None,
        }
    }

    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: &wgpu::SurfaceConfiguration,
        picker: &LocalGeometry,
        viewport: &Viewport,
    ) -> Result<[f32; 4], String> {
        let Viewport {
            origin: viewport_origin,
            scale,
        } = *viewport;
        let picker_origin = [
            (f64::from(picker.origin[0] - viewport_origin[0]) * scale[0]) as f32,
            (f64::from(picker.origin[1] - viewport_origin[1]) * scale[1]) as f32,
        ];
        let size = [
            (f64::from(picker.size[0]) * scale[0]).ceil() as u32,
            (f64::from(picker.size[1]) * scale[1]).ceil() as u32,
        ];
        let scene_size = checked_scene_size(device, size)?;
        if self
            .target
            .as_ref()
            .is_none_or(|target| target.size != scene_size)
        {
            self.target = Some(PickerTarget::new(
                device,
                config.format,
                scene_size,
                picker_origin,
                &self.composite_layout,
            ));
        }
        let target = self.target.as_mut().unwrap();
        if target.origin != picker_origin {
            queue.write_buffer(&target.composite_buffer, 0, &composite_bytes(picker_origin));
            target.origin = picker_origin;
        }
        self.scene.reset_and_resize(scene_size[0], scene_size[1]);
        self.scene.set_transform(
            Affine::scale(f64::from(PICKER_RENDER_SCALE))
                * Affine::scale_non_uniform(scale[0], scale[1]),
        );
        replay_geometry(&mut self.scene, &picker.geometry, config.format.is_srgb());
        let left = picker_origin[0].max(0.0);
        let top = picker_origin[1].max(0.0);
        let right =
            (picker_origin[0] + picker.size[0] as f32 * scale[0] as f32).min(config.width as f32);
        let bottom =
            (picker_origin[1] + picker.size[1] as f32 * scale[1] as f32).min(config.height as f32);
        Ok([left, top, right - left, bottom - top])
    }

    pub(super) fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        texture_bindings: &vello_hybrid::TextureBindings,
        view: &wgpu::TextureView,
        viewport: [f32; 4],
    ) -> Result<(), String> {
        let size = self.target.as_ref().unwrap().size;
        let render_size = vello_hybrid::RenderSize {
            width: u32::from(size[0]),
            height: u32::from(size[1]),
        };
        self.renderer
            .render(
                &self.scene,
                &mut self.resources,
                device,
                queue,
                encoder,
                &render_size,
                &self.target.as_ref().unwrap().view,
                texture_bindings,
            )
            .map_err(|error| format!("Vello picker render failed: {error}"))?;
        if viewport[2] > 0.0 && viewport[3] > 0.0 {
            self.composite_picker(encoder, view, self.target.as_ref().unwrap(), viewport);
        }
        Ok(())
    }

    pub(super) fn release_target(&mut self) {
        self.target = None;
    }

    fn composite_picker(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        source: &PickerTarget,
        viewport: [f32; 4],
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("composite picker"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.composite_pipeline);
        let [x, y, width, height] = viewport;
        pass.set_viewport(x, y, width, height, 0.0, 1.0);
        pass.set_bind_group(0, &source.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn checked_scene_size(device: &wgpu::Device, size: [u32; 2]) -> Result<[u16; 2], String> {
    let scaled = size.map(|dimension| dimension.checked_mul(PICKER_RENDER_SCALE));
    let [Some(width), Some(height)] = scaled else {
        return Err(format!(
            "picker target {}x{} overflows at {PICKER_RENDER_SCALE}x scale",
            size[0], size[1]
        ));
    };
    checked_target_size(device, [width, height], "picker")
}

fn create_composite_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("picker composite pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &[("render_scale", PICKER_RENDER_SCALE as f64)],
                ..Default::default()
            },
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}
