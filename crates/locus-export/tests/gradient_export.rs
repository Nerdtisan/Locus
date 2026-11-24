// SPDX-License-Identifier: AGPL-3.0-or-later

//! Gradient fills in export (#436, ADR 0019 decision 4): SVG writes a
//! native linear or radial gradient in the region's frame (conic as clipped
//! bands), PDF clips stepped bands to the region, and PNG paints each pixel
//! from the same sampler as the canvas.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::gradient::{Gradient, GradientKind};
use locus_doc::region_fill::RegionFill;
use locus_doc::{Command, Document, Editor, Fill, Geom, Style};
use locus_export::{PngOptions, prepare, to_pdf_model, to_png_model, to_svg_model};
use locus_geom::Vec2;

/// A 40 × 20 mm rectangle on the default (Draw) layer, filled with `g`.
fn doc(g: Gradient) -> Document {
    let mut ed = Editor::new(Document::new());
    let l = Document::default_layer();
    let p: Vec<_> = [(0.0, 0.0), (40.0, 0.0), (40.0, 20.0), (0.0, 20.0)]
        .iter()
        .map(|(x, y)| {
            ed.apply(&Command::AddEntity {
                layer: l,
                construction: false,
                geom: Geom::Point {
                    pos: Vec2::new(*x, *y),
                },
                style: Style::default(),
            })
            .unwrap()
            .entities[0]
        })
        .collect();
    let mut lines = Vec::new();
    for k in 0..4 {
        lines.push(
            ed.apply(&Command::AddEntity {
                layer: l,
                construction: false,
                geom: Geom::Line {
                    start: p[k],
                    end: p[(k + 1) % 4],
                },
                style: Style::default(),
            })
            .unwrap()
            .entities[0],
        );
    }
    let key = locus_doc::regions::regions(ed.document())[0].key.clone();
    ed.apply(&Command::SetRegionFill {
        regions: vec![key],
        fill: Some(RegionFill {
            fill: Fill::Colour([0, 0, 255]),
            opacity: 1.0,
            gradient: Some(Box::new(g)),
        }),
    })
    .unwrap();
    ed.into_document()
}

fn blue_to_white() -> Gradient {
    Gradient::linear_from([0, 0, 255])
}

#[test]
fn svg_writes_a_native_gradient_in_the_regions_frame() {
    let model = prepare(&doc(blue_to_white()), false).model;
    let svg = String::from_utf8(to_svg_model(&model)).unwrap();
    assert!(svg.contains("<linearGradient id=\"grad0\""), "{svg}");
    assert!(svg.contains("gradientUnits=\"userSpaceOnUse\""));
    assert!(svg.contains("stop-color=\"#0000ff\""));
    assert!(svg.contains("fill=\"url(#grad0)\""));
    let mut radial = blue_to_white();
    radial.kind = GradientKind::Elliptical { ratio: 0.5 };
    let svg = String::from_utf8(to_svg_model(&prepare(&doc(radial), false).model)).unwrap();
    assert!(svg.contains("<radialGradient"), "{svg}");
    assert!(svg.contains("scale(1 0.5)"));
    let mut conic = blue_to_white();
    conic.kind = GradientKind::Conic;
    let svg = String::from_utf8(to_svg_model(&prepare(&doc(conic), false).model)).unwrap();
    assert!(svg.contains("<clipPath"), "{svg}");
    assert_eq!(
        svg.matches("<polygon").count(),
        locus_export::gradient::BANDS
    );
}

#[test]
fn pdf_clips_bands_to_the_region() {
    let pdf = to_pdf_model(&prepare(&doc(blue_to_white()), false).model);
    let text = String::from_utf8_lossy(&pdf);
    // The content stream may be compressed; the clip operator and a band
    // colour are there when it isn't.
    if text.contains(" rg") {
        assert!(text.contains("W* n"), "the region clips the bands");
        assert!(text.matches(" rg").count() >= locus_export::gradient::BANDS);
    }
}

#[test]
fn png_paints_the_gradient_across_the_region() {
    let png = to_png_model(
        &prepare(&doc(blue_to_white()), false).model,
        &PngOptions::default(),
    )
    .unwrap();
    let img = tiny_skia::Pixmap::decode_png(&png).unwrap();
    let (w, h) = (img.width(), img.height());
    let px = |x: u32, y: u32| img.pixel(x, y).unwrap().demultiply();
    let left = px(w / 6, h / 2);
    let right = px(w * 5 / 6, h / 2);
    // Blue at the left, near white at the right.
    assert!(left.blue() > 200 && left.red() < 120, "{left:?}");
    assert!(right.red() > left.red() + 80, "{left:?} → {right:?}");
}
