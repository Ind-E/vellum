use super::{Editor, HistoryEntry, Interaction};
use crate::config::SizeRange;
use crate::draw::scene::{ElementKind, Style, tool_for};
use crate::tool::Tool;

#[derive(Default)]
pub(in crate::draw) struct Adjustment {
    pub(in crate::draw) changed: bool,
    pub(in crate::draw) feedback: Option<String>,
    pub(in crate::draw) hit_stop: bool,
}

const MIN_OPACITY: f32 = 0.05;

fn size_label(value: f32, default: f32) -> String {
    let suffix = if value == default { " · default" } else { "" };
    format!("{value} px{suffix}")
}

fn percent_label(value: f32, default: f32) -> String {
    let suffix = if value == default { " · default" } else { "" };
    format!("{:.0}%{suffix}", value * 100.0)
}

fn fill_label(filled: bool) -> String {
    format!("Fill · {}", if filled { "solid" } else { "outline" })
}

fn background_label(background: bool) -> String {
    format!("Background · {}", if background { "on" } else { "off" })
}

fn adjust_percent(value: &mut f32, default: f32, steps: f32, min: f32) -> Adjustment {
    let previous = *value;
    *value = stepped_value(*value, default, steps, 0.01, min, 1.0);
    Adjustment {
        changed: *value != previous,
        feedback: Some(percent_label(*value, default)),
        ..Default::default()
    }
}

fn stepped_value(value: f32, default: f32, steps: f32, increment: f32, min: f32, max: f32) -> f32 {
    let offset = (value - default) / increment;
    let aligned = if steps.is_sign_positive() {
        (offset + 1e-4).floor()
    } else {
        (offset - 1e-4).ceil()
    };
    (default + (aligned + steps) * increment).clamp(min, max)
}

struct SizeAdjustment {
    value: f32,
    hit_stop: bool,
}

fn stepped_size(value: f32, default: f32, steps: f32, range: &SizeRange) -> SizeAdjustment {
    let at_stop = value == default || range.stops().contains(&value);
    let target = if at_stop {
        range.clamp(value + steps * range.step())
    } else {
        stepped_value(
            value,
            default,
            steps,
            range.step(),
            range.min(),
            range.max(),
        )
    };
    let stops = std::iter::once(default).chain(range.stops().iter().copied());
    let stop = if target > value {
        stops
            .filter(|stop| *stop > value && *stop <= target)
            .min_by(f32::total_cmp)
    } else {
        stops
            .filter(|stop| *stop < value && *stop >= target)
            .max_by(f32::total_cmp)
    };
    SizeAdjustment {
        value: stop.unwrap_or(target),
        hit_stop: stop.is_some(),
    }
}

#[derive(Clone, Copy)]
pub(super) struct ToolProperties {
    pub size: f32,
    pub opacity: f32,
    pub roundness: f32,
    pub filled: bool,
}

#[derive(Clone, Copy)]
pub(super) struct ToolPropertySet([ToolProperties; Tool::SIZED.len()]);

impl ToolPropertySet {
    pub(super) fn new(
        stroke_size: f32,
        default_opacity: f32,
        default_fill_shapes: bool,
        defaults: &crate::config::ToolDefaults,
        size_ranges: &std::collections::BTreeMap<Tool, SizeRange>,
    ) -> Self {
        let properties = |tool: Tool| {
            let configured = defaults.get(&tool).cloned().unwrap_or_default();
            let size_range = size_ranges
                .get(&tool)
                .expect("adjustable tools have size ranges");
            let size = configured.size.unwrap_or_else(|| {
                tool.initial_size(stroke_size)
                    .expect("adjustable tools have sizes")
            });
            ToolProperties {
                size: size_range.clamp(size),
                opacity: configured.opacity.unwrap_or(default_opacity),
                roundness: configured
                    .roundness
                    .unwrap_or_else(|| tool.initial_roundness()),
                filled: if tool == Tool::Text {
                    configured.background.unwrap_or(false)
                } else {
                    configured
                        .filled
                        .unwrap_or(default_fill_shapes && tool.supports_fill())
                },
            }
        };
        Self(Tool::SIZED.map(properties))
    }

    pub(super) fn properties(&self, tool: Tool) -> Option<&ToolProperties> {
        self.0
            .get(Tool::SIZED.iter().position(|item| *item == tool)?)
    }

    fn properties_mut(&mut self, tool: Tool) -> Option<&mut ToolProperties> {
        self.0
            .get_mut(Tool::SIZED.iter().position(|item| *item == tool)?)
    }
}

impl Editor {
    pub(super) fn toggle_fill(&mut self) -> Adjustment {
        if let Some(edit) = self.text_edit_mut() {
            edit.style.filled = !edit.style.filled;
            return Adjustment {
                changed: true,
                feedback: Some(background_label(edit.style.filled)),
                ..Default::default()
            };
        }
        if !self.selected.is_empty() {
            let enable = if let Some(Interaction::Resizing { current, .. }) = &self.interaction {
                !current.style.filled
            } else {
                self.selected
                    .iter()
                    .filter_map(|id| self.element(*id))
                    .filter(|element| supports_fill(&element.kind))
                    .any(|element| !element.style.filled)
            };
            return self.adjust_selected(|kind, style| {
                supports_fill(kind).then(|| {
                    style.filled = enable;
                    if matches!(kind, ElementKind::Text { .. }) {
                        background_label(enable)
                    } else {
                        fill_label(enable)
                    }
                })
            });
        }
        if self.tool == Tool::Text {
            let properties = self
                .properties_mut(Tool::Text)
                .expect("text has adjustable properties");
            properties.filled = !properties.filled;
            let background = properties.filled;
            self.sync_active_style();
            return Adjustment {
                changed: true,
                feedback: Some(background_label(background)),
                ..Default::default()
            };
        }
        if !self.tool.supports_fill() {
            return Adjustment::default();
        }
        let properties = self
            .properties_mut(self.tool)
            .expect("fillable tools have adjustable properties");
        let filled = !properties.filled;
        properties.filled = filled;
        self.sync_active_style();
        Adjustment {
            changed: true,
            feedback: Some(fill_label(filled)),
            ..Default::default()
        }
    }

    pub(super) fn tool_fill(&self, tool: Tool) -> bool {
        self.properties(tool)
            .is_some_and(|properties| properties.filled)
    }

    pub(in crate::draw) fn adjust_size(&mut self, steps: f32) -> Adjustment {
        if steps == 0.0 {
            return Adjustment::default();
        }
        let default_text_size = self
            .default_size(Tool::Text)
            .expect("text must have an adjustable size");
        let text_size_range = self
            .size_ranges
            .get(&Tool::Text)
            .expect("text must have a size range")
            .clone();
        if let Some(edit) = self.text_edit_mut() {
            let adjustment =
                stepped_size(edit.style.size, default_text_size, steps, &text_size_range);
            let label = size_label(adjustment.value, default_text_size);
            let changed = edit.style.size != adjustment.value;
            if changed {
                edit.set_size(adjustment.value);
            }
            return Adjustment {
                changed,
                feedback: Some(label),
                hit_stop: adjustment.hit_stop,
            };
        }
        if !self.selected.is_empty() {
            let defaults = self.default_tool_properties;
            let size_ranges = self.size_ranges.clone();
            let mut hit_stop = false;
            let mut adjustment = self.adjust_selected(|kind, style| {
                let tool = tool_for(kind);
                let default = defaults
                    .properties(tool)
                    .expect("element tools have adjustable properties")
                    .size;
                let size_range = size_ranges
                    .get(&tool)
                    .expect("element tools have size ranges");
                let adjustment = stepped_size(style.size, default, steps, size_range);
                style.size = adjustment.value;
                hit_stop |= adjustment.hit_stop;
                Some(size_label(style.size, default))
            });
            adjustment.hit_stop = hit_stop;
            return adjustment;
        }
        let tool = self.tool;
        let Some(default) = self.default_size(tool) else {
            return Adjustment::default();
        };
        let size_range = self
            .size_ranges
            .get(&tool)
            .expect("adjustable tools have size ranges")
            .clone();
        let properties = self
            .properties_mut(tool)
            .expect("tools with a default size have adjustable properties");
        let adjustment = stepped_size(properties.size, default, steps, &size_range);
        let label = size_label(adjustment.value, default);
        let changed = adjustment.value != properties.size;
        if changed {
            properties.size = adjustment.value;
            self.sync_active_style();
        }
        Adjustment {
            changed,
            feedback: Some(label),
            hit_stop: adjustment.hit_stop,
        }
    }

    pub(in crate::draw) fn adjust_opacity(&mut self, steps: f32) -> Adjustment {
        if steps == 0.0 {
            return Adjustment::default();
        }
        let default_text_opacity = self.default_properties(Tool::Text).opacity;
        if let Some(edit) = self.text_edit_mut() {
            return adjust_percent(
                &mut edit.style.color[3],
                default_text_opacity,
                steps,
                MIN_OPACITY,
            );
        }
        if self.selected.is_empty() {
            if matches!(self.tool, Tool::Eraser | Tool::Select) {
                return Adjustment::default();
            }
            let tool = self.tool;
            let default = self.default_properties(tool).opacity;
            let Some(properties) = self.properties_mut(tool) else {
                return Adjustment::default();
            };
            let adjustment = adjust_percent(&mut properties.opacity, default, steps, MIN_OPACITY);
            if adjustment.changed {
                self.sync_active_style();
            }
            return adjustment;
        }
        let defaults = self.default_tool_properties;
        self.adjust_selected(|kind, style| {
            let default = defaults
                .properties(tool_for(kind))
                .expect("element tools have adjustable properties")
                .opacity;
            style.color[3] = stepped_value(style.color[3], default, steps, 0.01, MIN_OPACITY, 1.0);
            Some(percent_label(style.color[3], default))
        })
    }

    pub(in crate::draw) fn adjust_roundness(&mut self, steps: f32) -> Adjustment {
        if steps == 0.0 {
            return Adjustment::default();
        }
        let default_text_roundness = self.default_properties(Tool::Text).roundness;
        if let Some(edit) = self.text_edit_mut() {
            return adjust_percent(
                &mut edit.style.roundness,
                default_text_roundness,
                steps,
                0.0,
            );
        }
        if self.selected.is_empty() {
            let tool = self.tool;
            if tool.default_roundness().is_none() {
                return Adjustment::default();
            }
            let default = self.default_properties(tool).roundness;
            let properties = self
                .properties_mut(tool)
                .expect("tools with roundness have adjustable properties");
            let adjustment = adjust_percent(&mut properties.roundness, default, steps, 0.0);
            if adjustment.changed {
                self.sync_active_style();
            }
            return adjustment;
        }
        let defaults = self.default_tool_properties;
        self.adjust_selected(|kind, style| {
            let tool = tool_for(kind);
            tool.default_roundness()?;
            let default = defaults
                .properties(tool)
                .expect("element tools have adjustable properties")
                .roundness;
            style.roundness = stepped_value(style.roundness, default, steps, 0.01, 0.0, 1.0);
            Some(percent_label(style.roundness, default))
        })
    }

    fn adjust_selected(
        &mut self,
        mut adjust: impl FnMut(&ElementKind, &mut Style) -> Option<String>,
    ) -> Adjustment {
        if let Some(Interaction::Resizing {
            properties,
            current,
            ..
        }) = &mut self.interaction
        {
            let mut style = current.style;
            let feedback = adjust(&current.kind, &mut style);
            let changed = style != current.style;
            if changed {
                // Text resizing also changes font size; retain only the user's adjustment.
                *properties = Style {
                    size: properties.size + (style.size - current.style.size),
                    ..style
                };
                current.set_style(style);
            }
            return Adjustment {
                changed,
                feedback,
                ..Default::default()
            };
        }
        let ids = self.selected.clone();
        let mut updates = Vec::with_capacity(ids.len());
        let mut feedback = None;
        for id in ids {
            let Some(element) = self.element_mut(id) else {
                continue;
            };
            let mut style = element.style;
            if let Some(label) = adjust(&element.kind, &mut style) {
                feedback = Some(label);
            }
            if style != element.style {
                let (kind, style) = element.replace(element.kind.clone(), style);
                updates.push((id, kind, style));
            }
        }
        let changed = !updates.is_empty();
        if changed {
            self.history.record(HistoryEntry::Update(updates));
        }
        Adjustment {
            changed,
            feedback,
            ..Default::default()
        }
    }

    pub(in crate::draw) fn apply_rgba(&mut self, rgba: [f32; 4]) -> bool {
        let mut changed = self.style.color != rgba;
        if let Some(properties) = self.properties_mut(self.color_tool()) {
            changed |= properties.opacity != rgba[3];
            properties.opacity = rgba[3];
        }
        self.style.color = rgba;
        self.update_live_stroke_style();
        if let Some(edit) = self.text_edit_mut() {
            changed |= edit.style.color != rgba;
            edit.style.color = rgba;
            return changed;
        }
        if self.selected.is_empty() {
            return changed;
        }
        changed |= self
            .adjust_selected(|_, style| {
                style.color = rgba;
                None
            })
            .changed;
        changed
    }

    pub(super) fn properties(&self, tool: Tool) -> Option<&ToolProperties> {
        self.tool_properties.properties(tool)
    }

    fn properties_mut(&mut self, tool: Tool) -> Option<&mut ToolProperties> {
        self.tool_properties.properties_mut(tool)
    }

    fn default_size(&self, tool: Tool) -> Option<f32> {
        self.default_tool_properties
            .properties(tool)
            .map(|properties| properties.size)
    }

    fn default_properties(&self, tool: Tool) -> &ToolProperties {
        self.default_tool_properties
            .properties(tool)
            .expect("tools with adjustable properties have defaults")
    }

    pub(super) fn style_for(&self, tool: Tool) -> Style {
        let Some(properties) = self.properties(tool) else {
            return self.style;
        };
        let mut style = self.style;
        style.size = properties.size;
        style.color[3] = properties.opacity;
        style.roundness = properties.roundness;
        style.filled = properties.filled;
        style
    }

    pub(super) fn sync_active_style(&mut self) {
        self.style = self.style_for(self.tool);
        self.update_live_stroke_style();
    }

    fn update_live_stroke_style(&mut self) {
        let style = self.style_for(Tool::Pen);
        if let Some(Interaction::Freehand(stroke)) = &mut self.interaction {
            stroke.update_style(style);
        }
    }
}

fn supports_fill(kind: &ElementKind) -> bool {
    tool_for(kind).supports_fill() || matches!(kind, ElementKind::Text { .. })
}
