//! Assembles visible annotations and transient decorations for an output.

use super::decorations::{
    text_caret, text_cursor_rectangle, text_preedit_span, tool_cursor_geometry,
};
use super::scene::{self, ElementKind};
use super::{DrawState, Feedback, Point, TextInputSnapshot};
use crate::OutputId;
use crate::render::{SceneItem, TextSpec, Viewport, WgpuState};

impl DrawState {
    pub fn render(
        &mut self,
        output: OutputId,
        origin: Point,
        scale: [f64; 2],
        wgpu: &mut WgpuState,
        before_present: impl FnOnce(Option<TextInputSnapshot<'_>>),
    ) -> Result<(), String> {
        let Some(damage) = self.outputs.get_mut(&output).filter(|damage| damage.dirty) else {
            return Ok(());
        };
        let size = wgpu.size();
        let viewport = kurbo::Rect::new(
            f64::from(origin.x),
            f64::from(origin.y),
            f64::from(origin.x) + f64::from(size[0]) / scale[0],
            f64::from(origin.y) + f64::from(size[1]) / scale[1],
        );
        damage.viewport = Some(viewport);
        let visible = scene::Bounds {
            min: origin,
            max: Point::new(viewport.x1 as f32, viewport.y1 as f32),
        }
        .expanded(1.0);
        let active_text = self.editor.text_edit();
        let items = {
            let mut items = Vec::with_capacity(self.editor.elements().len());
            for element in self.editor.elements() {
                // Layout bounds do not include all glyph ink overhangs; keep text conservative.
                if !matches!(element.kind, ElementKind::Text { .. })
                    && !self
                        .editor
                        .element_bounds_preview(element)
                        .intersects(visible)
                {
                    continue;
                }
                if let Some(edit) = active_text.filter(|edit| edit.id == Some(element.id)) {
                    items.push(SceneItem::Text(edit.spec()));
                    continue;
                }
                let (kind, style) = self
                    .editor
                    .text_resize_preview(element.id)
                    .unwrap_or((&element.kind, element.style));
                let ElementKind::Text {
                    origin,
                    content,
                    scale,
                } = kind
                else {
                    items.push(SceneItem::Geometry(
                        self.editor
                            .element_geometry_preview(element)
                            .map(std::borrow::Cow::Owned)
                            .unwrap_or(std::borrow::Cow::Borrowed(&element.geometry)),
                    ));
                    continue;
                };
                let offset = self.editor.moving_offset(element.id).unwrap_or_default();
                items.push(SceneItem::Text(TextSpec {
                    key: element.id,
                    content,
                    left: origin.x + offset.x,
                    top: origin.y + offset.y,
                    font_size: style.size,
                    color: style.color,
                    background_roundness: style.filled.then_some(style.roundness),
                    scale: *scale,
                }));
            }
            if let Some(edit) = active_text.filter(|edit| edit.id.is_none()) {
                items.push(SceneItem::Text(edit.spec()));
            }
            if let Some(Feedback {
                text: content,
                anchor: at,
                ..
            }) = &self.feedback
            {
                for [x, y] in [[15.0, 16.0], [17.0, 16.0], [16.0, 15.0], [16.0, 17.0]].into_iter() {
                    items.push(SceneItem::Text(TextSpec {
                        key: u64::MAX - 30,
                        content,
                        left: at.x + x,
                        top: at.y + y,
                        font_size: 18.0,
                        color: [0.0, 0.0, 0.0, 0.9],
                        background_roundness: None,
                        scale: [1.0; 2],
                    }));
                }
                items.push(SceneItem::Text(TextSpec {
                    key: u64::MAX - 30,
                    content,
                    left: at.x + 16.0,
                    top: at.y + 16.0,
                    font_size: 18.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                    background_roundness: None,
                    scale: [1.0; 2],
                }));
            }
            items
        };

        self.previews.clear();
        let mut cursor_rectangle = None;
        if let Some(edit) = self.editor.text_edit() {
            let [scale_x, scale_y] = edit.scale;
            let [x, y] = edit.cursor_position();
            cursor_rectangle = Some(text_cursor_rectangle(
                edit.origin,
                edit.ime_area(),
                edit.scale,
                origin,
            ));
            if self.caret_visible && edit.shows_caret() {
                self.previews.push(text_caret(
                    edit.origin.x + x * scale_x,
                    edit.origin.y + y * scale_y,
                    edit.style.size * scale_y,
                ));
            }
            edit.decoration_rectangles(|[x, y, width, height], style| {
                self.previews.push(text_preedit_span(
                    edit.origin.x + x * scale_x,
                    edit.origin.x + (x + width) * scale_x,
                    edit.origin.y + y * scale_y,
                    height * scale_y,
                    style,
                ));
            });
        }
        self.editor.append_preview_geometry(&mut self.previews);
        self.editor
            .append_selection_geometry(self.feedback.is_none(), &mut self.previews);
        if let Some((point, cursor)) = self.tool_cursor {
            self.previews.push(tool_cursor_geometry(point, cursor));
        }
        self.previews.retain(|geometry| {
            geometry
                .bounds()
                .is_some_and(|bounds| bounds.inflate(1.0, 1.0).intersect(viewport).area() > 0.0)
        });
        let picker = self.editor.picker_geometry();
        if wgpu.render(
            &items,
            &self.previews,
            picker.as_ref(),
            Viewport {
                origin: [origin.x, origin.y],
                scale,
            },
            self.editor
                .text_edit()
                .map(|edit| (edit.id.unwrap_or(0), edit.layout())),
            || {
                before_present(
                    self.editor
                        .text_edit()
                        .map(|edit| edit.snapshot(cursor_rectangle)),
                );
            },
        )? {
            damage.dirty = false;
        }
        Ok(())
    }
}
