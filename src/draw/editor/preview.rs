use super::interaction::ResizeSnapshot;
use super::{Editor, Interaction, drawing_kind};
use crate::draw::picker::{ShapeFills, picker_geometry};
use crate::draw::scene::{ElementId, ElementKind, Point, geometry};
use crate::draw::selection;
use crate::render::Geometry;
use crate::tool::Tool;

impl Editor {
    pub(in crate::draw) fn append_preview_geometry(&self, output: &mut Vec<Geometry>) {
        match &self.interaction {
            Some(Interaction::Freehand(stroke)) => output.push(stroke.tail_geometry()),
            Some(Interaction::Drawing {
                tool,
                start,
                current,
                modifiers,
            }) => output.push(geometry(
                &drawing_kind(*tool, *start, *current, *modifiers),
                self.style,
            )),
            _ => {}
        }
    }

    pub(in crate::draw) fn append_selection_geometry(
        &self,
        show_handles: bool,
        output: &mut Vec<Geometry>,
    ) {
        if self.tool != Tool::Select {
            return;
        }
        if self.selected.len() > 1 {
            let mut bounds: Option<(Point, Point)> = None;
            for id in &self.selected {
                let Some(element) = self.element(*id) else {
                    continue;
                };
                let offset = self.moving_offset(*id).unwrap_or_default();
                let (min, max) = (element.bounds.min + offset, element.bounds.max + offset);
                bounds = Some(bounds.map_or((min, max), |(current_min, current_max)| {
                    (
                        Point::new(current_min.x.min(min.x), current_min.y.min(min.y)),
                        Point::new(current_max.x.max(max.x), current_max.y.max(max.y)),
                    )
                }));
            }
            if let Some((min, max)) = bounds {
                output.push(selection::outline(min, max));
            }
            return;
        }
        let Some(&id) = self.selected.first() else {
            return;
        };
        let Some(element) = self.element(id) else {
            return;
        };
        if let Some(edit) = self.text_edit().filter(|edit| edit.id == Some(id)) {
            let bounds = edit.bounds();
            output.push(selection::outline(bounds.min, bounds.max));
            return;
        }
        let preview = self.resize_preview(id);
        let (kind, bounds) = preview.map_or((&element.kind, element.bounds), |current| {
            (&current.kind, current.bounds)
        });
        let offset = self.moving_offset(id).unwrap_or_default();
        if !matches!(
            kind,
            ElementKind::Segment { .. } | ElementKind::Triangle { .. }
        ) {
            output.push(selection::outline(bounds.min + offset, bounds.max + offset));
        }
        if !show_handles || self.interaction.is_some() {
            return;
        }
        selection::append_handles(kind, element.style, output);
    }

    pub(in crate::draw) fn picker_geometry(
        &self,
        viewport: kurbo::Rect,
    ) -> Option<crate::render::LocalGeometry> {
        let picker = self.picker?;
        let active = self.color_tool();
        picker_geometry(
            picker.center,
            picker.hovered,
            active,
            self.current_color(),
            ShapeFills {
                triangle: self.tool_fill(Tool::Triangle),
                rectangle: self.tool_fill(Tool::Rectangle),
                ellipse: self.tool_fill(Tool::Ellipse),
            },
            &self.palette,
            viewport,
        )
    }

    pub(in crate::draw) fn resize_preview(&self, id: ElementId) -> Option<&ResizeSnapshot> {
        match &self.interaction {
            Some(Interaction::Resizing {
                id: resized,
                current,
                ..
            }) if *resized == id => Some(current),
            _ => None,
        }
    }

    pub(in crate::draw) fn moving_offset(&self, id: ElementId) -> Option<Point> {
        let Some(Interaction::Moving {
            ids,
            start,
            current,
        }) = &self.interaction
        else {
            return None;
        };
        ids.binary_search(&id).is_ok().then_some(*current - *start)
    }
}
