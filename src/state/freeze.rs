use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use wayland_client::protocol::wl_callback::WlCallback;
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_subsurface::WlSubsurface, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::{
    self, Flags, ZwpLinuxBufferParamsV1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wlr_capture::wl::{Client, Frame as CapturedFrame, Protocol};

use super::{DrawOn, OutputId, State};

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(2);

enum Capture {
    Prepared,
    Image(OutputId, CapturedFrame),
}

pub(super) struct Freeze {
    pub on_activate: bool,
    pub defer_input: bool,
    generation: u64,
    worker: Option<std::thread::JoinHandle<()>>,
    phase: Phase,
}

enum Phase {
    Live,
    WaitingOutput,
    Capturing {
        deadline: Instant,
        frames: BTreeMap<OutputId, Frame>,
        events: Receiver<Result<Capture, String>>,
        start: Sender<()>,
    },
    Frozen,
}

#[derive(Default)]
struct Frame {
    hidden: bool,
    presented: bool,
    ready: bool,
    received: bool,
}

// The library owns its Wayland queue and blocks while negotiating constraints.
// Keep that work off Vellum's event loop, and wait for its transparent commits
// before submitting captures on the separate connection.
fn capture(
    outputs: Vec<(OutputId, String)>,
    events: &Sender<Result<Capture, String>>,
    start: Receiver<()>,
    deadline: Instant,
    use_dmabuf: bool,
) -> Result<(), String> {
    let mut client = Client::connect().map_err(|error| error.to_string())?;
    if client.protocol() != Protocol::ImageCopyCapture {
        return Err(
            "compositor does not support ext-image-copy-capture with output sources".into(),
        );
    }
    if !use_dmabuf {
        client.disable_gpu();
    }
    let mut sessions = Vec::with_capacity(outputs.len());
    for (id, name) in outputs {
        let output = client
            .outputs()
            .iter()
            .find(|output| output.name == name)
            .cloned()
            .ok_or_else(|| format!("capture output {name:?} is unavailable"))?;
        let session = client
            .open_output_session(&output)
            .map_err(|error| error.to_string())?;
        sessions.push((session, id));
    }
    if events.send(Ok(Capture::Prepared)).is_err()
        || start
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_err()
    {
        return Ok(());
    }
    while !sessions.is_empty() {
        if start.try_recv() == Err(TryRecvError::Disconnected) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("capture timed out".into());
        }
        let (frames, stopped) = client.poll(POLL_INTERVAL);
        if !stopped.is_empty() {
            return Err("capture stopped by compositor".into());
        }
        for (session, frame) in frames {
            let Some(index) = sessions.iter().position(|(pending, _)| *pending == session) else {
                continue;
            };
            let (_, output) = sessions.swap_remove(index);
            client.close_session(&session);
            if events.send(Ok(Capture::Image(output, frame))).is_err() {
                return Ok(());
            }
        }
    }
    Ok(())
}

impl Freeze {
    pub fn new(on_activate: bool) -> Self {
        Self {
            on_activate,
            defer_input: false,
            generation: 0,
            worker: None,
            phase: Phase::Live,
        }
    }

    pub fn capturing(&self) -> bool {
        matches!(self.phase, Phase::Capturing { .. })
    }

    pub fn hides(&self, output: OutputId) -> bool {
        matches!(&self.phase, Phase::Capturing { frames, .. } if frames.get(&output).is_some_and(|frame| frame.hidden && !frame.ready))
    }

    pub fn next_wakeup(&self) -> Option<Instant> {
        match &self.phase {
            Phase::Capturing { deadline, .. } => {
                Some((*deadline).min(Instant::now() + POLL_INTERVAL))
            }
            _ => None,
        }
    }
}

impl State {
    pub(super) fn toggle_freeze(&mut self) {
        if self.mode == super::Mode::Whiteboard {
            return;
        }
        if !self.freeze.capturing()
            && (self.draw.is_editing_text()
                || self.draw.picker_active()
                || self.pointer.input_grab_active()
                || self.tablet.input_grab_active())
        {
            return;
        }
        if !matches!(self.freeze.phase, Phase::Live) {
            self.stop_freeze(false);
        } else {
            self.start_freeze();
        }
    }

    pub(super) fn start_freeze(&mut self) {
        if self.draw_on == DrawOn::Current
            && self.selected_output.is_none()
            && !self.freeze.defer_input
        {
            self.freeze.phase = Phase::WaitingOutput;
            return;
        }
        // Negotiation in the library has no timeout. Retain its handle so a
        // stalled compositor cannot accumulate abandoned capture threads.
        if self
            .freeze
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            self.fail_freeze("Screen freeze unavailable: previous capture is still finishing");
            return;
        }
        let mut outputs = Vec::new();
        for (&id, output) in &self.wayland.outputs {
            // Before taking focus we cannot know which output the pointer is on.
            // Capture all outputs, then keep only the one selected on entry.
            if self.draw_on == DrawOn::Current
                && self.selected_output.is_some_and(|selected| selected != id)
            {
                continue;
            }
            if output.wgpu.is_none() || output.name.is_empty() {
                self.fail_freeze("Screen freeze unavailable: output is not ready");
                return;
            }
            outputs.push((id, output.name.clone()));
        }
        if outputs.is_empty() {
            self.fail_freeze("Screen freeze unavailable: no output");
            return;
        }
        let frames = outputs
            .iter()
            .map(|(id, _)| (*id, Frame::default()))
            .collect();
        let deadline = Instant::now() + CAPTURE_TIMEOUT;
        let (events_tx, events) = mpsc::channel();
        let (start, start_rx) = mpsc::channel();
        let use_dmabuf = self.wayland.dmabuf.is_some()
            && self.wayland.subcompositor.is_some()
            && self.wayland.viewporter.is_some();
        match std::thread::Builder::new()
            .name("screen capture".into())
            .spawn(move || {
                if let Err(error) = capture(outputs, &events_tx, start_rx, deadline, use_dmabuf) {
                    let _ = events_tx.send(Err(error));
                }
            }) {
            Ok(worker) => self.freeze.worker = Some(worker),
            Err(error) => {
                self.fail_freeze(&format!("Screen freeze unavailable: {error}"));
                return;
            }
        }
        self.freeze.generation = self.freeze.generation.wrapping_add(1);
        self.freeze.phase = Phase::Capturing {
            deadline,
            frames,
            events,
            start,
        };
        self.keyboard.cancel_repeat();
    }

    pub(super) fn freeze_output_selected(&mut self) {
        if matches!(self.freeze.phase, Phase::WaitingOutput) && self.selected_output.is_some() {
            self.start_freeze();
        } else if matches!(self.freeze.phase, Phase::Frozen) && self.draw_on == DrawOn::Current {
            for (&id, output) in &mut self.wayland.outputs {
                if self.selected_output != Some(id) && output.is_frozen() {
                    output.clear_frozen_background();
                    self.draw.damage(id);
                }
            }
            self.request_render();
        }
    }

    pub(super) fn stop_freeze(&mut self, deactivating: bool) {
        let deferred_input = std::mem::take(&mut self.freeze.defer_input);
        let frames = match std::mem::replace(&mut self.freeze.phase, Phase::Live) {
            Phase::Capturing { frames, .. } => frames,
            _ => BTreeMap::new(),
        };
        let mut damaged = Vec::new();
        for (&id, output) in &mut self.wayland.outputs {
            if output.is_frozen() || frames.contains_key(&id) {
                output.clear_frozen_background();
                self.draw.damage(id);
                damaged.push(id);
            }
        }
        if !deactivating {
            if deferred_input {
                self.update_output_input();
            }
            for id in damaged {
                self.render(id);
            }
            self.request_render();
        }
    }

    pub(super) fn invalidate_freeze(&mut self) {
        if matches!(self.freeze.phase, Phase::Capturing { .. } | Phase::Frozen) {
            self.stop_freeze(false);
        }
    }

    pub(super) fn fail_freeze(&mut self, message: &str) {
        self.stop_freeze(false);
        eprintln!("vellum: {message}");
    }

    fn copy_screen(&mut self) -> Result<(), String> {
        let Phase::Capturing { frames, .. } = &self.freeze.phase else {
            unreachable!()
        };
        let mut blanks = Vec::with_capacity(frames.len());
        for &output in frames.keys() {
            let blank = self.wayland.outputs[&output]
                .wgpu
                .as_ref()
                .unwrap()
                .hide_annotations()?
                .ok_or("could not hide annotations for capture")?;
            blanks.push((output, blank));
        }
        let Phase::Capturing { frames, .. } = &mut self.freeze.phase else {
            unreachable!()
        };
        for (output, blank) in blanks {
            frames.get_mut(&output).unwrap().hidden = true;
            self.wayland.outputs[&output]
                .surface
                .frame(&self.qhandle, (self.freeze.generation, output));
            blank.present();
            self.draw.damage(output);
        }
        Ok(())
    }

    fn capture_event(&mut self, event: Capture) -> Result<(), String> {
        match event {
            Capture::Prepared => self.copy_screen(),
            Capture::Image(output, image) => {
                let Phase::Capturing { frames, .. } = &mut self.freeze.phase else {
                    unreachable!()
                };
                let frame = frames
                    .get_mut(&output)
                    .ok_or("capture output disappeared")?;
                frame.received = true;
                match image {
                    CapturedFrame::Shm(image) => {
                        let wgpu = self
                            .wayland
                            .outputs
                            .get_mut(&output)
                            .unwrap()
                            .wgpu
                            .as_mut()
                            .unwrap();
                        wgpu.set_frozen_background([image.width, image.height], &image.rgba)?;
                        frame.ready = true;
                        log::debug!(
                            "frozen output {output}: shared memory {}x{}",
                            image.width,
                            image.height
                        );
                    }
                    CapturedFrame::Dmabuf(image) => {
                        let params = self
                            .wayland
                            .dmabuf
                            .as_ref()
                            .unwrap()
                            .create_params(&self.qhandle, (self.freeze.generation, output));
                        for (index, plane) in image.planes.iter().enumerate() {
                            use std::os::fd::AsFd;
                            params.add(
                                plane.fd.as_fd(),
                                index as u32,
                                plane.offset,
                                plane.stride,
                                (image.modifier >> 32) as u32,
                                image.modifier as u32,
                            );
                        }
                        params.create(
                            image.width as i32,
                            image.height as i32,
                            image.fourcc,
                            Flags::empty(),
                        );
                        log::debug!(
                            "frozen output {output}: DMA-BUF {}x{}, {} planes, modifier {:#x}",
                            image.width,
                            image.height,
                            image.planes.len(),
                            image.modifier
                        );
                        return Ok(());
                    }
                }
                self.draw.damage(output);
                self.render(output);
                Ok(())
            }
        }
    }

    pub(super) fn handle_freeze(&mut self, now: Instant) {
        let disconnected = loop {
            let Phase::Capturing { events, .. } = &self.freeze.phase else {
                return;
            };
            match events.try_recv() {
                Ok(event) => {
                    if let Err(error) = event.and_then(|event| self.capture_event(event)) {
                        self.fail_freeze(&format!("Screen freeze failed: {error}"));
                        return;
                    }
                }
                Err(error) => break error == TryRecvError::Disconnected,
            }
        };
        let Phase::Capturing {
            deadline, frames, ..
        } = &self.freeze.phase
        else {
            return;
        };
        if frames.values().all(|frame| frame.ready) {
            log::debug!(
                "screen frozen in {:?}",
                Instant::now().duration_since(*deadline - CAPTURE_TIMEOUT)
            );
            self.freeze.phase = Phase::Frozen;
            if self.selected_output.is_some() {
                self.freeze_output_selected();
            }
            if std::mem::take(&mut self.freeze.defer_input) {
                self.update_output_input();
            }
            self.request_render();
        } else if disconnected && frames.values().any(|frame| !frame.received) {
            self.fail_freeze("Screen freeze failed: capture worker stopped");
        } else if now >= *deadline {
            self.fail_freeze("Screen freeze timed out; returned to live drawing");
        }
    }
}

impl Dispatch<WlCallback, (u64, OutputId)> for State {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        _: <WlCallback as Proxy>::Event,
        &(generation, output): &(u64, OutputId),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if generation != state.freeze.generation {
            return;
        }
        let Phase::Capturing { frames, start, .. } = &mut state.freeze.phase else {
            return;
        };
        let Some(frame) = frames.get_mut(&output) else {
            return;
        };
        frame.presented = true;
        if frames.values().all(|frame| frame.presented) && start.send(()).is_err() {
            state.fail_freeze("Screen freeze failed: capture worker stopped");
        }
    }
}

// The compositor samples the captured buffer beneath the transparent annotation
// surface. Vellum never maps it or imports it into its Vulkan device.
pub(super) struct FrozenSurface {
    surface: WlSurface,
    subsurface: WlSubsurface,
    viewport: WpViewport,
    buffer: Option<WlBuffer>,
}

impl FrozenSurface {
    pub(super) fn is_frozen(&self) -> bool {
        self.buffer.is_some()
    }

    pub(super) fn clear(&mut self) {
        // Apply removal with the replacement parent frame, including when its
        // presentation is delayed. Reuse the empty subsurface on the next freeze.
        self.surface.attach(None, 0, 0);
        self.surface.commit();
        if let Some(buffer) = self.buffer.take() {
            buffer.destroy();
        }
    }
}

impl Drop for FrozenSurface {
    fn drop(&mut self) {
        self.subsurface.destroy();
        self.viewport.destroy();
        self.surface.destroy();
        if let Some(buffer) = self.buffer.take() {
            buffer.destroy();
        }
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, (u64, OutputId)> for State {
    fn event(
        state: &mut Self,
        params: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        &(generation, output): &(u64, OutputId),
        _: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        params.destroy();
        let current = generation == state.freeze.generation
            && matches!(&state.freeze.phase, Phase::Capturing { frames, .. }
                if frames.get(&output).is_some_and(|frame| !frame.ready));
        match event {
            zwp_linux_buffer_params_v1::Event::Created { buffer } => {
                if !current {
                    buffer.destroy();
                    return;
                }
                let parent = &mut state.wayland.outputs.get_mut(&output).unwrap();
                if parent.frozen_surface.is_none() {
                    let surface = state.wayland.compositor.create_surface(qhandle, output);
                    let subsurface = state
                        .wayland
                        .subcompositor
                        .as_ref()
                        .unwrap()
                        .get_subsurface(&surface, &parent.surface, qhandle, ());
                    subsurface.place_below(&parent.surface);
                    let viewport = state.wayland.viewporter.as_ref().unwrap().get_viewport(
                        &surface,
                        qhandle,
                        (),
                    );
                    let empty = state.wayland.compositor.create_region(qhandle, ());
                    surface.set_input_region(Some(&empty));
                    empty.destroy();
                    parent.frozen_surface = Some(FrozenSurface {
                        surface,
                        subsurface,
                        viewport,
                        buffer: None,
                    });
                }
                let frozen = parent.frozen_surface.as_mut().unwrap();
                frozen
                    .viewport
                    .set_destination(parent.logical_size[0] as i32, parent.logical_size[1] as i32);
                frozen.surface.attach(Some(&buffer), 0, 0);
                frozen.surface.damage(0, 0, i32::MAX, i32::MAX);
                // Synchronized subsurfaces become visible with the next parent
                // render, so the frozen desktop and annotations appear together.
                frozen.surface.commit();
                frozen.buffer = Some(buffer);
                if let Phase::Capturing { frames, .. } = &mut state.freeze.phase {
                    frames.get_mut(&output).unwrap().ready = true;
                }
                state.draw.damage(output);
                state.render(output);
                state.handle_freeze(Instant::now());
            }
            zwp_linux_buffer_params_v1::Event::Failed if current => {
                state.fail_freeze("Screen freeze failed: compositor could not import DMA-BUF");
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(State, ZwpLinuxBufferParamsV1, [0 => (WlBuffer, ())]);
}

wayland_client::delegate_noop!(State: ignore WlBuffer);
wayland_client::delegate_noop!(State: ignore WlSubsurface);
