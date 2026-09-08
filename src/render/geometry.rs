use kurbo::{Affine, BezPath, ParamCurve, ParamCurveNearest, Shape, Stroke};
use peniko::Fill;

#[derive(Debug, Clone)]
pub(super) enum DrawCommand {
    Fill {
        path: BezPath,
        fill_rule: Fill,
        color: [f32; 4],
    },
    Stroke {
        path: BezPath,
        stroke: Stroke,
        color: [f32; 4],
    },
}

#[derive(Debug, Clone, Default)]
pub struct Geometry {
    pub(super) commands: Vec<DrawCommand>,
}

pub struct LocalGeometry {
    pub(super) geometry: Geometry,
    pub(super) origin: [f32; 2],
    pub(super) size: [u32; 2],
}

impl LocalGeometry {
    pub fn new(geometry: Geometry, origin: [f32; 2], size: [u32; 2]) -> Self {
        Self {
            geometry,
            origin,
            size,
        }
    }
}

impl Geometry {
    pub fn bounds(&self) -> Option<kurbo::Rect> {
        self.commands
            .iter()
            .map(|command| match command {
                DrawCommand::Fill { path, .. } => path.control_box(),
                DrawCommand::Stroke { path, stroke, .. } => {
                    let expansion = stroke.width * 0.5 * stroke.miter_limit.max(1.0);
                    path.control_box().inflate(expansion, expansion)
                }
            })
            .reduce(|first, second| first.union(second))
    }

    pub fn fill(path: BezPath, fill_rule: Fill, color: [f32; 4]) -> Self {
        Self {
            commands: vec![DrawCommand::Fill {
                path,
                fill_rule,
                color,
            }],
        }
    }

    pub fn stroke(path: BezPath, stroke: Stroke, color: [f32; 4]) -> Self {
        Self {
            commands: vec![DrawCommand::Stroke {
                path,
                stroke,
                color,
            }],
        }
    }

    pub fn push_fill(&mut self, path: BezPath, fill_rule: Fill, color: [f32; 4]) {
        self.commands.push(DrawCommand::Fill {
            path,
            fill_rule,
            color,
        });
    }

    pub fn push_stroke(&mut self, path: BezPath, stroke: Stroke, color: [f32; 4]) {
        self.commands.push(DrawCommand::Stroke {
            path,
            stroke,
            color,
        });
    }

    pub fn append(&mut self, other: Self) {
        self.commands.extend(other.commands);
    }

    pub fn fill_hit_test(&self, point: kurbo::Point, slop: f64) -> bool {
        let slop = slop.max(0.0);
        let slop_squared = slop.powi(2);
        self.commands.iter().any(|command| {
            let DrawCommand::Fill {
                path, fill_rule, ..
            } = command
            else {
                return false;
            };
            // An edge hit avoids scanning the entire outline for its winding.
            if slop_squared > 0.0
                && path.segments().any(|segment| {
                    let bounds = segment.bounding_box().inflate(slop, slop);
                    point.x >= bounds.x0
                        && point.x <= bounds.x1
                        && point.y >= bounds.y0
                        && point.y <= bounds.y1
                        && segment.nearest(point, 0.1).distance_sq <= slop_squared
                })
            {
                return true;
            }
            let winding = path.winding(point);
            match fill_rule {
                Fill::NonZero => winding != 0,
                Fill::EvenOdd => winding % 2 != 0,
            }
        })
    }

    pub fn swept_hit_test(&self, line: kurbo::Line, radius: f64) -> bool {
        let delta = line.p1 - line.p0;
        let length = delta.hypot();
        let to_local =
            Affine::rotate(-delta.y.atan2(delta.x)) * Affine::translate(-line.p0.to_vec2());
        self.commands.iter().any(|command| {
            let (path, tolerance, fill_rule) = match command {
                DrawCommand::Fill {
                    path, fill_rule, ..
                } => (path, radius, Some(fill_rule)),
                DrawCommand::Stroke { path, stroke, .. } => {
                    (path, radius + stroke.width * 0.5, None)
                }
            };
            let bounds = line.bounding_box().inflate(tolerance, tolerance);
            let hit_edge = path.segments().any(|segment| {
                let other = segment.bounding_box();
                bounds.x0 <= other.x1
                    && bounds.x1 >= other.x0
                    && bounds.y0 <= other.y1
                    && bounds.y1 >= other.y0
                    && if length == 0.0 {
                        segment.nearest(line.p0, 0.1).distance_sq <= tolerance * tolerance
                    } else {
                        let local = to_local * segment;
                        // A closest pair lies at an endpoint, a perpendicular extremum,
                        // or an intersection. Curve parameter tolerances are not pixels.
                        !segment.intersect_line(line).is_empty()
                            || kurbo::ParamCurveExtrema::extrema(&local)
                                .into_iter()
                                .chain([0.0, 1.0])
                                .any(|t| {
                                    let point = local.eval(t);
                                    point.y.powi(2) + (point.x - point.x.clamp(0.0, length)).powi(2)
                                        <= tolerance * tolerance
                                })
                            || [kurbo::Point::ZERO, kurbo::Point::new(length, 0.0)]
                                .into_iter()
                                .any(|point| {
                                    local.nearest(point, 0.1).distance_sq <= tolerance * tolerance
                                })
                    }
            });
            hit_edge
                || fill_rule.is_some_and(|rule| {
                    let winding = path.winding(line.p0);
                    match rule {
                        Fill::NonZero => winding != 0,
                        Fill::EvenOdd => winding % 2 != 0,
                    }
                })
        })
    }

    pub fn translated(&self, offset: [f32; 2]) -> Self {
        let transform = Affine::translate((f64::from(offset[0]), f64::from(offset[1])));
        Self {
            commands: self
                .commands
                .iter()
                .map(|command| match command {
                    DrawCommand::Fill {
                        path,
                        fill_rule,
                        color,
                    } => DrawCommand::Fill {
                        path: transform * path,
                        fill_rule: *fill_rule,
                        color: *color,
                    },
                    DrawCommand::Stroke {
                        path,
                        stroke,
                        color,
                    } => DrawCommand::Stroke {
                        path: transform * path,
                        stroke: stroke.clone(),
                        color: *color,
                    },
                })
                .collect(),
        }
    }
}
