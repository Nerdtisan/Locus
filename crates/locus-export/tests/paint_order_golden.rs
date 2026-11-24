// SPDX-License-Identifier: AGPL-3.0-or-later

//! Export paint order matches the canvas (#422, #451, SPEC §8): fills
//! belong to regions and are all written before any stroke, so no fill can
//! hide a line, and each region is filled on its own (a ring is not its
//! hole).
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
        add(ed, layer, Geom::Line { start, end }, style.clone());
    }
}

/// Fill the region around `at` (#451): fills belong to regions.
fn fill_at(ed: &mut Editor, at: Vec2, colour: [u8; 3], opacity: f64) {
    let key = locus_doc::regions::regions(ed.document())
        .into_iter()
        .find(|r| r.contains(at))
        .expect("a region there")
        .key;
    ed.apply(&Command::SetRegionFill {
        regions: vec![key],
        fill: Some(locus_doc::region_fill::RegionFill {
            fill: Fill::Colour(colour),
            opacity,
        }),
    })
    .unwrap();
}

/// The owner's file in miniature, in the order it was drawn: a thick
/// unfilled loop (the offset), then a large rectangle around it, then a
/// small square inside both. The outer ring is filled white and the small
/// square red; the ring between the loop and the square stays open.
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
    square(&mut ed, 0.0, 100.0, Style::default());
    square(&mut ed, 40.0, 60.0, Style::default());
    fill_at(&mut ed, Vec2::new(10.0, 50.0), [255, 255, 255], 1.0);
    fill_at(&mut ed, Vec2::new(50.0, 50.0), [255, 0, 0], 1.0);
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

/// Every fill is written before every stroke (#451).
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
    // The ring between the thick loop and the red square is its own
    // region, left unfilled: no other fill covers it (#451).
    let open = px(30, 50);
    assert_eq!(open.alpha(), 0, "the open ring: {open:?}");
}
