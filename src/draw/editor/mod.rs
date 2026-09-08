mod elements;
mod interaction;
mod preview;
mod properties;

use self::interaction::Interaction;
use self::properties::ToolPropertySet;
use super::Modifiers;
use super::history::{Entry as HistoryEntry, History};
use super::picker::{Choice, Picker, choice};
use super::scene::Element;
use super::scene::{ElementId, ElementKind, Point, Style};
use super::selection;
use super::text_edit::TextEdit;
use super::text_edit::{CursorMove, TextInputBatch};
use crate::tool::Tool;

pub(crate) enum Action {
    Undo,
    Redo,
    SelectAll,
    ToggleEraser,
    ToggleFill,
    Delete,
    Clear,
    Cancel,
    CommitText,
    Backspace,
    BackspaceWord,
    MoveCursor(CursorMove, bool),
    InsertText(String),
    ApplyTextInput(TextInputBatch),
}

#[derive(Default)]
pub(crate) struct EditorEffect {
    pub changed: bool,
    pub deactivate: bool,
    pub feedback: Option<String>,
}

pub(super) struct Editor {
    tool: Tool,
    style: Style,
    // IDs increase on insertion; undo restores the original order.
    elements: Vec<Element>,
    selected: Vec<ElementId>,
    interaction: Option<Interaction>,
    history: History,
    next_id: ElementId,
    next_text_session: u64,
    picker: Option<Picker>,
    default_tool: Tool,
    last_non_eraser_tool: Tool,
    tool_properties: ToolPropertySet,
    default_tool_properties: ToolPropertySet,
    size_ranges: std::sync::Arc<std::collections::BTreeMap<Tool, crate::config::SizeRange>>,
    remember_last_tool: bool,
    palette: Vec<[f32; 4]>,
}

impl Editor {
    pub(in crate::draw) fn new(settings: crate::config::Settings) -> Self {
        let default_tool_properties = ToolPropertySet::new(
            settings.stroke_size,
            settings.default_color[3],
            settings.default_fill_shapes,
            &settings.tool_defaults,
            &settings.size_ranges,
        );
        let tool_properties = default_tool_properties;
        let active = tool_properties.properties(settings.default_tool).copied();
        let fallback = tool_properties
            .properties(Tool::Pen)
            .copied()
            .expect("pen has adjustable properties");
        let active = active.unwrap_or(fallback);
        let mut color = settings.default_color;
        color[3] = active.opacity;
        Self {
            tool: settings.default_tool,
            style: Style {
                size: active.size,
                color,
                roundness: active.roundness,
                filled: active.filled,
            },
            elements: Vec::new(),
            selected: Vec::new(),
            interaction: None,
            history: History::default(),
            next_id: 1,
            next_text_session: 1,
            picker: None,
            default_tool: settings.default_tool,
            last_non_eraser_tool: if settings.default_tool == Tool::Eraser {
                Tool::Pen
            } else {
                settings.default_tool
            },
            tool_properties,
            default_tool_properties,
            size_ranges: settings.size_ranges,
            remember_last_tool: settings.remember_last_tool,
            palette: settings.palette,
        }
    }

    pub(in crate::draw) fn activate(&mut self) -> bool {
        if self.remember_last_tool || self.tool == self.default_tool {
            return false;
        }
        self.switch_tool(self.default_tool)
    }

    pub(in crate::draw) fn deactivate(&mut self) -> bool {
        let changed = self.finish_interaction();
        let clear_preview =
            !std::mem::take(&mut self.selected).is_empty() | self.picker.take().is_some();
        changed | clear_preview
    }

    pub(in crate::draw) fn is_editing_text(&self) -> bool {
        matches!(self.interaction, Some(Interaction::EditingText(_)))
    }

    pub(in crate::draw) fn is_drawing_pen(&self) -> bool {
        matches!(self.interaction, Some(Interaction::Freehand(_)))
    }

    pub(super) fn text_edit(&self) -> Option<&TextEdit> {
        match &self.interaction {
            Some(Interaction::EditingText(edit)) => Some(edit),
            _ => None,
        }
    }

    fn text_edit_mut(&mut self) -> Option<&mut TextEdit> {
        match &mut self.interaction {
            Some(Interaction::EditingText(edit)) => Some(edit),
            _ => None,
        }
    }

    pub(in crate::draw) fn current_color(&self) -> [f32; 4] {
        if let Some(edit) = self.text_edit() {
            edit.style.color
        } else if let Some(element) = self.selected.last().and_then(|id| self.element(*id)) {
            element.style.color
        } else {
            self.style.color
        }
    }

    pub(in crate::draw) fn picker_active(&self) -> bool {
        self.picker.is_some()
    }

    pub(in crate::draw) fn handle_action(&mut self, action: Action) -> EditorEffect {
        let mut effect = EditorEffect::default();
        if let Action::ApplyTextInput(batch) = action {
            let submit = batch.submit;
            if let Some(edit) = self.text_edit_mut() {
                effect.changed = edit.apply_text_input(batch);
                if submit {
                    effect.changed |= self.commit_text();
                }
            }
            return effect;
        }
        let closed_picker = self.picker.take().is_some();
        if closed_picker && matches!(action, Action::Cancel) {
            effect.changed = true;
            return effect;
        }
        match action {
            Action::Undo if !self.is_editing_text() => effect.changed = self.undo(),
            Action::Redo if !self.is_editing_text() => effect.changed = self.redo(),
            Action::SelectAll => {
                effect.changed = if let Some(edit) = self.text_edit_mut() {
                    edit.select_all()
                } else {
                    self.select_all()
                };
            }
            Action::ToggleEraser => effect.changed = self.toggle_eraser(),
            Action::ToggleFill => {
                let adjustment = self.toggle_fill();
                effect.changed = adjustment.changed;
                effect.feedback = adjustment.feedback;
            }
            Action::Delete => {
                if let Some(edit) = self.text_edit_mut() {
                    effect.changed = edit.delete();
                } else {
                    effect.changed = self.delete_selection();
                }
            }
            Action::Clear => effect.changed = self.clear(),
            Action::Cancel => {
                let cancelled = self.cancel_interaction();
                if cancelled || !std::mem::take(&mut self.selected).is_empty() {
                    effect.changed = true;
                } else {
                    effect.deactivate = true;
                }
            }
            Action::CommitText => effect.changed = self.commit_text(),
            Action::Backspace => {
                if let Some(edit) = self.text_edit_mut() {
                    effect.changed = edit.backspace();
                }
            }
            Action::BackspaceWord => {
                if let Some(edit) = self.text_edit_mut() {
                    effect.changed = edit.backspace_word();
                }
            }
            Action::MoveCursor(movement, extend) => {
                if let Some(edit) = self.text_edit_mut() {
                    effect.changed = edit.move_cursor(movement, extend);
                }
            }
            Action::InsertText(text) => {
                if let Some(edit) = self.text_edit_mut() {
                    effect.changed = edit.insert(&text);
                }
            }
            Action::Undo | Action::Redo | Action::ApplyTextInput(_) => {}
        }
        effect.changed |= closed_picker;
        effect
    }

    pub(in crate::draw) fn open_picker(&mut self, center: Point) {
        self.picker = Some(Picker {
            center,
            hovered: None,
        });
    }

    pub(in crate::draw) fn picker_motion(&mut self, point: Point) -> bool {
        let Some(picker) = &mut self.picker else {
            return false;
        };
        let choice = choice(picker.center, point, self.palette.len());
        let changed = picker.hovered != choice;
        picker.hovered = choice;
        changed
    }

    pub(in crate::draw) fn picker_release(&mut self, point: Point, latch_center: bool) -> bool {
        let Some(picker) = self.picker else {
            return false;
        };
        let choice = choice(picker.center, point, self.palette.len());
        if choice.is_none() && latch_center {
            return false;
        }
        self.picker = None;
        match choice {
            Some(Choice::Color(index)) => {
                self.apply_rgba(self.palette[index]);
            }
            Some(Choice::Tool(tool)) => {
                self.switch_tool(tool);
            }
            None => {}
        }
        true
    }

    pub(in crate::draw) fn dismiss_picker(&mut self) -> bool {
        self.picker.take().is_some()
    }

    fn toggle_eraser(&mut self) -> bool {
        let tool = if self.tool == Tool::Eraser {
            self.last_non_eraser_tool
        } else {
            Tool::Eraser
        };
        self.switch_tool(tool)
    }

    fn color_tool(&self) -> Tool {
        if self.tool == Tool::Eraser {
            self.last_non_eraser_tool
        } else {
            self.tool
        }
    }

    pub(super) fn clear_preedit(&mut self) -> bool {
        self.text_edit_mut().is_some_and(TextEdit::clear_preedit)
    }

    pub(super) fn make_text_edit(
        &mut self,
        id: Option<ElementId>,
        origin: Point,
        content: String,
        style: Style,
        scale: [f32; 2],
    ) -> TextEdit {
        let session = self.next_text_session;
        self.next_text_session = self.next_text_session.wrapping_add(1).max(1);
        TextEdit::new(session, id, origin, content, style, scale)
    }

    fn switch_tool(&mut self, tool: Tool) -> bool {
        if self.tool == tool {
            return false;
        }
        self.finish_interaction();
        self.selected.clear();
        self.tool = tool;
        if tool != Tool::Eraser {
            self.last_non_eraser_tool = tool;
        }
        self.sync_active_style();
        true
    }
}

fn drawing_kind(tool: Tool, start: Point, current: Point, modifiers: Modifiers) -> ElementKind {
    match tool {
        Tool::Line | Tool::Arrow => ElementKind::Segment {
            points: [
                start,
                selection::constrained_endpoint(start, current, modifiers.shift),
            ],
            arrow: tool == Tool::Arrow,
        },
        Tool::Triangle => ElementKind::Triangle {
            vertices: selection::triangle_from_drag(start, current, modifiers),
        },
        Tool::Rectangle => {
            let (min, max) =
                selection::constrained_box(start, current, modifiers.shift, modifiers.alt);
            ElementKind::Rectangle { min, max }
        }
        Tool::Ellipse => {
            let (min, max) =
                selection::constrained_box(start, current, modifiers.shift, modifiers.alt);
            ElementKind::Ellipse {
                center: min.midpoint(max),
                radii: Point::new((max.x - min.x) * 0.5, (max.y - min.y) * 0.5),
            }
        }
        Tool::Pen | Tool::Text | Tool::Eraser | Tool::Select => unreachable!(),
    }
}
