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

fn adjust_percent(
    value: &mut f32,
    default: f32,
    steps: f32,
    increment: f32,
    min: f32,
    stops: &[f32],
) -> Adjustment {
    let previous = *value;
    let adjustment = stepped_with_stops(previous, default, steps, increment, min, 1.0, stops);
    *value = adjustment.value;
    Adjustment {
        changed: *value != previous,
        feedback: Some(percent_label(*value, default)),
        hit_stop: adjustment.hit_stop,
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

struct StepAdjustment {
    value: f32,
    hit_stop: bool,
}

fn stepped_with_stops(
    value: f32,
    default: f32,
    steps: f32,
    increment: f32,
    min: f32,
    max: f32,
    stops: &[f32],
) -> StepAdjustment {
    let at_stop = value == default || stops.contains(&value);
    let target = if at_stop {
        (value + steps * increment).clamp(min, max)
    } else {
        stepped_value(value, default, steps, increment, min, max)
    };
    let tolerance = increment * 1e-4;
    let stops = std::iter::once(default).chain(stops.iter().copied());
    let stop = if target > value {
        stops
            .filter(|stop| *stop > value && *stop <= target + tolerance)
            .min_by(f32::total_cmp)
    } else if target < value {
        stops
            .filter(|stop| *stop < value && *stop >= target - tolerance)
            .max_by(f32::total_cmp)
    } else {
        None
    };
    StepAdjustment {
        value: stop.unwrap_or(target),
        hit_stop: stop.is_some(),
    }
}

fn stepped_size(
    value: f32,
    default: f32,
    steps: f32,
    increment: f32,
    range: &SizeRange,
    stops: &[f32],
) -> StepAdjustment {
    stepped_with_stops(
        value,
        default,
        steps,
        increment,
        range.min(),
        range.max(),
        stops,
    )
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
        let fallback = crate::config::PropertyDefaults::default();
        let properties = |tool: Tool| {
            let configured = defaults.get(&tool).unwrap_or(&fallback);
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
        if !self.tool.supports_fill() && self.tool != Tool::Text {
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
            feedback: Some(if self.tool == Tool::Text {
                background_label(filled)
            } else {
                fill_label(filled)
            }),
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
            .copied()
            .expect("text must have a size range");
        let text_step = self
            .steps
            .get(&Tool::Text)
            .expect("text has size step")
            .size;
        let text_stops = self
            .stops
            .get(&Tool::Text)
            .expect("text has size stops")
            .size
            .clone();
        if let Some(edit) = self.text_edit_mut() {
            let adjustment = stepped_size(
                edit.style.size,
                default_text_size,
                steps,
                text_step,
                &text_size_range,
                &text_stops,
            );
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
            let step_sizes = self.steps.clone();
            let stops = self.stops.clone();
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
                let stop_values = &stops
                    .get(&tool)
                    .expect("element tools have size stops")
                    .size;
                let increment = step_sizes
                    .get(&tool)
                    .expect("element tools have size step")
                    .size;
                let adjustment = stepped_size(
                    style.size,
                    default,
                    steps,
                    increment,
                    size_range,
                    stop_values,
                );
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
            .copied()
            .expect("adjustable tools have size ranges");
        let increment = self
            .steps
            .get(&tool)
            .expect("adjustable tools have size step")
            .size;
        let stops = self
            .stops
            .get(&tool)
            .expect("adjustable tools have size stops")
            .size
            .clone();
        let properties = self
            .properties_mut(tool)
            .expect("tools with a default size have adjustable properties");
        let adjustment = stepped_size(
            properties.size,
            default,
            steps,
            increment,
            &size_range,
            &stops,
        );
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
        let text_step = self
            .steps
            .get(&Tool::Text)
            .expect("text has opacity step")
            .opacity;
        let text_stops = self
            .stops
            .get(&Tool::Text)
            .expect("text has opacity stops")
            .opacity
            .clone();
        if let Some(edit) = self.text_edit_mut() {
            return adjust_percent(
                &mut edit.style.color[3],
                default_text_opacity,
                steps,
                text_step,
                MIN_OPACITY,
                &text_stops,
            );
        }
        if self.selected.is_empty() {
            if matches!(self.tool, Tool::Eraser | Tool::Select) {
                return Adjustment::default();
            }
            let tool = self.tool;
            let default = self.default_properties(tool).opacity;
            let increment = self
                .steps
                .get(&tool)
                .expect("tool has opacity step")
                .opacity;
            let stops = self
                .stops
                .get(&tool)
                .expect("tool has opacity stops")
                .opacity
                .clone();
            let Some(properties) = self.properties_mut(tool) else {
                return Adjustment::default();
            };
            let adjustment = adjust_percent(
                &mut properties.opacity,
                default,
                steps,
                increment,
                MIN_OPACITY,
                &stops,
            );
            if adjustment.changed {
                self.sync_active_style();
            }
            return adjustment;
        }
        let defaults = self.default_tool_properties;
        let step_sizes = self.steps.clone();
        let stops = self.stops.clone();
        let mut hit_stop = false;
        let mut adjustment = self.adjust_selected(|kind, style| {
            let tool = tool_for(kind);
            let default = defaults
                .properties(tool)
                .expect("element tools have adjustable properties")
                .opacity;
            let stop_values = &stops.get(&tool).expect("tool has opacity stops").opacity;
            let increment = step_sizes
                .get(&tool)
                .expect("tool has opacity step")
                .opacity;
            let adjustment = adjust_percent(
                &mut style.color[3],
                default,
                steps,
                increment,
                MIN_OPACITY,
                stop_values,
            );
            hit_stop |= adjustment.hit_stop;
            adjustment.feedback
        });
        adjustment.hit_stop = hit_stop;
        adjustment
    }

    pub(in crate::draw) fn adjust_roundness(&mut self, steps: f32) -> Adjustment {
        if steps == 0.0 {
            return Adjustment::default();
        }
        let default_text_roundness = self.default_properties(Tool::Text).roundness;
        let text_step = self
            .steps
            .get(&Tool::Text)
            .expect("text has roundness step")
            .roundness;
        let text_stops = self
            .stops
            .get(&Tool::Text)
            .expect("text has roundness stops")
            .roundness
            .clone();
        if let Some(edit) = self.text_edit_mut() {
            return adjust_percent(
                &mut edit.style.roundness,
                default_text_roundness,
                steps,
                text_step,
                0.0,
                &text_stops,
            );
        }
        if self.selected.is_empty() {
            let tool = self.tool;
            if tool.default_roundness().is_none() {
                return Adjustment::default();
            }
            let default = self.default_properties(tool).roundness;
            let increment = self
                .steps
                .get(&tool)
                .expect("tool has roundness step")
                .roundness;
            let stops = self
                .stops
                .get(&tool)
                .expect("tool has roundness stops")
                .roundness
                .clone();
            let properties = self
                .properties_mut(tool)
                .expect("tools with roundness have adjustable properties");
            let adjustment = adjust_percent(
                &mut properties.roundness,
                default,
                steps,
                increment,
                0.0,
                &stops,
            );
            if adjustment.changed {
                self.sync_active_style();
            }
            return adjustment;
        }
        let defaults = self.default_tool_properties;
        let step_sizes = self.steps.clone();
        let stops = self.stops.clone();
        let mut hit_stop = false;
        let mut adjustment = self.adjust_selected(|kind, style| {
            let tool = tool_for(kind);
            tool.default_roundness()?;
            let default = defaults
                .properties(tool)
                .expect("element tools have adjustable properties")
                .roundness;
            let stop_values = &stops
                .get(&tool)
                .expect("tool has roundness stops")
                .roundness;
            let increment = step_sizes
                .get(&tool)
                .expect("tool has roundness step")
                .roundness;
            let adjustment = adjust_percent(
                &mut style.roundness,
                default,
                steps,
                increment,
                0.0,
                stop_values,
            );
            hit_stop |= adjustment.hit_stop;
            adjustment.feedback
        });
        adjustment.hit_stop = hit_stop;
        adjustment
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
            self.document.history.record(HistoryEntry::Update(updates));
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
