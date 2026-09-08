//! Overlay lifecycle and application state.

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
    pending_pen_motion: PendingPenMotion,

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
        if active {
            self.activate();
        } else {
            self.deactivate();
        }
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
    }

    fn deactivate(&mut self) {
        self.flush_pen_motion();
        self.keyboard.cancel_repeat();
        self.pointer.cancel_gesture();
        self.tablet.cancel_gesture();
        self.pending_pen_motion.reset(None);
        let preview_changed = self.draw.deactivate();
        self.pointer.restore_cursor();
        self.tablet.restore_cursors();
        self.active = false;
        self.selected_output = None;
        self.update_output_input();
        if preview_changed {
            self.request_render();
        } else {
            for output in self.wayland.outputs.values_mut() {
                if let Some(wgpu) = &mut output.wgpu {
                    wgpu.release_picker_target();
                }
            }
        }
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
        [self.draw.next_wakeup(), self.keyboard.next_wakeup()]
            .into_iter()
            .flatten()
            .min()
    }

    pub(crate) fn handle_timeouts(&mut self, now: std::time::Instant) {
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
