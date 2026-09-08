use std::collections::BTreeMap;
use std::sync::Arc;

use super::ToolDefaults;
use crate::tool::Tool;

const DEFAULT_TEXT_MIN_SIZE: f32 = 8.0;
const DEFAULT_TEXT_MAX_SIZE: f32 = 500.0;
const DEFAULT_TEXT_STEP: f32 = 0.5;

#[derive(Clone, Debug)]
pub(crate) struct SizeRange {
    min: f32,
    max: f32,
    step: f32,
    stops: Arc<[f32]>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SizeRangeConfig {
    min: Option<f32>,
    max: Option<f32>,
    step: Option<f32>,
    stops: Option<Vec<f32>>,
}

impl SizeRangeConfig {
    pub(super) fn resolve(&self, fallback: &SizeRange) -> SizeRange {
        let stops = self.stops.as_ref().map_or_else(
            || fallback.stops.clone(),
            |stops| {
                let mut stops = stops.clone();
                stops.sort_by(f32::total_cmp);
                stops.dedup();
                Arc::from(stops)
            },
        );
        SizeRange {
            min: self.min.unwrap_or(fallback.min),
            max: self.max.unwrap_or(fallback.max),
            step: self.step.unwrap_or(fallback.step),
            stops,
        }
    }
}

impl Default for SizeRange {
    fn default() -> Self {
        Self {
            min: 1.0,
            max: 100.0,
            step: 1.0,
            stops: Arc::from([]),
        }
    }
}

impl SizeRange {
    pub(super) fn validate(self, name: &str) -> Result<Self, String> {
        if !self.min.is_finite() || self.min <= 0.0 {
            return Err(format!("{name}.min must be greater than 0"));
        }
        if !self.max.is_finite() || self.max < self.min {
            return Err(format!(
                "{name}.max must be greater than or equal to {name}.min"
            ));
        }
        if !self.step.is_finite() || self.step <= 0.0 {
            return Err(format!("{name}.step must be greater than 0"));
        }
        if self.stops.iter().any(|stop| !self.contains(*stop)) {
            return Err(format!(
                "{name}.stops values must be between {} and {}",
                self.min, self.max,
            ));
        }
        Ok(self)
    }

    pub(crate) fn contains(&self, size: f32) -> bool {
        size.is_finite() && (self.min..=self.max).contains(&size)
    }

    pub(crate) fn clamp(&self, size: f32) -> f32 {
        size.clamp(self.min, self.max)
    }

    pub(crate) fn min(&self) -> f32 {
        self.min
    }

    pub(crate) fn max(&self) -> f32 {
        self.max
    }

    pub(crate) fn step(&self) -> f32 {
        self.step
    }

    pub(crate) fn stops(&self) -> &[f32] {
        &self.stops
    }
}

pub(super) fn resolve_size_ranges(
    tools: &ToolDefaults,
    fallback: &SizeRange,
    global: &SizeRangeConfig,
) -> Result<BTreeMap<Tool, SizeRange>, String> {
    Tool::SIZED
        .into_iter()
        .map(|tool| {
            let name = format!("tools.{}.size_range", tool.name());
            let mut fallback = fallback.clone();
            if tool == Tool::Text {
                if global.min.is_none() {
                    fallback.min = DEFAULT_TEXT_MIN_SIZE;
                }
                if global.max.is_none() {
                    fallback.max = DEFAULT_TEXT_MAX_SIZE;
                }
                if global.step.is_none() {
                    fallback.step = DEFAULT_TEXT_STEP;
                }
                let stops = fallback
                    .stops
                    .iter()
                    .copied()
                    .filter(|stop| fallback.contains(*stop))
                    .collect::<Vec<_>>();
                fallback.stops = Arc::from(stops);
            }
            let range = tools
                .get(&tool)
                .and_then(|defaults| defaults.size_range.as_ref())
                .map_or_else(|| fallback.clone(), |range| range.resolve(&fallback))
                .validate(&name)?;
            Ok((tool, range))
        })
        .collect()
}
