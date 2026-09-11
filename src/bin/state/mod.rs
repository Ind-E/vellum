mod draw;
mod input;

pub(crate) use draw::Tool;

use color::DynamicColor;
use std::collections::BTreeMap;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::{delegate_dispatch, delegate_noop};

use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::EventQueue;
use wayland_client::Proxy;
use wayland_client::QueueHandle;
use wayland_client::WEnum;

use wayland_client::protocol::wl_callback::WlCallback;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_display::WlDisplay;
use wayland_client::protocol::wl_keyboard::WlKeyboard;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_surface::WlSurface;

use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1;
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3;
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::ZwpTextInputV3;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_manager_v2::ZwpTabletManagerV2;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_group_v2::ZwpTabletPadGroupV2;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_pad_v2::ZwpTabletPadV2;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_seat_v2::ZwpTabletSeatV2;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_tool_v2::ZwpTabletToolV2;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_v2::ZwpTabletV2;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::ZxdgOutputV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    KeyboardInteractivity, ZwlrLayerSurfaceV1,
};

use crate::config::DrawOn;
use crate::render::{GpuContext, WgpuState};
use draw::{Action, Cursor, Modifiers, Point};

const MAX_PENDING_PEN_SAMPLES: usize = 64;
const PEN_BEND_DEVIATION_SQUARED: f32 = 0.5 * 0.5;
type OutputId = u32;

#[derive(Default)]
struct PendingPenMotion {
    anchor: Option<Point>,
    samples: Vec<Point>,
    modifiers: Modifiers,
}

impl PendingPenMotion {
    fn reset(&mut self, anchor: Option<Point>) {
        self.anchor = anchor;
        self.samples.clear();
    }

    fn push(&mut self, point: Point) {
        if self.samples.last() == Some(&point) {
            return;
        }
        if self.samples.len() == MAX_PENDING_PEN_SAMPLES {
            let mut write = 1;
            for read in (2..self.samples.len()).step_by(2) {
                self.samples[write] = self.samples[read];
                write += 1;
            }
            self.samples.truncate(write);
        }
        self.samples.push(point);
    }

    fn take(&mut self) -> Option<draw::PenMotion> {
        let &end = self.samples.last()?;
        let bend = self.anchor.and_then(|anchor| {
            self.samples[..self.samples.len() - 1]
                .iter()
                .copied()
                .map(|point| (point, point.segment_distance_squared(anchor, end)))
                .max_by(|(_, first), (_, second)| first.total_cmp(second))
                .filter(|(bend, deviation)| {
                    *deviation >= PEN_BEND_DEVIATION_SQUARED && *bend != anchor && *bend != end
                })
                .map(|(bend, _)| bend)
        });
        self.anchor = Some(end);
        self.samples.clear();
        Some(draw::PenMotion { end, bend })
    }
}

pub struct State {
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
    pub fn setup_wayland(settings: crate::Settings) -> Result<(Self, EventQueue<Self>), String> {
        let connection = Connection::connect_to_env()
            .map_err(|error| format!("could not connect to Wayland: {error}"))?;
        let (globals, event_queue) = registry_queue_init::<State>(&connection)
            .map_err(|error| format!("Wayland setup failed: {error}"))?;
        let qhandle = event_queue.handle();
        let display = connection.display();
        let compositor = globals
            .bind::<WlCompositor, _, _>(&qhandle, 3..=6, ())
            .map_err(|_| "compositor does not provide wl_compositor version 3 or newer")?;
        let seat = globals
            .bind::<WlSeat, _, _>(&qhandle, 5..=9, ())
            .map_err(|_| "compositor does not provide wl_seat version 5 or newer")?;
        let layer_shell = globals
            .bind::<ZwlrLayerShellV1, _, _>(&qhandle, 1..=4, ())
            .map_err(|_| "compositor does not provide zwlr_layer_shell_v1")?;
        let cursor_shape_manager = globals
            .bind::<WpCursorShapeManagerV1, _, _>(&qhandle, 1..=1, ())
            .ok();
        let tablet_manager = globals
            .bind::<ZwpTabletManagerV2, _, _>(&qhandle, 1..=1, ())
            .ok();
        let xdg_output_manager = globals
            .bind::<ZxdgOutputManagerV1, _, _>(&qhandle, 1..=3, ())
            .ok();
        let viewporter = globals.bind::<WpViewporter, _, _>(&qhandle, 1..=1, ()).ok();
        let fractional_scale_manager = viewporter.as_ref().and_then(|_| {
            globals
                .bind::<WpFractionalScaleManagerV1, _, _>(&qhandle, 1..=1, ())
                .ok()
        });
        let text_input = globals
            .bind::<ZwpTextInputManagerV3, _, _>(&qhandle, 1..=2, ())
            .ok()
            .map(|manager| {
                let text_input = manager.get_text_input(&seat, &qhandle, ());
                manager.destroy();
                text_input
            });
        let output_globals = globals.contents().clone_list();
        let draw_on = settings.draw_on;
        let clear_on_escape = settings.clear_on_escape;

        let mut state = Self {
            fatal_error: None,
            active: false,
            draw_on,
            selected_output: None,
            input_output: None,
            keyboard_output: None,
            clear_on_escape,
            pending_pen_motion: PendingPenMotion::default(),
            wayland: WaylandState {
                _connection: connection,
                display,
                registry: globals.registry().clone(),
                compositor,
                seat,
                layer_shell,
                outputs: BTreeMap::new(),
                pointer: None,
                keyboard: None,
                text_input,
                cursor_shape_manager,
                tablet_manager,
                xdg_output_manager,
                viewporter,
                fractional_scale_manager,
            },
            draw: draw::DrawState::new(settings),
            keyboard: input::KeyboardState::default(),
            text_input: input::TextInputState::default(),
            pointer: input::PointerState::default(),
            tablet: input::TabletState::default(),
            gpu: None,
            qhandle,
        };

        for global in output_globals {
            if global.interface == WlOutput::interface().name {
                state.add_output(global.name, global.version);
            }
        }

        if let Some(manager) = &state.wayland.tablet_manager {
            state.tablet.set_tablet_seat(manager.get_tablet_seat(
                &state.wayland.seat,
                &state.qhandle,
                (),
            ));
        }

        Ok((state, event_queue))
    }

    fn add_output(&mut self, id: OutputId, version: u32) {
        use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
        use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;

        if self.wayland.outputs.contains_key(&id) {
            return;
        }
        let output =
            self.wayland
                .registry
                .bind::<WlOutput, _, _>(id, version.min(4), &self.qhandle, id);
        let xdg_output = self
            .wayland
            .xdg_output_manager
            .as_ref()
            .map(|manager| manager.get_xdg_output(&output, &self.qhandle, id));
        let surface = self.wayland.compositor.create_surface(&self.qhandle, id);
        let fractional_scale = self
            .wayland
            .fractional_scale_manager
            .as_ref()
            .map(|manager| manager.get_fractional_scale(&surface, &self.qhandle, id));
        let viewport = fractional_scale.as_ref().map(|_| {
            self.wayland
                .viewporter
                .as_ref()
                .unwrap()
                .get_viewport(&surface, &self.qhandle, ())
        });
        let layer_surface = self.wayland.layer_shell.get_layer_surface(
            &surface,
            Some(&output),
            Layer::Overlay,
            "vellum".into(),
            &self.qhandle,
            id,
        );
        layer_surface.set_anchor(Anchor::all());
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.set_exclusive_zone(-1);
        let empty_region = self.wayland.compositor.create_region(&self.qhandle, ());
        surface.set_input_region(Some(&empty_region));
        empty_region.destroy();
        surface.commit();

        self.wayland.outputs.insert(
            id,
            Output {
                output,
                xdg_output,
                origin: Point::default(),
                logical_size: [0; 2],
                integer_scale: 1,
                preferred_scale: None,
                fractional_scale,
                viewport,
                surface,
                layer_surface,
                frame_pending: false,
                wgpu: None,
            },
        );
        self.draw.add_output(id);
        if self.keyboard_output.is_none() {
            self.keyboard_output = Some(id);
        }
        if self.active {
            self.update_output_input();
        }
    }

    fn remove_output(&mut self, id: OutputId) {
        let pointer_position = self.pointer.remove_output(id);
        let pointer_owned_gesture = !self.tablet.input_grab_active();
        let tablet_position = self.tablet.remove_output(id);
        if let Some(pos) = tablet_position.or(pointer_position.filter(|_| pointer_owned_gesture)) {
            self.pointer_up(pos, self.modifiers(), false);
        }
        let Some(mut output) = self.wayland.outputs.remove(&id) else {
            return;
        };
        output.wgpu.take();
        if let Some(scale) = output.fractional_scale {
            scale.destroy();
        }
        if let Some(viewport) = output.viewport {
            viewport.destroy();
        }
        if let Some(xdg_output) = output.xdg_output {
            xdg_output.destroy();
        }
        output.layer_surface.destroy();
        output.surface.destroy();
        if output.output.version() >= 3 {
            output.output.release();
        }
        self.draw.remove_output(id);
        self.text_input_output_removed(id);

        if self.selected_output == Some(id) {
            self.selected_output = None;
        }
        if self.input_output == Some(id) {
            self.input_output = self.wayland.outputs.keys().next().copied();
        }
        if self.keyboard_output == Some(id) {
            self.keyboard_output = self.wayland.outputs.keys().next().copied();
        }
        if self.active {
            self.update_output_input();
        }
        self.refresh_cursor();
    }

    fn focus_output(&mut self, output: OutputId) {
        if !self.active || !self.wayland.outputs.contains_key(&output) {
            return;
        }
        let selection_changed =
            self.draw_on == DrawOn::Current && self.selected_output != Some(output);
        if self.draw_on == DrawOn::Current {
            if let Some(selected) = self.selected_output
                && selected != output
            {
                return;
            }
            self.selected_output = Some(output);
        }
        self.input_output = Some(output);
        if self.draw.is_editing_text() {
            return;
        }
        if self.keyboard_output != Some(output) || selection_changed {
            self.keyboard_output = Some(output);
            self.update_output_input();
        }
    }

    fn restore_keyboard_focus(&mut self) {
        if self.draw.is_editing_text() {
            return;
        }
        self.focus_keyboard_on_input();
    }

    fn focus_keyboard_on_input(&mut self) {
        let Some(output) = self
            .input_output
            .filter(|output| self.wayland.outputs.contains_key(output))
        else {
            return;
        };
        if self.keyboard_output != Some(output) {
            self.keyboard_output = Some(output);
            self.update_output_input();
        }
    }

    fn output_for_surface(&self, surface: &WlSurface) -> Option<OutputId> {
        surface
            .data::<OutputId>()
            .copied()
            .filter(|output| self.wayland.outputs.contains_key(output))
    }

    fn output_origin(&self, output: OutputId) -> Point {
        self.wayland
            .outputs
            .get(&output)
            .map_or_else(Point::default, |output| output.origin)
    }

    fn set_output_origin(&mut self, output: OutputId, origin: Point) {
        let Some(output_state) = self.wayland.outputs.get_mut(&output) else {
            return;
        };
        if output_state.origin == origin {
            return;
        }
        output_state.origin = origin;
        self.draw.damage(output);
        self.request_render();
    }

    fn update_output_input(&mut self) {
        let empty_region = self.wayland.compositor.create_region(&self.qhandle, ());
        let input_grab_active = self.pointer.input_grab_active() || self.tablet.input_grab_active();
        for (&id, output) in &self.wayland.outputs {
            let accepts_input = self.active
                && match self.draw_on {
                    DrawOn::All => true,
                    DrawOn::Current => {
                        input_grab_active
                            || self.selected_output.is_none_or(|selected| selected == id)
                    }
                };
            output.surface.set_input_region(if accepts_input {
                None
            } else {
                Some(&empty_region)
            });
            output.layer_surface.set_keyboard_interactivity(
                if accepts_input && self.keyboard_output == Some(id) {
                    KeyboardInteractivity::Exclusive
                } else {
                    KeyboardInteractivity::None
                },
            );
            output.surface.commit();
        }
        empty_region.destroy();
    }

    pub fn toggle_input(&mut self) {
        self.set_input_active(!self.active);
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn is_text_editing(&self) -> bool {
        self.draw.is_editing_text()
    }

    pub fn set_input_active(&mut self, active: bool) {
        if active == self.active {
            return;
        }
        if active {
            self.activate();
        } else {
            self.deactivate();
        }
    }

    pub fn activate(&mut self) {
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

    pub fn deactivate(&mut self) {
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

    fn render(&mut self, output: OutputId) {
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

    fn resize_output(&mut self, id: OutputId) {
        let Some(output) = self.wayland.outputs.get_mut(&id) else {
            return;
        };
        if output.logical_size.contains(&0) {
            return;
        }
        output.configure_scale();
        let [width, height] = output.buffer_size();
        if let Some(wgpu) = &mut output.wgpu {
            if let Err(error) = wgpu.resize(width, height) {
                self.fatal_error = Some(error);
                return;
            }
            self.draw.damage(id);
            self.request_render();
        }
    }

    fn apply_action(&mut self, action: Action) {
        self.flush_pen_motion();
        let clear_on_escape = self.clear_on_escape && matches!(action, Action::Cancel);
        let anchor = self
            .pointer
            .position()
            .map(|(x, y)| Point::new(x as f32, y as f32));
        let effect = self.draw.handle_action(action, anchor);
        if effect.changed {
            self.request_render();
        }
        if effect.deactivate {
            if clear_on_escape {
                self.clear();
            }
            self.deactivate();
        }
        self.restore_keyboard_focus();
        self.refresh_cursor();
    }

    pub fn clear(&mut self) {
        self.apply_action(Action::Clear);
    }

    pub fn set_current_color(&mut self, color: DynamicColor) {
        let rgba = crate::color_to_srgb(color);
        let render = self.draw.set_current_color(rgba);
        if render {
            self.request_render();
        }
        self.refresh_cursor();
    }

    fn modifiers(&self) -> Modifiers {
        self.keyboard.modifiers()
    }

    fn modifiers_changed(&mut self) {
        self.flush_pen_motion();
        let modifiers = self.modifiers();
        if self.draw.modifiers_changed(modifiers) {
            self.request_render();
        }
        self.refresh_cursor();
    }

    fn pointer_down(
        &mut self,
        (x, y): (f64, f64),
        modifiers: Modifiers,
        tool_override: draw::ToolOverride,
    ) {
        let point = Point::new(x as f32, y as f32);
        self.pending_pen_motion.reset(None);
        if self.draw.picker_active() {
            if tool_override == draw::ToolOverride::Eraser {
                self.dismiss_picker();
            } else {
                if self.draw.picker_motion(point) {
                    self.request_render();
                }
                return;
            }
        }
        self.focus_keyboard_on_input();
        let changed = self.draw.pointer_down(point, modifiers, tool_override);
        if self.draw.is_drawing_pen() {
            self.pending_pen_motion.reset(Some(point));
        }
        if changed {
            self.request_render();
        }
        self.restore_keyboard_focus();
    }

    fn pointer_motion(&mut self, (x, y): (f64, f64), modifiers: Modifiers) {
        let point = Point::new(x as f32, y as f32);
        if self.draw.picker_active() {
            if self.draw.picker_motion(point) {
                self.request_render();
            }
            return;
        }
        // Preserve one real bend inside each display frame without letting a
        // high-polling-rate device grow the stroke without bound.
        if self.draw.is_drawing_pen() {
            if self.pending_pen_motion.modifiers != modifiers {
                self.flush_pen_motion();
            }
            self.pending_pen_motion.modifiers = modifiers;
            self.pending_pen_motion.push(point);
            self.request_render();
            return;
        }
        if self.draw.pointer_motion(point, modifiers) {
            self.request_render();
        }
    }

    fn pointer_up(&mut self, (x, y): (f64, f64), modifiers: Modifiers, latch_picker: bool) {
        self.flush_pen_motion();
        let point = Point::new(x as f32, y as f32);
        if self.draw.picker_active() {
            if self.draw.picker_release(point, latch_picker) {
                self.request_render();
                self.restore_keyboard_focus();
            }
            return;
        }
        if self.draw.pointer_up(point, modifiers) {
            self.request_render();
        }
        self.pending_pen_motion.reset(None);
    }

    fn open_picker(&mut self, (x, y): (f64, f64)) {
        self.draw.open_picker(Point::new(x as f32, y as f32));
        self.request_render();
    }

    fn toggle_picker(&mut self, pos: (f64, f64)) {
        if self.draw.picker_active() {
            self.dismiss_picker();
        } else {
            self.open_picker(pos);
        }
    }

    fn dismiss_picker(&mut self) {
        if self.draw.dismiss_picker() {
            self.request_render();
        }
    }

    fn text_click_at(&mut self, (x, y): (f64, f64), clicks: u8) -> bool {
        let Some(changed) = self
            .draw
            .text_click_at(Point::new(x as f32, y as f32), clicks)
        else {
            return false;
        };
        self.focus_keyboard_on_input();
        if changed {
            self.request_render();
        }
        true
    }

    fn adjust(&mut self, steps: f32, (x, y): (f64, f64), modifiers: Modifiers) -> bool {
        let hit_stop = self
            .draw
            .adjust(steps, Point::new(x as f32, y as f32), modifiers);
        self.request_render();
        hit_stop
    }

    fn refresh_cursor(&mut self) {
        if !self.active {
            self.clear_tool_cursor();
            return;
        }
        if !self.pointer.input_grab_active()
            && let Some(preview_changed) = self.tablet.refresh_cursor(&mut self.draw)
        {
            if preview_changed {
                self.request_render();
            }
            return;
        }
        let (Some((x, y)), Some(pointer)) = (self.pointer.position(), &self.wayland.pointer) else {
            self.clear_tool_cursor();
            return;
        };
        let point = Point::new(x as f32, y as f32);
        let cursor = self.draw.cursor(point, self.pointer.tool_override());
        let preview_changed = self.draw.set_tool_cursor(match cursor {
            Cursor::Tool(preview) if self.pointer.tool_cursor_supported() => Some((point, preview)),
            _ => None,
        });
        self.pointer.refresh_cursor(pointer, cursor);
        if preview_changed {
            self.request_render();
        }
    }

    fn clear_tool_cursor(&mut self) {
        if self.draw.set_tool_cursor(None) {
            self.request_render();
        }
    }

    fn request_render(&mut self) {
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

    fn flush_pen_motion(&mut self) {
        if let Some(motion) = self.pending_pen_motion.take() {
            let modifiers = self.pending_pen_motion.modifiers;
            self.draw.pen_motion(motion, modifiers);
        }
    }

    pub fn next_wakeup(&self) -> Option<std::time::Instant> {
        [self.draw.next_wakeup(), self.keyboard.next_wakeup()]
            .into_iter()
            .flatten()
            .min()
    }

    pub fn handle_timeouts(&mut self, now: std::time::Instant) {
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

struct WaylandState {
    _connection: Connection,
    display: WlDisplay,
    registry: WlRegistry,
    compositor: WlCompositor,
    seat: WlSeat,
    layer_shell: ZwlrLayerShellV1,
    outputs: BTreeMap<OutputId, Output>,
    pointer: Option<WlPointer>,
    keyboard: Option<WlKeyboard>,
    text_input: Option<ZwpTextInputV3>,

    cursor_shape_manager: Option<WpCursorShapeManagerV1>,
    tablet_manager: Option<ZwpTabletManagerV2>,
    xdg_output_manager: Option<ZxdgOutputManagerV1>,
    viewporter: Option<WpViewporter>,
    fractional_scale_manager: Option<WpFractionalScaleManagerV1>,
}

struct Output {
    output: WlOutput,
    xdg_output: Option<ZxdgOutputV1>,
    origin: Point,
    logical_size: [u32; 2],
    integer_scale: i32,
    preferred_scale: Option<u32>,
    fractional_scale: Option<WpFractionalScaleV1>,
    viewport: Option<WpViewport>,
    surface: WlSurface,
    layer_surface: ZwlrLayerSurfaceV1,
    frame_pending: bool,
    wgpu: Option<WgpuState>,
}

impl Output {
    fn buffer_size(&self) -> [u32; 2] {
        let scale = self
            .preferred_scale
            .map_or(f64::from(self.integer_scale), |scale| {
                f64::from(scale) / 120.0
            });
        self.logical_size
            .map(|size| (f64::from(size) * scale).round() as u32)
    }

    fn render_scale(&self) -> [f64; 2] {
        let size = self.buffer_size();
        [
            f64::from(size[0]) / f64::from(self.logical_size[0]),
            f64::from(size[1]) / f64::from(self.logical_size[1]),
        ]
    }

    fn configure_scale(&self) {
        if let Some(viewport) = &self.viewport {
            self.surface.set_buffer_scale(1);
            viewport.set_destination(self.logical_size[0] as i32, self.logical_size[1] as i32);
        } else {
            self.surface.set_buffer_scale(self.integer_scale);
        }
    }
}

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlRegion);
delegate_noop!(State: ignore ZwpTextInputManagerV3);
delegate_noop!(State: ignore WpViewporter);
delegate_noop!(State: ignore WpViewport);
delegate_noop!(State: ignore WpFractionalScaleManagerV1);

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        _registry: &WlRegistry,
        event: <WlRegistry as Proxy>::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_registry::Event;
        match event {
            Event::Global {
                name,
                interface,
                version,
            } if interface == WlOutput::interface().name => state.add_output(name, version),
            Event::GlobalRemove { name } => state.remove_output(name),
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, OutputId> for State {
    fn event(
        state: &mut Self,
        _proxy: &WlOutput,
        event: <WlOutput as Proxy>::Event,
        output: &OutputId,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_output::Event;
        if let Event::Scale { factor } = event {
            if let Some(output) = state.wayland.outputs.get_mut(output) {
                output.integer_scale = factor.max(1);
            }
            state.resize_output(*output);
        }
        if let Event::Geometry { x, y, .. } = event
            && state
                .wayland
                .outputs
                .get(output)
                .is_some_and(|output| output.xdg_output.is_none())
        {
            state.set_output_origin(*output, Point::new(x as f32, y as f32));
        }
    }
}

delegate_noop!(State: ignore ZxdgOutputManagerV1);

impl Dispatch<WpFractionalScaleV1, OutputId> for State {
    fn event(
        state: &mut Self,
        _proxy: &WpFractionalScaleV1,
        event: <WpFractionalScaleV1 as Proxy>::Event,
        output: &OutputId,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            if let Some(output) = state.wayland.outputs.get_mut(output) {
                output.preferred_scale = Some(scale.max(1));
            }
            state.resize_output(*output);
        }
    }
}

impl Dispatch<ZxdgOutputV1, OutputId> for State {
    fn event(
        state: &mut Self,
        _proxy: &ZxdgOutputV1,
        event: <ZxdgOutputV1 as Proxy>::Event,
        output: &OutputId,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::Event;
        if let Event::LogicalPosition { x, y } = event {
            state.set_output_origin(*output, Point::new(x as f32, y as f32));
        }
    }
}

impl Dispatch<WlSurface, OutputId> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlSurface,
        _event: <WlSurface as Proxy>::Event,
        _data: &OutputId,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: <WlSeat as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qhandle: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_seat::Capability;
        use wayland_client::protocol::wl_seat::Event;
        let Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        else {
            return;
        };
        if capabilities.contains(Capability::Pointer) && state.wayland.pointer.is_none() {
            let pointer = seat.get_pointer(qhandle, ());
            let shape_device = state
                .wayland
                .cursor_shape_manager
                .as_ref()
                .map(|manager| manager.get_pointer(&pointer, qhandle, ()));
            state.pointer.set_cursor_device(shape_device);
            state.wayland.pointer = Some(pointer);
        } else if !capabilities.contains(Capability::Pointer)
            && let Some(pointer) = state.wayland.pointer.take()
        {
            if state.pointer.input_grab_active()
                && !state.tablet.input_grab_active()
                && let Some(pos) = state.pointer.position()
            {
                state.pointer_up(pos, state.modifiers(), false);
            }
            pointer.release();
            state.pointer.clear_pointer();
            state.refresh_cursor();
            state.update_output_input();
        }
        if capabilities.contains(Capability::Keyboard) && state.wayland.keyboard.is_none() {
            state.wayland.keyboard = Some(seat.get_keyboard(qhandle, ()));
        } else if !capabilities.contains(Capability::Keyboard)
            && let Some(keyboard) = state.wayland.keyboard.take()
        {
            state.keyboard.clear();
            keyboard.release();
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

impl Dispatch<WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _callback: &WlCallback,
        _event: <WlCallback as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let globals = state
            .wayland
            .registry
            .data::<GlobalListContents>()
            .unwrap()
            .clone_list();
        for global in globals {
            if global.interface == WlOutput::interface().name {
                state.add_output(global.name, global.version);
            }
        }
    }
}

delegate_noop!(State: ignore ZwlrLayerShellV1);
impl Dispatch<ZwlrLayerSurfaceV1, OutputId> for State {
    fn event(
        state: &mut Self,
        layer_surface: &ZwlrLayerSurfaceV1,
        event: <ZwlrLayerSurfaceV1 as Proxy>::Event,
        output: &OutputId,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Event;
        match event {
            Event::Configure {
                serial,
                width,
                height,
            } => {
                layer_surface.ack_configure(serial);
                let Some(output_state) = state.wayland.outputs.get_mut(output) else {
                    return;
                };
                if width == 0 || height == 0 {
                    return;
                }
                output_state.logical_size = [width, height];
                if output_state.wgpu.is_some() {
                    state.resize_output(*output);
                } else {
                    output_state.configure_scale();
                    let [width, height] = output_state.buffer_size();
                    let surface = output_state.surface.clone();
                    let display = state.wayland.display.clone();
                    let wgpu = if let Some(gpu) = &state.gpu {
                        gpu.create_surface(&display, &surface)
                            .and_then(|surface| WgpuState::new(gpu, surface, width, height))
                    } else {
                        GpuContext::new(&display, &surface, width, height).map(|(gpu, wgpu)| {
                            state.gpu = Some(gpu);
                            wgpu
                        })
                    };
                    match wgpu {
                        Ok(wgpu) => output_state.wgpu = Some(wgpu),
                        Err(error) => {
                            state.fatal_error = Some(error);
                            return;
                        }
                    }

                    // Some compositors require a buffer with the initial configure.
                    state.render(*output);
                }
            }
            Event::Closed => {
                state.remove_output(*output);
                // A closed layer surface does not necessarily mean its output is gone.
                // Wait for accompanying registry removals before recreating surfaces.
                state.wayland.display.sync(&state.qhandle, ());
            }
            _ => {}
        }
    }
}

delegate_dispatch!(State: [WlPointer: ()] => input::PointerState);

delegate_noop!(State: ignore WpCursorShapeManagerV1);
delegate_noop!(State: ignore WpCursorShapeDeviceV1);

delegate_noop!(State: ignore ZwpTabletManagerV2);
delegate_dispatch!(State: [ZwpTabletSeatV2: ()] => input::TabletState);
delegate_dispatch!(State: [ZwpTabletV2: ()] => input::TabletState);
delegate_dispatch!(State: [ZwpTabletToolV2: ()] => input::TabletState);
delegate_dispatch!(State: [ZwpTabletPadV2: ()] => input::TabletState);
delegate_dispatch!(State: [ZwpTabletPadGroupV2: ()] => input::TabletState);
delegate_noop!(State: ignore ZwpTabletPadRingV2);
delegate_noop!(State: ignore ZwpTabletPadStripV2);
