//! `Window::paint_glyph` paints a glyph on its own the way a line paints its
//! glyphs, keeping the rendering of a run from one glyph to the next. What it
//! draws must be what the line draws: the same sprites, from the same tiles,
//! in the same colors, when the font size and the color change between runs.
//!
//! `ShapedLine::paint_glyphs_scaled` paints a line's glyphs at their scaled
//! places: unscaled, where the line paints them, and scaled, where painting
//! each glyph on its own at its scaled position and size puts them.

use std::sync::Arc;

use super::oracle::GlyphBoxTextSystem;
use crate::{
    App, AppContext as _, Bounds, Context, DevicePixels, Hsla, IntoElement, NoopTextSystem, Pixels,
    Point, Render, ScaledPixels, ShapedLine, Styled as _, TestAppContext, TextAlign, TextRun,
    Window, blue, canvas, font, point, px, red,
};

const LINE_HEIGHT: Pixels = px(20.);

type PaintLine = fn(&ShapedLine, Point<Pixels>, Hsla, &mut Window, &mut App);

/// Three lines painted by `paint`: two alike around one in another size and
/// color, so a rendering kept from a run is met by a run it is not for.
struct Lines {
    paint: PaintLine,
}

impl Render for Lines {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let paint = self.paint;
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                for (font_size, color, top) in [
                    (px(14.), red(), px(10.)),
                    (px(9.), blue(), px(40.)),
                    (px(14.), red(), px(70.)),
                ] {
                    let run = TextRun {
                        len: 5,
                        font: font("Helvetica"),
                        color,
                        ..Default::default()
                    };
                    let line =
                        window
                            .text_system()
                            .shape_line("Glyph".into(), font_size, &[run], None);
                    paint(&line, point(px(10.3), top), color, window, cx);
                }
            },
        )
        .size_full()
    }
}

fn paint_as_a_line(
    line: &ShapedLine,
    origin: Point<Pixels>,
    _: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    line.paint(origin, LINE_HEIGHT, TextAlign::Left, None, window, cx)
        .unwrap();
}

fn paint_glyph_by_glyph(
    line: &ShapedLine,
    origin: Point<Pixels>,
    color: Hsla,
    window: &mut Window,
    _: &mut App,
) {
    paint_glyphs_at_scale(line, origin, color, 1., window);
}

const SCALE: f32 = 1.5;

fn paint_glyph_by_glyph_scaled(
    line: &ShapedLine,
    origin: Point<Pixels>,
    color: Hsla,
    window: &mut Window,
    _: &mut App,
) {
    paint_glyphs_at_scale(line, origin, color, SCALE, window);
}

fn paint_glyphs_at_scale(
    line: &ShapedLine,
    origin: Point<Pixels>,
    color: Hsla,
    scale: f32,
    window: &mut Window,
) {
    let baseline =
        origin.y + (LINE_HEIGHT - (line.ascent + line.descent) * scale) / 2. + line.ascent * scale;
    for run in &line.runs {
        for glyph in &run.glyphs {
            window
                .paint_glyph(
                    point(
                        origin.x + glyph.position.x * scale,
                        baseline + glyph.position.y * scale,
                    ),
                    run.font_id,
                    glyph.id,
                    line.font_size * scale,
                    color,
                )
                .unwrap();
        }
    }
}

fn paint_scaled_by_one(
    line: &ShapedLine,
    origin: Point<Pixels>,
    _: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    line.paint_glyphs_scaled(origin, LINE_HEIGHT, TextAlign::Left, None, 1., window, cx)
        .unwrap();
}

fn paint_scaled(
    line: &ShapedLine,
    origin: Point<Pixels>,
    _: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    line.paint_glyphs_scaled(
        origin,
        LINE_HEIGHT,
        TextAlign::Left,
        None,
        SCALE,
        window,
        cx,
    )
    .unwrap();
}

/// Where each glyph was drawn, from which part of the atlas and in what
/// color, top to bottom and left to right.
fn sprites(paint: PaintLine) -> Vec<(Bounds<ScaledPixels>, Bounds<DevicePixels>, Hsla)> {
    let mut cx = TestAppContext::with_text_system(Arc::new(GlyphBoxTextSystem(NoopTextSystem)));
    let window = cx.add_window(move |_, _| Lines { paint });
    let mut sprites = cx
        .update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let scene = &window.rendered_frame.scene;
            assert!(scene.subpixel_sprites.is_empty());
            scene
                .monochrome_sprites
                .iter()
                .map(|sprite| (sprite.bounds, sprite.tile.bounds, sprite.color))
                .collect::<Vec<_>>()
        })
        .unwrap();
    sprites.sort_by(|(a, _, _), (b, _, _)| {
        (a.origin.y.0, a.origin.x.0)
            .partial_cmp(&(b.origin.y.0, b.origin.x.0))
            .unwrap()
    });
    sprites
}

#[test]
fn glyphs_painted_on_their_own_are_drawn_as_their_line_draws_them() {
    let line = sprites(paint_as_a_line);
    assert_eq!(line.len(), 15);
    assert_eq!(sprites(paint_glyph_by_glyph), line);
}

#[test]
fn a_line_scaled_by_one_is_drawn_as_the_line() {
    assert_eq!(sprites(paint_scaled_by_one), sprites(paint_as_a_line));
}

#[test]
fn a_scaled_line_draws_each_glyph_at_its_scaled_place_and_size() {
    let scaled = sprites(paint_scaled);
    assert_eq!(scaled.len(), 15);
    assert_ne!(scaled, sprites(paint_as_a_line));
    assert_eq!(scaled, sprites(paint_glyph_by_glyph_scaled));
}
