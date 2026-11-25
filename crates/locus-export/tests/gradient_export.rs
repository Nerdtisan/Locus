// SPDX-License-Identifier: AGPL-3.0-or-later

//! Gradient fills in export (#436, ADR 0019 decision 4): SVG writes a
//! native linear or radial gradient in the region's frame (conic as clipped
//! bands), PDF writes native axial/radial shading clipped to the region
//! (#513; conic as bands), and PNG paints each pixel
//! from the same sampler as the canvas.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::gradient::{Gradient, GradientKind, Stop};
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

/// The PDF's text (its content stream is not compressed).
fn pdf_text(g: Gradient) -> String {
    String::from_utf8_lossy(&to_pdf_model(&prepare(&doc(g), false).model)).into_owned()
}

/// #513: linear and radial gradients are native smooth shadings clipped to
/// the region, not bands; more than two stops stitch; stop opacity is a
/// soft mask; conic stays bands (PDF has no conic shading).
#[test]
fn pdf_writes_native_smooth_shading() {
    // Two stops: one axial shading with an exponential function.
    let t = pdf_text(blue_to_white());
    assert!(t.contains("W* n"), "the region clips it");
    assert!(t.contains("/ShadingType 2"), "{t}");
    assert!(t.contains("/Sh0 sh"));
    assert!(t.contains("/Shading << /Sh0 "));
    assert!(t.contains("/FunctionType 2 /Domain [0 1] /C0 [0 0 1] /C1 [1 1 1] /N 1"));
    assert!(!t.contains("/FunctionType 3"));
    assert!(!t.contains("/SMask"), "opaque stops need no mask");
    assert!(
        t.matches(" rg").count() < 4,
        "no bands: {} fills",
        t.matches(" rg").count()
    );
    // Four stops: a stitching function over three pieces.
    let mut four = blue_to_white();
    four.stops = vec![
        Stop {
            pos: 0.0,
            colour: [255, 0, 0],
            opacity: 1.0,
        },
        Stop {
            pos: 0.25,
            colour: [0, 255, 0],
            opacity: 1.0,
        },
        Stop {
            pos: 0.75,
            colour: [0, 0, 255],
            opacity: 1.0,
        },
        Stop {
            pos: 1.0,
            colour: [255, 255, 255],
            opacity: 1.0,
        },
    ];
    let t = pdf_text(four.clone());
    assert!(t.contains("/FunctionType 3 /Domain [0 1]"), "{t}");
    assert!(t.contains("/Bounds [0.25 0.75]"));
    assert!(t.contains("/Encode [0 1 0 1 0 1]"));
    // Elliptical: a radial shading, squashed by its cm.
    let mut ell = blue_to_white();
    ell.kind = GradientKind::Elliptical { ratio: 0.5 };
    let t = pdf_text(ell);
    assert!(t.contains("/ShadingType 3 /Coords [0 0 0 0 0 "), "{t}");
    // Stop opacity: a luminosity soft mask from a grey copy.
    let mut fade = four;
    fade.stops[3].opacity = 0.0;
    let t = pdf_text(fade);
    assert!(t.contains("/SMask << /Type /Mask /S /Luminosity"), "{t}");
    assert!(t.contains("/ColorSpace /DeviceGray"));
    assert!(t.contains("/SM0 gs"));
    // Conic: clipped bands still.
    let mut conic = blue_to_white();
    conic.kind = GradientKind::Conic;
    let t = pdf_text(conic);
    assert!(!t.contains("/ShadingType"));
    assert!(t.matches(" rg").count() >= locus_export::gradient::BANDS);
}

/// Rotate with shape (#513): the shading follows the region's frame, so a
/// turned frame changes the matrix the shading is drawn with.
#[test]
fn pdf_shading_follows_the_frame() {
    let cm = |t: &str| {
        t.lines()
            .find(|l| l.ends_with(" cm /Sh0 sh"))
            .map(str::to_owned)
            .expect("a shading")
    };
    let level = cm(&pdf_text(blue_to_white()));
    let mut turned = blue_to_white();
    turned.frame_angle = std::f64::consts::FRAC_PI_2;
    assert_ne!(level, cm(&pdf_text(turned)));
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
