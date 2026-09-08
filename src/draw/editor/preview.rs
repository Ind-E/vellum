use super::{Editor, Interaction, drawing_kind};
use crate::draw::picker::{ShapeFills, picker_geometry};
use crate::draw::scene::{Element, ElementId, ElementKind, Point, Style, geometry};
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
        if let Some(id) = self.selected.first() {
            self.append_selection_geometry_for(
                *id,
                show_handles && self.interaction.is_none(),
                output,
            );
        }
    }

    fn append_selection_geometry_for(
        &self,
        id: ElementId,
        show_handles: bool,
        output: &mut Vec<Geometry>,
    ) {
        let Some(element) = self.element(id) else {
            return;
        };
        match &self.interaction {
            Some(Interaction::EditingText(edit)) if edit.id == Some(id) => {
                let bounds = edit.bounds();
                output.push(selection::outline(bounds.min, bounds.max));
                return;
            }
            Some(Interaction::Resizing {
                id: resizing_id,
                current,
                ..
            }) if *resizing_id == id => {
                if !matches!(
                    current.kind,
                    ElementKind::Segment { .. } | ElementKind::Triangle { .. }
                ) {
                    output.push(selection::outline(current.bounds.min, current.bounds.max));
                }
                return;
            }
            _ => {}
        }
        let offset = self.moving_offset(id).unwrap_or_default();
        let kind = &element.kind;
        if !matches!(
            kind,
            ElementKind::Segment { .. } | ElementKind::Triangle { .. }
        ) {
            let bounds = element.bounds;
            output.push(selection::outline(bounds.min + offset, bounds.max + offset));
        }
        if !show_handles {
            return;
        }
        selection::append_handles(kind, element.style, output);
    }

    pub(in crate::draw) fn picker_geometry(&self) -> Option<crate::render::LocalGeometry> {
        let picker = self.picker?;
        let active = self.color_tool();
        Some(picker_geometry(
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
        ))
    }

    pub(in crate::draw) fn element_geometry_preview(&self, element: &Element) -> Option<Geometry> {
        if let Some(delta) = self.moving_offset(element.id) {
            return Some(element.geometry.translated([delta.x, delta.y]));
        }
        match &self.interaction {
            Some(Interaction::Resizing {
                id: resized,
                current,
                ..
            }) if *resized == element.id => Some(geometry(&current.kind, current.style)),
            _ => None,
        }
    }

    pub(in crate::draw) fn element_bounds_preview(
        &self,
        element: &Element,
    ) -> crate::draw::scene::Bounds {
        if let Some(Interaction::Resizing { id, current, .. }) = &self.interaction
            && *id == element.id
        {
            return current.bounds;
        }
        let offset = self.moving_offset(element.id).unwrap_or_default();
        crate::draw::scene::Bounds {
            min: element.bounds.min + offset,
            max: element.bounds.max + offset,
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

    pub(in crate::draw) fn text_resize_preview(
        &self,
        id: ElementId,
    ) -> Option<(&ElementKind, Style)> {
        let Some(Interaction::Resizing {
            id: resized,
            current,
            ..
        }) = &self.interaction
        else {
            return None;
        };
        (*resized == id && matches!(current.kind, ElementKind::Text { .. }))
            .then_some((&current.kind, current.style))
    }
}
