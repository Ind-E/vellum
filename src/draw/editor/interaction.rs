use super::{Editor, HistoryEntry, drawing_kind};
use crate::draw::scene::{Bounds, Element, ElementId, ElementKind, Point, Style, bounds_for};
use crate::draw::selection::{self, Handle};
use crate::draw::text_edit::TextEdit;
use crate::draw::{Cursor, Modifiers, PenMotion, ToolCursor, ToolOverride, freehand};
use crate::text::text_line_height;
use crate::tool::Tool;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ResizeSnapshot {
    pub(super) kind: ElementKind,
    pub(super) style: Style,
    pub(super) bounds: Bounds,
}

impl From<&Element> for ResizeSnapshot {
    fn from(element: &Element) -> Self {
        Self {
            kind: element.kind.clone(),
            style: element.style,
            bounds: element.bounds,
        }
    }
}

impl ResizeSnapshot {
    pub(super) fn set_style(&mut self, style: Style) {
        if !matches!(self.kind, ElementKind::Text { .. })
            || style.size != self.style.size
            || style.filled != self.style.filled
            || (style.filled && style.roundness != self.style.roundness)
        {
            self.bounds = bounds_for(&self.kind, style);
        }
        self.style = style;
    }
}

#[derive(Debug)]
pub(super) enum Interaction {
    Freehand(freehand::LiveStroke),
    Drawing {
        tool: Tool,
        start: Point,
        current: Point,
        modifiers: Modifiers,
    },
    Moving {
        ids: Vec<ElementId>,
        start: Point,
        current: Point,
    },
    Resizing {
        id: ElementId,
        handle: Handle,
        start: Point,
        point: Point,
        original: ResizeSnapshot,
        properties: Style,
        current: ResizeSnapshot,
        equal_side_anchor: Option<usize>,
    },
    EditingText(TextEdit),
    Erasing {
        previous: Point,
    },
}

impl Editor {
    pub(in crate::draw) fn modifiers_changed(&mut self, modifiers: Modifiers) -> bool {
        let text_size_range = self.text_size_range();
        match &mut self.interaction {
            Some(Interaction::Drawing {
                modifiers: current, ..
            }) if *current != modifiers => {
                *current = modifiers;
                true
            }
            Some(Interaction::Resizing {
                handle,
                start,
                point,
                original,
                properties,
                current,
                equal_side_anchor,
                ..
            }) => {
                let resized = resize_element(
                    original,
                    *handle,
                    *point - *start,
                    modifiers,
                    equal_side_anchor,
                    text_size_range,
                    *properties,
                );
                if resized == *current {
                    false
                } else {
                    *current = resized;
                    true
                }
            }
            _ => false,
        }
    }

    pub(in crate::draw) fn pointer_down(
        &mut self,
        point: Point,
        modifiers: Modifiers,
        tool_override: ToolOverride,
    ) -> bool {
        if tool_override == ToolOverride::None
            && let Some(edit) = self.text_edit_mut()
            && edit.bounds().contains(point)
        {
            return edit.click(point, 1, modifiers.shift);
        }
        let previous = self.finish_interaction();
        let effective_tool = tool_override.effective_tool(self.tool);
        if effective_tool == Tool::Eraser {
            self.interaction = Some(Interaction::Erasing { previous: point });
            return previous | self.erase_between(point, point);
        }

        match effective_tool {
            Tool::Pen => {
                self.interaction = Some(Interaction::Freehand(freehand::LiveStroke::new(
                    point,
                    self.style_for(effective_tool),
                )));
                true
            }
            Tool::Line | Tool::Arrow | Tool::Triangle | Tool::Rectangle | Tool::Ellipse => {
                self.interaction = Some(Interaction::Drawing {
                    tool: effective_tool,
                    start: point,
                    current: point,
                    modifiers,
                });
                true
            }
            Tool::Text => {
                let origin = Point::new(point.x, point.y - text_line_height(self.style.size) * 0.5);
                let edit = self.make_text_edit(None, origin, String::new(), self.style, [1.0; 2]);
                self.interaction = Some(Interaction::EditingText(edit));
                true
            }
            Tool::Select => {
                if !modifiers.ctrl
                    && self.selected.len() == 1
                    && let Some(id) = self.selected.first().copied()
                    && let Some(handle) = self.hit_handle(id, point)
                    && let Some(element) = self.element(id)
                {
                    let original = ResizeSnapshot::from(element);
                    self.interaction = Some(Interaction::Resizing {
                        id,
                        handle,
                        start: point,
                        point,
                        current: original.clone(),
                        properties: original.style,
                        original,
                        equal_side_anchor: None,
                    });
                    return true;
                }
                let hit = self.hit_test(point);
                if modifiers.ctrl {
                    if let Some(id) = hit {
                        if let Some(index) =
                            self.selected.iter().position(|selected| *selected == id)
                        {
                            self.selected.remove(index);
                        } else {
                            self.selected.push(id);
                        }
                        return true;
                    }
                    return previous;
                }
                let changed = hit.is_none_or(|id| !self.selected.contains(&id));
                if let Some(id) = hit {
                    if changed {
                        self.selected.clear();
                        self.selected.push(id);
                    }
                    let mut ids = self.selected.clone();
                    ids.sort_unstable();
                    self.interaction = Some(Interaction::Moving {
                        ids,
                        start: point,
                        current: point,
                    });
                } else {
                    self.selected.clear();
                }
                if hit.is_some() {
                    true
                } else {
                    previous | changed
                }
            }
            Tool::Eraser => unreachable!(),
        }
    }

    pub(in crate::draw) fn pointer_motion(&mut self, point: Point, modifiers: Modifiers) -> bool {
        let text_size_range = self.text_size_range();
        match &mut self.interaction {
            Some(Interaction::EditingText(edit)) => edit.drag(point),
            Some(Interaction::Drawing {
                current,
                modifiers: current_modifiers,
                ..
            }) => {
                *current = point;
                *current_modifiers = modifiers;
                true
            }
            Some(Interaction::Moving { current, .. }) => {
                *current = point;
                true
            }
            Some(Interaction::Resizing {
                handle,
                start,
                point: current_point,
                original,
                properties,
                current,
                equal_side_anchor,
                ..
            }) => {
                *current = resize_element(
                    original,
                    *handle,
                    point - *start,
                    modifiers,
                    equal_side_anchor,
                    text_size_range,
                    *properties,
                );
                *current_point = point;
                true
            }
            Some(Interaction::Erasing { previous }) => {
                let start = std::mem::replace(previous, point);
                self.erase_between(start, point)
            }
            Some(Interaction::Freehand(_)) | None => false,
        }
    }

    pub(in crate::draw) fn pen_motion(&mut self, motion: PenMotion, modifiers: Modifiers) -> bool {
        let Some(Interaction::Freehand(stroke)) = &mut self.interaction else {
            return false;
        };
        stroke.push_motion(motion, modifiers.shift)
    }

    pub(in crate::draw) fn pointer_up(&mut self, point: Point, modifiers: Modifiers) -> bool {
        if let Some(edit) = self.text_edit_mut() {
            return edit.end_drag(point);
        }
        let text_size_range = self.text_size_range();
        match self.interaction.take() {
            Some(Interaction::Freehand(stroke)) => {
                let (points, style, geometry) = stroke.finish(point, modifiers.shift);
                self.insert_element(Element::with_geometry(
                    self.next_id,
                    ElementKind::Freehand { points },
                    style,
                    geometry,
                ));
                true
            }
            Some(Interaction::Drawing { tool, start, .. }) => {
                self.insert_kind(drawing_kind(tool, start, point, modifiers), self.style);
                true
            }
            Some(Interaction::Moving { ids, start, .. }) => {
                if point != start {
                    let delta = point - start;
                    let mut elements = Vec::with_capacity(ids.len());
                    for id in ids {
                        let element = self.element_mut(id).expect("moving element exists");
                        let after = element.kind.translated(delta);
                        let style = element.style;
                        let (kind, style) = element.replace(after, style);
                        elements.push((id, kind, style));
                    }
                    if !elements.is_empty() {
                        self.history.record(HistoryEntry::Update(elements));
                    }
                }
                true
            }
            Some(Interaction::Resizing {
                id,
                handle,
                start,
                original,
                properties,
                mut equal_side_anchor,
                ..
            }) => {
                let current = resize_element(
                    &original,
                    handle,
                    point - start,
                    modifiers,
                    &mut equal_side_anchor,
                    text_size_range,
                    properties,
                );
                if current != original
                    && let Some(element) = self.element_mut(id)
                {
                    element.replace(current.kind, current.style);
                    self.history.record(HistoryEntry::Update(vec![(
                        id,
                        original.kind,
                        original.style,
                    )]));
                }
                true
            }
            Some(Interaction::Erasing { previous }) => self.erase_between(previous, point),
            interaction => {
                self.interaction = interaction;
                false
            }
        }
    }

    fn hit_handle(&self, id: ElementId, point: Point) -> Option<Handle> {
        let element = self.element(id)?;
        selection::hit_handle(&element.kind, element.style, element.bounds, point)
    }

    fn text_size_range(&self) -> [f32; 2] {
        let range = self
            .size_ranges
            .get(&Tool::Text)
            .expect("text has a size range");
        [range.min(), range.max()]
    }

    pub(in crate::draw) fn cursor(&self, point: Point, tool_override: ToolOverride) -> Cursor {
        let effective_tool = tool_override.effective_tool(self.tool);
        if tool_override != ToolOverride::None && effective_tool == Tool::Eraser {
            return self.tool_cursor(effective_tool);
        }
        match &self.interaction {
            Some(Interaction::Resizing { handle, .. }) => {
                return Cursor::Shape(selection::cursor(*handle));
            }
            Some(Interaction::Freehand(_)) => return Cursor::Hidden,
            Some(Interaction::Drawing { .. }) => {
                return Cursor::Shape(selection::CursorHint::Crosshair);
            }
            Some(Interaction::Moving { .. }) => {
                return Cursor::Shape(selection::CursorHint::Move);
            }
            Some(Interaction::EditingText(_)) => {
                return Cursor::Shape(selection::CursorHint::Text);
            }
            Some(Interaction::Erasing { .. }) => return self.tool_cursor(Tool::Eraser),
            _ => {}
        }
        if self.picker.is_some() {
            return Cursor::Shape(selection::CursorHint::Crosshair);
        }
        if effective_tool == Tool::Text {
            return Cursor::Shape(selection::CursorHint::Text);
        }
        if matches!(
            effective_tool,
            Tool::Line | Tool::Arrow | Tool::Triangle | Tool::Rectangle | Tool::Ellipse
        ) {
            return Cursor::Shape(selection::CursorHint::Crosshair);
        }
        if effective_tool != Tool::Select {
            return self.tool_cursor(effective_tool);
        }
        if self.selected.len() != 1 {
            return Cursor::Shape(selection::CursorHint::Crosshair);
        }
        let id = self.selected[0];
        Cursor::Shape(match self.hit_handle(id, point) {
            Some(handle) => selection::cursor(handle),
            None if self
                .element(id)
                .is_some_and(|element| element.hit_test(point)) =>
            {
                selection::CursorHint::Move
            }
            None => selection::CursorHint::Crosshair,
        })
    }

    fn tool_cursor(&self, tool: Tool) -> Cursor {
        let style = self.style_for(tool);
        Cursor::Tool(ToolCursor {
            tool,
            size: style.size,
            roundness: style.roundness,
            color: style.color,
        })
    }

    pub(super) fn cancel_interaction(&mut self) -> bool {
        self.interaction.take().is_some()
    }

    pub(super) fn finish_interaction(&mut self) -> bool {
        if self.is_editing_text() {
            self.commit_text()
        } else {
            self.cancel_interaction()
        }
    }
}

fn resize_element(
    original: &ResizeSnapshot,
    handle: Handle,
    delta: Point,
    modifiers: Modifiers,
    equal_side_anchor: &mut Option<usize>,
    text_size_range: [f32; 2],
    properties: Style,
) -> ResizeSnapshot {
    if matches!(original.kind, ElementKind::Text { .. }) {
        let (kind, style, bounds) = selection::resize_text(
            &original.kind,
            original.style,
            original.bounds,
            handle,
            delta,
            modifiers,
            text_size_range,
        );
        let mut resized = ResizeSnapshot {
            kind,
            style,
            bounds,
        };
        let size = (style.size + (properties.size - original.style.size))
            .clamp(text_size_range[0], text_size_range[1]);
        resized.set_style(Style { size, ..properties });
        return resized;
    }
    let kind = selection::resize(
        &original.kind,
        handle,
        delta,
        original.style.roundness,
        modifiers,
        equal_side_anchor,
    );
    ResizeSnapshot {
        bounds: bounds_for(&kind, properties),
        kind,
        style: properties,
    }
}
