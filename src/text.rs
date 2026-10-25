//! Font discovery, shaping and metrics shared by editing and rendering.

use std::borrow::Cow;
use std::cell::RefCell;

use parley::{Alignment, FontContext, Layout, LayoutContext, LineHeight, StyleProperty};

const LINE_HEIGHT_SCALE: f32 = 1.25;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextFont {
    pub family: Vec<parley::style::FontFamilyName<'static>>,
    pub weight: parley::style::FontWeight,
    pub style: parley::style::FontStyle,
    pub features: Vec<parley::style::FontFeature>,
}

thread_local! {
    static TEXT_FONT: RefCell<Option<TextFont>> = const { RefCell::new(None) };
}

pub(crate) fn set_text_font(font: TextFont) {
    TEXT_FONT.set(Some(font));
}

pub(crate) fn text_styles() -> [StyleProperty<'static, ()>; 5] {
    TEXT_FONT.with_borrow(|font| {
        let font = font.as_ref().expect("text font initialized at startup");
        [
            StyleProperty::FontFamily(parley::style::FontFamily::List(Cow::Owned(
                font.family.clone(),
            ))),
            StyleProperty::FontWeight(font.weight),
            StyleProperty::FontStyle(font.style),
            StyleProperty::FontFeatures(parley::style::FontFeatures::List(Cow::Owned(
                font.features.clone(),
            ))),
            StyleProperty::LineHeight(LineHeight::FontSizeRelative(LINE_HEIGHT_SCALE)),
        ]
    })
}

thread_local! {
    // The event loop and all outputs share font discovery and shaping caches.
    static TEXT_CONTEXT: RefCell<(FontContext, LayoutContext<()>)> =
        RefCell::new((FontContext::new(), LayoutContext::new()));
}

pub(crate) fn with_text_context<T>(
    f: impl FnOnce(&mut FontContext, &mut LayoutContext<()>) -> T,
) -> T {
    TEXT_CONTEXT.with_borrow_mut(|(fonts, layouts)| f(fonts, layouts))
}

pub(crate) fn text_line_height(font_size: f32) -> f32 {
    font_size * LINE_HEIGHT_SCALE
}

pub(crate) fn text_padding(font_size: f32) -> [f32; 2] {
    [font_size * 0.25, font_size * 0.125]
}

pub(crate) fn text_bounds(
    [left, top]: [f32; 2],
    [width, height]: [f32; 2],
    font_size: f32,
    background_roundness: Option<f32>,
    [scale_x, scale_y]: [f32; 2],
) -> [[f32; 2]; 2] {
    let [padding_x, padding_y] = if background_roundness.is_some() {
        text_padding(font_size)
    } else {
        [0.0; 2]
    };
    // Expand in text space so the background and its editing bounds stretch together.
    let width = background_roundness.map_or(width, |roundness| {
        width.max((height + 2.0 * padding_y) * roundness - 2.0 * padding_x)
    });
    let end_x = left + (width + padding_x) * scale_x;
    let end_y = top + (height + padding_y) * scale_y;
    let start_x = left - padding_x * scale_x;
    let start_y = top - padding_y * scale_y;
    let min_x = start_x.min(end_x);
    let min_y = start_y.min(end_y);
    let max_x = start_x.max(end_x);
    let max_y = start_y.max(end_y);
    [[min_x, min_y], [max_x, max_y]]
}

pub(crate) fn layout_text(content: &str, font_size: f32) -> Layout<()> {
    with_text_context(|fonts, layouts| {
        let mut builder = layouts.ranged_builder(fonts, content, 1.0, false);
        for style in text_styles() {
            builder.push_default(style);
        }
        builder.push_default(StyleProperty::FontSize(font_size));
        let mut layout = builder.build(content);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, Default::default());
        layout
    })
}
