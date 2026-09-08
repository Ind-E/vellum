//! Frame callbacks, damage scheduling and presentation.

use wayland_client::protocol::wl_callback::WlCallback;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use super::State;
use crate::OutputId;

impl State {
    pub(super) fn render(&mut self, output: OutputId) {
        if self.fatal_error.is_some() {
            return;
        }
        if let Some(output_state) = self.wayland.outputs.get_mut(&output) {
            let scale = output_state.render_scale();
            let Some(wgpu) = output_state.wgpu.as_mut() else {
                return;
            };
            let text_input = &mut self.text_input;
            let proxy = self.wayland.text_input.as_ref();
            if let Err(error) =
                self.draw
                    .render(output, output_state.origin, scale, wgpu, |snapshot| {
                        text_input.sync_render(proxy, output, snapshot);
                    })
            {
                self.fatal_error = Some(error);
                return;
            }
            if !self.active {
                wgpu.release_picker_target();
            }
        }
    }

    pub(super) fn request_render(&mut self) {
        // Only the input output advances a stroke. An idle secondary output
        // must not defeat batching while the input output awaits its frame.
        if self
            .input_output
            .and_then(|id| self.wayland.outputs.get(&id))
            .is_some_and(|output| !output.frame_pending && output.wgpu.is_some())
        {
            self.flush_pen_motion();
        }
        let outputs: Vec<_> = self.draw.damaged_outputs().collect();
        for output in outputs {
            self.request_output_render(output);
        }
    }

    fn request_output_render(&mut self, id: OutputId) {
        let Some(output) = self.wayland.outputs.get_mut(&id) else {
            return;
        };
        if output.frame_pending || output.wgpu.is_none() || !self.draw.needs_render(id) {
            return;
        }
        output.surface.frame(&self.qhandle, id);
        output.frame_pending = true;
        self.render(id);
        // A successful presentation commits the frame request with its buffer.
        // If acquisition failed, commit the callback alone so it can retry.
        if self.fatal_error.is_none()
            && self.draw.needs_render(id)
            && let Some(output) = self.wayland.outputs.get(&id)
        {
            output.surface.commit();
        }
    }
}

impl Dispatch<WlCallback, OutputId> for State {
    fn event(
        state: &mut Self,
        _callback: &WlCallback,
        event: <WlCallback as Proxy>::Event,
        output: &OutputId,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_callback::Event;
        if let Event::Done { callback_data: _ } = event {
            if let Some(output_state) = state.wayland.outputs.get_mut(output) {
                output_state.frame_pending = false;
            }
            state.request_render();
        }
    }
}
