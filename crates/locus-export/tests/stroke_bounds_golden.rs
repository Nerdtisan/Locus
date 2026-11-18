// SPDX-License-Identifier: AGPL-3.0-or-later

//! The export page includes the strokes (#450, SPEC §8): a thick stroke on
//! a circle or rectangle is not cut off at the page edge in SVG, PDF or PNG,
//! and a job placed on the bed (#443) keeps its coordinates.
//!
//! Regenerate after an intended change with
//! `LOCUS_UPDATE_GOLDEN=1 cargo test -p locus-export --test stroke_bounds_golden`
//! and review the diff of `tests/fixtures/stroke-bounds/`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{Command, Document, Editor, EntityId, Geom, LayerId, Style};
use locus_export::{
    Background, PngOptions, Resolution, prepare, prepare_for, profile, to_pdf_model, to_png_model,
    to_svg_model,
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

fn point(ed: &mut Editor, x: f64, y: f64) -> EntityId {
    add(
        ed,
        Document::default_layer(),
        Geom::Point {
            pos: Vec2::new(x, y),
        },
        Style::default(),
    )
}

fn thick(w: f64) -> Style {
    Style {
        width: w,
        ..Style::default()
    }
}

/// The owner's circle: r 178.885 mm, 4.06 mm stroke, centred on the origin.
fn circle() -> Document {
    let mut ed = Editor::new(Document::new());
    let c = point(&mut ed, 0.0, 0.0);
    add(
        &mut ed,
        Document::default_layer(),
        Geom::Circle {
            center: c,
            radius: 178.885,
        },
        thick(4.06),
    );
    ed.into_document()
}

/// A 60 × 40 rectangle from the origin with a 3 mm stroke.
fn rectangle() -> Document {
    let mut ed = Editor::new(Document::new());
    let p: Vec<_> = [(0.0, 0.0), (60.0, 0.0), (60.0, 40.0), (0.0, 40.0)]
        .iter()
        .map(|&(x, y)| point(&mut ed, x, y))
        .collect();
    for i in 0..4 {
        add(
            &mut ed,
            Document::default_layer(),
            Geom::Line {
                start: p[i],
                end: p[(i + 1) % 4],
            },
            thick(3.0),
        );
    }
    ed.into_document()
}

fn golden(name: &str, got: &[u8]) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/stroke-bounds")
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
    for (name, doc) in [("circle", circle()), ("rectangle", rectangle())] {
        let model = prepare(&doc, false).model;
        golden(&format!("{name}.svg"), &to_svg_model(&model));
        golden(&format!("{name}.pdf"), &to_pdf_model(&model));
    }
}

/// The owner's file: the page is the diameter plus the stroke, so nothing
/// is clipped (it used to be the bare 357.77 mm diameter).
#[test]
fn the_page_includes_half_the_stroke_on_every_side() {
    let model = prepare(&circle(), false).model;
    let svg = String::from_utf8(to_svg_model(&model)).unwrap();
    // 2 × 178.885 + 4.06 = 361.83
    assert!(
        svg.contains("width=\"361.83mm\" height=\"361.83mm\" viewBox=\"0 0 361.83 361.83\""),
        "{svg}"
    );
    assert!(
        svg.contains("cx=\"180.915\" cy=\"180.915\" r=\"178.885\""),
        "{svg}"
    );
    let pdf = String::from_utf8_lossy(&to_pdf_model(&model)).into_owned();
    // 361.83 mm in points.
    assert!(pdf.contains("/MediaBox [0 0 1025.6598 1025.6598]"), "{pdf}");
    let rect = String::from_utf8(to_svg_model(&prepare(&rectangle(), false).model)).unwrap();
    assert!(
        rect.contains("width=\"63mm\" height=\"43mm\" viewBox=\"0 0 63 43\""),
        "{rect}"
    );
}

/// The PNG has the whole stroke: 1 px per mm, a 1 mm margin, so the
/// stroke's outer edge is opaque just inside the margin on each side.
#[test]
fn png_is_not_clipped() {
    let o = PngOptions {
        resolution: Resolution::Dpi(25.4),
        background: Background::Transparent,
    };
    let bytes = to_png_model(&prepare(&rectangle(), false).model, &o).unwrap();
    let img = tiny_skia::Pixmap::decode_png(&bytes).unwrap();
    // 63 + 2 × 1 mm margin.
    assert_eq!((img.width(), img.height()), (65, 45));
    let alpha = |x: u32, y: u32| img.pixel(x, y).unwrap().alpha();
    // Left edge: the stroke spans x 1..4 (1 mm margin, 3 mm stroke).
    assert!(alpha(2, 22) > 200, "left stroke drawn");
    assert_eq!(alpha(0, 22), 0, "margin stays clear");
    assert!(alpha(62, 22) > 200, "right stroke drawn");
}

/// A job placed on the bed (#443) keeps its coordinates: the page is the
/// bed, grown only where a stroke crosses its edge, by a negative `viewBox`
/// origin rather than by moving the job.
#[test]
fn a_placed_job_keeps_its_coordinates() {
    let mut ed = Editor::new(rectangle());
    let lb = profile::find("lightburn", &[]).unwrap();
    ed.apply(&Command::SetPlacement {
        offset: Some(Vec2::new(0.0, 0.0)),
    })
    .unwrap();
    // The rectangle sits on the bed's bottom-left corner: its stroke pokes
    // 1.5 mm past the left and bottom edges.
    let svg = String::from_utf8(to_svg_model(
        &prepare_for(ed.document(), false, Some(&lb)).model,
    ))
    .unwrap();
    assert!(
        svg.contains("width=\"401.5mm\" height=\"401.5mm\" viewBox=\"-1.5 0 401.5 401.5\""),
        "{svg}"
    );
    // The corner at bed (0, 0) is still written at x 0, y 400 (Y down).
    assert!(svg.contains("M 0 400"), "{svg}");
    let pdf = String::from_utf8_lossy(&to_pdf_model(
        &prepare_for(ed.document(), false, Some(&lb)).model,
    ))
    .into_owned();
    assert!(
        pdf.contains("/MediaBox [-4.252 -4.252 1133.8583 1133.8583]"),
        "{pdf}"
    );
}
