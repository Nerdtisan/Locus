// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reference layer behaviour of an import (#243; SPEC §4 layer rules,
//! §7 Reference): locked, never exported, snappable and constrainable.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{Command, Constraint, Dimension, Document, EntityId, Geom, Operation, Sketcher};
use locus_geom::Vec2;
use locus_import::{Format, ImportError, detect, reference_command};

const SVG: &[u8] = include_bytes!("fixtures/lightburn.svg");
const DXF: &[u8] = include_bytes!("fixtures/librecad.dxf");

#[test]
fn format_is_found_by_extension_then_content() {
    assert_eq!(detect("a.SVG", b""), Some(Format::Svg));
    assert_eq!(detect("a.dxf", b""), Some(Format::Dxf));
    assert_eq!(detect("noext", SVG), Some(Format::Svg));
    assert_eq!(detect("noext", DXF), Some(Format::Dxf));
    assert_eq!(detect("a.png", b"\x89PNG"), None);
}

#[test]
fn import_is_one_locked_reference_layer_named_after_the_file() {
    for (name, bytes) in [("C:\\tmp\\plan.svg", SVG), ("/home/x/part.dxf", DXF)] {
        let mut s = Sketcher::new(Document::new());
        let before = s.document().clone();
        let (cmd, _warnings) = reference_command(s.document(), name, bytes).unwrap();
        let applied = s.apply(&cmd).unwrap();
        let layer = s.document().layer(applied.layers[0]).unwrap();
        assert!(layer.locked);
        assert_eq!(layer.operation, Operation::Reference);
        assert!(
            layer.name == "plan.svg" || layer.name == "part.dxf",
            "{}",
            layer.name
        );
        s.undo().unwrap();
        // IDs are never reused, so only the content is compared.
        assert_eq!(s.document().layers(), before.layers());
        assert!(s.document().entities().is_empty());
    }
}

#[test]
fn unknown_and_empty_files_are_refused_without_changing_anything() {
    let doc = Document::new();
    assert_eq!(
        reference_command(&doc, "a.png", b"junk").unwrap_err(),
        ImportError::UnknownFormat
    );
    assert!(
        reference_command(&doc, "a.svg", b"<svg xmlns='http://www.w3.org/2000/svg'/>").is_err()
    );
}

fn first_point(doc: &Document, layer_op: Operation) -> EntityId {
    doc.entities()
        .iter()
        .find(|(_, e)| {
            matches!(e.geom, Geom::Point { .. })
                && doc.layer(e.layer).is_some_and(|l| l.operation == layer_op)
        })
        .map(|(id, _)| id)
        .unwrap()
}

#[test]
fn reference_geometry_acts_as_fixed_geometry_for_constraints() {
    let mut s = Sketcher::new(Document::new());
    let (cmd, _) = reference_command(s.document(), "plan.svg", SVG).unwrap();
    s.apply(&cmd).unwrap();
    let r = first_point(s.document(), Operation::Reference);
    let r_at = s.document().point_pos(r).unwrap();

    let p = s
        .apply(&Command::AddEntity {
            layer: Document::default_layer(),
            construction: false,
            style: Default::default(),
            geom: Geom::Point {
                pos: Vec2::new(r_at.x + 3.0, r_at.y + 1.0),
            },
        })
        .unwrap()
        .entities[0];
    s.apply(&Command::AddConstraint {
        constraint: Constraint::Distance {
            a: r,
            b: p,
            dim: Dimension::driving(20.0),
        },
    })
    .unwrap();
    // The reference point never moves; the sketch point does.
    assert_eq!(s.document().point_pos(r).unwrap(), r_at);
    let d = (s.document().point_pos(p).unwrap() - r_at).length();
    assert!((d - 20.0).abs() < 1e-6, "{d}");
    // And it cannot be edited directly.
    assert!(
        s.apply(&Command::ModifyEntity {
            id: r,
            geom: Geom::Point { pos: Vec2::ZERO },
        })
        .is_err()
    );
}

#[test]
fn reference_layers_are_never_exported() {
    let mut s = Sketcher::new(Document::new());
    let a = s
        .apply(&Command::AddEntity {
            layer: Document::default_layer(),
            construction: false,
            style: Default::default(),
            geom: Geom::Point { pos: Vec2::ZERO },
        })
        .unwrap()
        .entities[0];
    let b = s
        .apply(&Command::AddEntity {
            layer: Document::default_layer(),
            construction: false,
            style: Default::default(),
            geom: Geom::Point {
                pos: Vec2::new(10.0, 0.0),
            },
        })
        .unwrap()
        .entities[0];
    s.apply(&Command::AddEntity {
        layer: Document::default_layer(),
        construction: false,
        style: Default::default(),
        geom: Geom::Line { start: a, end: b },
    })
    .unwrap();
    let plain = locus_export::svg::to_svg(s.document());
    let (cmd, _) = reference_command(s.document(), "plan.svg", SVG).unwrap();
    s.apply(&cmd).unwrap();
    let with_ref = locus_export::svg::to_svg(s.document());
    assert_eq!(plain, with_ref);
}

/// #390: a PNG lands as one locked Reference layer holding the image, in
/// one undo step, centred where asked; a damaged one changes nothing.
#[test]
fn a_png_imports_as_a_locked_reference_image() {
    use locus_import::reference_command_at;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(20, 10, image::Rgba([0, 0, 0, 255]))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let bytes = bytes.into_inner();
    let mut s = Sketcher::new(Document::new());
    let centre = locus_geom::Vec2::new(50.0, 40.0);
    let (cmd, warnings) =
        reference_command_at(s.document(), "C:/pics/photo.png", &bytes, centre).unwrap();
    assert!(warnings.iter().any(|w| w.contains("Calibrate")));
    s.apply(&cmd).unwrap();
    let doc = s.document();
    let (layer, img) = doc.images().iter().next().unwrap();
    let l = doc.layer(*layer).unwrap();
    assert_eq!(l.name, "photo.png");
    assert!(l.locked && !l.operation.is_exportable());
    assert_eq!((img.width_px, img.height_px), (20, 10));
    let (lo, hi) = img.rect();
    assert!(((lo + hi) * 0.5 - centre).length() < 1e-9);
    s.undo().unwrap();
    assert!(s.document().images().is_empty(), "one undo step");
    let before = s.document().clone();
    assert_eq!(
        reference_command_at(&before, "x.png", &bytes[..bytes.len() / 2], centre).unwrap_err(),
        ImportError::BadImage
    );
}
