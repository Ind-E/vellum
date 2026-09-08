//! Shared GPU device and the Wayland raw-handle boundary.

use wayland_client::Proxy;
use wayland_client::protocol::wl_display::WlDisplay;
use wayland_client::protocol::wl_surface::WlSurface;

use super::WgpuState;

pub(crate) struct GpuContext {
    instance: wgpu::Instance,
    pub(super) adapter: wgpu::Adapter,
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
}

impl GpuContext {
    pub(crate) fn new(
        display: &WlDisplay,
        surface: &WlSurface,
        width: u32,
        height: u32,
    ) -> Result<(Self, WgpuState), String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = create_surface(&instance, display, surface)?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .map_err(|error| format!("could not select a Vulkan adapter: {error}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: None,
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|error| format!("could not create the GPU device: {error}"))?;
        let gpu = Self {
            instance,
            adapter,
            device,
            queue,
        };
        let wgpu = WgpuState::new(&gpu, surface, width, height)?;
        Ok((gpu, wgpu))
    }

    pub(crate) fn create_surface(
        &self,
        display: &WlDisplay,
        surface: &WlSurface,
    ) -> Result<wgpu::Surface<'static>, String> {
        create_surface(&self.instance, display, surface)
    }
}

fn create_surface(
    instance: &wgpu::Instance,
    display: &WlDisplay,
    surface: &WlSurface,
) -> Result<wgpu::Surface<'static>, String> {
    let raw_display_handle =
        wgpu::rwh::RawDisplayHandle::Wayland(wgpu::rwh::WaylandDisplayHandle::new(
            std::ptr::NonNull::new(display.id().as_ptr() as *mut _).unwrap(),
        ));
    let raw_window_handle =
        wgpu::rwh::RawWindowHandle::Wayland(wgpu::rwh::WaylandWindowHandle::new(
            std::ptr::NonNull::new(surface.id().as_ptr() as *mut _).unwrap(),
        ));
    // SAFETY: These proxies belong to the live connection retained by State.
    // State drops each output's renderer before destroying its wl_surface, and
    // drops all renderers and the GPU before the connection (see State::drop).
    unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(raw_display_handle),
            raw_window_handle,
        })
    }
    .map_err(|error| format!("could not create the Wayland GPU surface: {error}"))
}
