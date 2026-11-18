// SPDX-License-Identifier: AGPL-3.0-or-later

//! Export paint order matches the canvas (#422, SPEC §8): the canvas draws
//! every fill under the geometry, so a filled shape must never be written
//! after, and painted over, a stroke or a smaller fill inside it.
//!
//! Regenerate after an intended change with
//! `LOCUS_UPDATE_GOLDEN=1 cargo test -p locus-export --test paint_order_golden`
//! and review the diff of `tests/fixtures/paint-order/`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{Command, Document, Editor, EntityId, Fill, Geom, LayerId, Style};
use locus_export::{
    Background, PngOptions, Resolution, prepare, to_pdf_model, to_png_model, to_svg_model,
};
use locus_geom::Vec2;
use std::path::PathBuf;

fn add(ed: &mut Editor, layer: LayerId, geom: Geom, style: Style) -> EntityId {
    ed.apply(&Command::AddEntity {
        layer,
        construction: false,
        geom,
        style,
    })
    .unwrap()
    .entities[0]
}

fn square(ed: &mut Editor, x0: f64, x1: f64, style: Style) {
    let layer = Document::default_layer();
    let pts: Vec<EntityId> = [(x0, x0), (x1, x0), (x1, x1), (x0, x1)]
        .iter()
        .map(|&(x, y)| {
            add(
                ed,
                layer,
                Geom::Point {
                    pos: Vec2::new(x, y),
                },
                Style::default(),
            )
        })
        .collect();
    for i in 0..4 {
        let (start, end) = (pts[i], pts[(i + 1) % 4]);
        add(ed, layer, Geom::Line { start, end }, style);
    }
}

/// The owner's file in miniature, in the order it was drawn: a thick
/// unfilled loop (the offset), then a large white-filled rectangle around
/// it, then a small red-filled square inside both.
fn doc() -> Document {
    let mut ed = Editor::new(Document::new());
    square(
        &mut ed,
        20.0,
        80.0,
        Style {
            width: 4.0,
            ..Style::default()
        },
    );
    square(
        &mut ed,
        0.0,
        100.0,
        Style {
            fill: Fill::Colour([255, 255, 255]),
            ..Style::default()
        },
    );
    square(
        &mut ed,
        40.0,
        60.0,
        Style {
            fill: Fill::Colour([255, 0, 0]),
            ..Style::default()
        },
    );
    ed.into_document()
}

fn golden(name: &str, got: &[u8]) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/paint-order")
        .join(name);
    if std::env::var_os("LOCUS_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
    }
    let text = |b: &[u8]| {
        b.iter()
            .map(|&c| if c.is_ascii() { char::from(c) } else { '?' })
            .collect::<String>()
            .replace("\r\n", "\n")
    };
    let want = std::fs::read(&path)
        .unwrap_or_else(|_| panic!("missing golden {name}; create it with LOCUS_UPDATE_GOLDEN=1"));
    assert_eq!(
        text(got),
        text(&want),
        "{name} differs from its golden copy"
    );
}

#[test]
fn svg_and_pdf_match_their_goldens() {
    let model = prepare(&doc(), false).model;
    golden("overlap.svg", &to_svg_model(&model));
    golden("overlap.pdf", &to_pdf_model(&model));
}

/// Fills are written outermost first, and before every unfilled stroke.
#[test]
fn fills_are_written_first_outermost_first() {
    let svg = String::from_utf8(to_svg_model(&prepare(&doc(), false).model)).unwrap();
    let at = |needle: &str| {
        svg.find(needle)
            .unwrap_or_else(|| panic!("{needle}: {svg}"))
    };
    let (white, red, thick) = (
        at("fill=\"#ffffff\""),
        at("fill=\"#ff0000\""),
        at("stroke-width=\"4\""),
    );
    assert!(white < red && red < thick, "{svg}");
}

/// The PNG shows what the canvas shows: the thick stroke and the red fill
/// over the white rectangle.
#[test]
fn png_shows_the_stroke_and_the_inner_fill() {
    let o = PngOptions {
        resolution: Resolution::Dpi(25.4), // 1 px per mm
        background: Background::Transparent,
    };
    let png = to_png_model(&prepare(&doc(), false).model, &o).unwrap();
    let img = tiny_skia::Pixmap::decode_png(&png).unwrap();
    // 1 mm margin; Y flipped about the top (y = 100 mm).
    let px = |x: u32, y: u32| img.pixel(x + 1, 100 - y + 1).unwrap();
    let stroke = px(20, 50);
    assert!(
        stroke.red() < 32 && stroke.alpha() == 255,
        "thick stroke hidden: {stroke:?}"
    );
    let red = px(50, 50);
    assert!(
        red.red() > 224 && red.green() < 32,
        "inner fill hidden: {red:?}"
    );
    let white = px(10, 50);
    assert!(white.red() > 224 && white.green() > 224, "{white:?}");
}
