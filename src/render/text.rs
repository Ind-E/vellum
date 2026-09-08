use std::collections::HashMap;

use crate::text::{layout_text, text_bounds};
use parley::{Layout, PositionedLayoutItem};
use skrifa::MetadataProvider;
use skrifa::bitmap::{BitmapData, BitmapFormat, BitmapGlyph, Origin};
use skrifa::raw::TableProvider;
use vello_common::paint::{Image, ImageId, ImageSource};

use super::{srgb_to_linear, vello_color};

const DARK_TEXT_EMBOLDENING: f64 = 0.2;
const BLACK_BACKGROUND_COLOR: f32 = 20.0 / 255.0;
const WHITE_BACKGROUND_COLOR: f32 = 235.0 / 255.0;

fn background_color([red, green, blue, alpha]: [f32; 4]) -> [f32; 4] {
    let luminance = 0.2126 * srgb_to_linear(red)
        + 0.7152 * srgb_to_linear(green)
        + 0.0722 * srgb_to_linear(blue);
    let contrast = |background| {
        let background_luminance = srgb_to_linear(background);
        (luminance.max(background_luminance) + 0.05) / (luminance.min(background_luminance) + 0.05)
    };
    let background = if contrast(BLACK_BACKGROUND_COLOR) >= contrast(WHITE_BACKGROUND_COLOR) {
        BLACK_BACKGROUND_COLOR
    } else {
        WHITE_BACKGROUND_COLOR
    };
    [background, background, background, alpha]
}

pub struct TextSpec<'a> {
    pub key: u64,
    pub content: &'a str,
    pub left: f32,
    pub top: f32,
    pub font_size: f32,
    pub color: [f32; 4],
    pub background_roundness: Option<f32>,
    pub scale: [f32; 2],
}

pub(super) struct TextTarget<'a> {
    pub scene: &'a mut vello_hybrid::Scene,
    pub resources: &'a mut vello_hybrid::Resources,
    pub renderer: &'a mut vello_hybrid::Renderer,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub is_srgb: bool,
}

type BitmapKey = (u64, u32, u32, u32, u32);

struct BitmapImage {
    id: ImageId,
    size: [u16; 2],
    used: bool,
}

impl TextSpec<'_> {
    fn append_to_scene(
        &self,
        layout: &Layout<()>,
        target: &mut TextTarget<'_>,
        bitmaps: &mut HashMap<BitmapKey, BitmapImage>,
        outline_glyphs: &mut Vec<glifo::Glyph>,
    ) {
        let scene = &mut *target.scene;
        let target_is_srgb = target.is_srgb;
        scene.set_fill_rule(peniko::Fill::NonZero);
        let automatic_background = self
            .background_roundness
            .map(|roundness| (background_color(self.color), roundness));
        let emboldening = if automatic_background.is_some_and(|(color, _)| color[0] > 0.5) {
            DARK_TEXT_EMBOLDENING
        } else {
            0.0
        };
        if let Some((background_color, roundness)) = automatic_background {
            use kurbo::Shape;
            let [[min_x, min_y], [max_x, max_y]] = text_bounds(
                [self.left, self.top],
                [layout.full_width(), layout.height()],
                self.font_size,
                Some(roundness),
                self.scale,
            );
            let radius = (max_y - min_y) * 0.5 * roundness;
            let background = kurbo::RoundedRect::new(
                f64::from(min_x),
                f64::from(min_y),
                f64::from(max_x),
                f64::from(max_y),
                f64::from(radius),
            )
            .to_path(0.1);
            scene.set_paint(vello_color(background_color, target_is_srgb));
            scene.fill_path(&background);
        }
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let [scale_x, scale_y] = self.scale;
                let synthesis = run.synthesis();
                let skew = f64::from(synthesis.skew().unwrap_or(0.0))
                    .to_radians()
                    .tan();
                let transform =
                    kurbo::Affine::scale_non_uniform(f64::from(scale_x), f64::from(scale_y))
                        * kurbo::Affine::new([1.0, 0.0, -skew, 1.0, 0.0, 0.0]);
                let emboldening = emboldening
                    + if synthesis.embolden() {
                        f64::from(run.font_size()) / 48.0
                    } else {
                        0.0
                    };
                let font =
                    skrifa::FontRef::from_index(run.font().data.as_ref(), run.font().index).ok();
                let strikes = font.as_ref().map(|font| font.bitmap_strikes());
                let units_per_em = font
                    .as_ref()
                    .and_then(|font| font.head().ok())
                    .map_or(1.0, |head| f32::from(head.units_per_em()));
                let is_sbix = strikes.as_ref().and_then(|strikes| strikes.format())
                    == Some(BitmapFormat::Sbix);
                let bitmap = |id| {
                    strikes
                        .as_ref()?
                        .glyph_for_size(
                            skrifa::instance::Size::new(run.font_size()),
                            skrifa::GlyphId::new(id),
                        )
                        .filter(|bitmap| matches!(bitmap.data, BitmapData::Png(_)))
                };
                // Hybrid's direct glyph path cannot consume bitmap pixmaps, and
                // its glyph cache excludes large sizes. Upload each native strike once.
                for glyph in glyph_run.positioned_glyphs() {
                    if let Some(mut bitmap) = bitmap(glyph.id) {
                        // Match glifo's Apple Color Emoji offset, inherited from
                        // CoreText: a zero SBIX bearing gets 100 font units.
                        if bitmap.bearing_y == 0.0 && is_sbix {
                            bitmap.bearing_y = 100.0;
                        }
                        self.draw_bitmap(target, bitmaps, run, glyph, &bitmap, units_per_em);
                    } else {
                        outline_glyphs.push(glifo::Glyph {
                            id: glyph.id,
                            x: self.left + scale_x * glyph.x,
                            y: self.top + scale_y * glyph.y,
                        });
                    }
                }
                target
                    .scene
                    .set_paint(vello_color(self.color, target_is_srgb));
                target
                    .scene
                    .glyph_run(target.resources, run.font())
                    .font_size(run.font_size())
                    .normalized_coords(run.normalized_coords())
                    .glyph_transform(transform)
                    .hint(true)
                    .font_embolden(glifo::FontEmbolden::new(kurbo::Diagonal2::new(
                        emboldening,
                        emboldening,
                    )))
                    .fill_glyphs(outline_glyphs.iter().copied());
                outline_glyphs.clear();
            }
        }
    }

    fn draw_bitmap(
        &self,
        target: &mut TextTarget<'_>,
        cache: &mut HashMap<BitmapKey, BitmapImage>,
        run: &parley::Run<'_, ()>,
        glyph: parley::Glyph,
        bitmap: &BitmapGlyph<'_>,
        units_per_em: f32,
    ) {
        let BitmapData::Png(data) = bitmap.data else {
            return;
        };
        let key = (
            run.font().data.id(),
            run.font().index,
            glyph.id,
            bitmap.ppem_x.to_bits(),
            bitmap.ppem_y.to_bits(),
        );
        let image = match cache.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let Ok(mut pixmap) = vello_hybrid::Pixmap::from_png(std::io::Cursor::new(data))
                else {
                    return;
                };
                if target.is_srgb {
                    for pixel in pixmap.data_mut() {
                        let alpha = f32::from(pixel.a);
                        if alpha > 0.0 {
                            let convert = |channel| {
                                (srgb_to_linear(f32::from(channel) / alpha) * alpha).round() as u8
                            };
                            pixel.r = convert(pixel.r);
                            pixel.g = convert(pixel.g);
                            pixel.b = convert(pixel.b);
                        }
                    }
                }
                let size = [pixmap.width(), pixmap.height()];
                let id = target.renderer.upload_image(
                    target.resources,
                    target.device,
                    target.queue,
                    target.encoder,
                    &pixmap,
                );
                entry.insert(BitmapImage {
                    id,
                    size,
                    used: false,
                })
            }
        };
        image.used = true;
        let size = run.font_size();
        let origin_y = if bitmap.placement_origin == Origin::BottomLeft {
            -f64::from(image.size[1])
        } else {
            0.0
        };
        let saved = target.scene.save_current_state();
        let transform = saved.transforms.scene_transform()
            * kurbo::Affine::translate((
                f64::from(self.left + self.scale[0] * glyph.x),
                f64::from(self.top + self.scale[1] * glyph.y),
            ))
            * kurbo::Affine::scale_non_uniform(f64::from(self.scale[0]), f64::from(self.scale[1]))
            * kurbo::Affine::translate((
                f64::from(-bitmap.bearing_x * size / units_per_em),
                f64::from(bitmap.bearing_y * size / units_per_em),
            ))
            * kurbo::Affine::scale_non_uniform(
                f64::from(size / bitmap.ppem_x),
                f64::from(size / bitmap.ppem_y),
            )
            * kurbo::Affine::translate((
                f64::from(-bitmap.inner_bearing_x),
                f64::from(-bitmap.inner_bearing_y) + origin_y,
            ));
        target.scene.set_transform(transform);
        target.scene.reset_paint_transform();
        target.scene.set_paint(Image {
            image: ImageSource::opaque_id(image.id),
            sampler: Default::default(),
        });
        target.scene.push_opacity_layer(self.color[3]);
        target.scene.fill_rect(&kurbo::Rect::new(
            0.0,
            0.0,
            f64::from(image.size[0]),
            f64::from(image.size[1]),
        ));
        target.scene.pop_layer();
        target.scene.restore_state(saved);
    }
}

#[derive(Default)]
struct CachedText {
    content: String,
    font_size: f32,
    layout: Layout<()>,
    used: bool,
}

fn cached_layout<'a>(
    buffers: &'a mut HashMap<u64, CachedText>,
    key: u64,
    content: &str,
    font_size: f32,
) -> &'a Layout<()> {
    let cached = buffers.entry(key).or_default();
    if cached.content != content || cached.font_size != font_size {
        content.clone_into(&mut cached.content);
        cached.font_size = font_size;
        cached.layout = layout_text(content, font_size);
    }
    cached.used = true;
    &cached.layout
}

#[derive(Default)]
pub(super) struct TextState {
    buffers: HashMap<u64, CachedText>,
    bitmaps: HashMap<BitmapKey, BitmapImage>,
    outline_glyphs: Vec<glifo::Glyph>,
}

impl TextState {
    pub(super) fn append_to_scene(
        &mut self,
        target: &mut TextTarget<'_>,
        spec: &TextSpec<'_>,
        active_text: Option<(u64, &Layout<()>)>,
    ) {
        let layout = match active_text {
            Some((key, layout)) if key == spec.key => layout,
            _ => cached_layout(&mut self.buffers, spec.key, spec.content, spec.font_size),
        };
        spec.append_to_scene(layout, target, &mut self.bitmaps, &mut self.outline_glyphs);
    }

    pub(super) fn finish_frame(&mut self, target: &mut TextTarget<'_>) {
        self.buffers
            .retain(|_, cached| std::mem::take(&mut cached.used));
        self.bitmaps.retain(|_, image| {
            if std::mem::take(&mut image.used) {
                true
            } else {
                target
                    .renderer
                    .destroy_image(target.resources, target.encoder, image.id);
                false
            }
        });
    }
}
