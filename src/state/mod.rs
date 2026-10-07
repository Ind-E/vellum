//! Overlay lifecycle and application state.

mod freeze;
mod input;
mod output;
mod render;
mod wayland;

use color::DynamicColor;
use wayland_client::QueueHandle;

use crate::OutputId;
use crate::config::DrawOn;
use crate::draw::{self, Action};
use crate::render::GpuContext;
use input::PendingPenMotion;
use wayland::WaylandState;

pub(crate) struct State {
    pub fatal_error: Option<String>,
    active: bool,
    draw_on: DrawOn,
    selected_output: Option<OutputId>,
    input_output: Option<OutputId>,
    keyboard_output: Option<OutputId>,
    clear_on_escape: bool,
    freeze: freeze::Freeze,
    pending_pen_motion: PendingPenMotion,
    render_requested: bool,

    wayland: WaylandState,
    draw: draw::DrawState,
    keyboard: input::KeyboardState,
    text_input: input::TextInputState,
    pointer: input::PointerState,
    tablet: input::TabletState,

    gpu: Option<GpuContext>,
    qhandle: QueueHandle<State>,
}

impl State {
    pub(crate) fn toggle_input(&mut self) {
        self.set_input_active(!self.active);
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn is_text_editing(&self) -> bool {
        self.draw.is_editing_text()
    }

    pub(crate) fn set_input_active(&mut self, active: bool) {
        if active == self.active {
            return;
        }
        log::debug!("setting drawing mode active={active}");
        if active {
            self.activate();
        } else {
            self.deactivate();
        }
        log::debug!("drawing mode active={}", self.active);
    }

    fn activate(&mut self) {
        self.active = true;
        self.selected_output = None;
        if self
            .keyboard_output
            .is_none_or(|output| !self.wayland.outputs.contains_key(&output))
        {
            self.keyboard_output = self.wayland.outputs.keys().next().copied();
        }
        self.update_output_input();
        if self.draw.activate() {
            self.request_render();
        }
        self.refresh_cursor();
        if self.freeze.on_activate {
            self.start_freeze();
        }
    }

    fn deactivate(&mut self) {
        self.stop_freeze(true);
        self.flush_pen_motion();
        self.keyboard.cancel_repeat();
        self.pointer.cancel_gesture();
        self.tablet.cancel_gesture();
        self.pending_pen_motion.reset(None);
        self.draw.deactivate();
        for output in self.wayland.outputs.values_mut() {
            if let Some(wgpu) = &mut output.wgpu {
                wgpu.release_picker_target();
            }
        }
        self.pointer.restore_cursor();
        self.tablet.restore_cursors();
        self.active = false;
        self.selected_output = None;
        // Commit input release and replacement of frozen content together.
        self.update_output_input();
    }

    pub(crate) fn clear(&mut self) {
        self.apply_action(Action::Clear);
    }

    pub(crate) fn set_current_color(&mut self, color: DynamicColor) {
        let rgba = crate::color_to_srgb(color);
        let render = self.draw.set_current_color(rgba);
        if render {
            self.request_render();
        }
        self.refresh_cursor();
    }

    pub(crate) fn next_wakeup(&self) -> Option<std::time::Instant> {
        [
            self.draw.next_wakeup(),
            self.keyboard.next_wakeup(),
            self.freeze.next_wakeup(),
            self.wayland
                .outputs
                .iter()
                .filter(|(id, _)| !self.freeze.hides(**id))
                .filter_map(|(_, output)| output.render_retry)
                .min(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    pub(crate) fn handle_timeouts(&mut self, now: std::time::Instant) {
        self.handle_freeze(now);
        if let Some(action) = self.keyboard.repeat_action(
            now,
            self.draw
                .text_input_snapshot()
                .map(|snapshot| snapshot.session),
        ) {
            self.apply_action(action);
        }
        if self.draw.handle_timeouts(now) {
            self.request_render();
        }
        if self.wayland.outputs.iter().any(|(id, output)| {
            !self.freeze.hides(*id) && output.render_retry.is_some_and(|deadline| now >= deadline)
        }) {
            self.request_render();
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        for output in self.wayland.outputs.values_mut() {
            output.wgpu.take();
        }
        self.gpu.take();
    }
}
