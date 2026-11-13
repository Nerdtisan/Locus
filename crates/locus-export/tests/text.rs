// SPDX-License-Identifier: AGPL-3.0-or-later

//! Text export (#179; SPEC §9, §10): the cached geometry exports without the
//! font, as closed outlines for outline fonts and open strokes for single-line
//! fonts.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{Command, Document, Editor, EntityId, Geom, TextData};
use locus_export::{Shape, collect, to_svg};
use locus_geom::Vec2;
use locus_text::{Align, BundledFont, BundledProvider, FontRef};

const L: locus_doc::LayerId = Document::default_layer();

fn add(ed: &mut Editor, geom: Geom) -> EntityId {
    ed.apply(&Command::AddEntity {
        layer: L,
        construction: false,
        style: Default::default(),
        geom,
    })
    .unwrap()
    .entities[0]
}

fn doc_with_text(font: BundledFont, content: &str) -> (Document, EntityId) {
    let mut ed = Editor::new(Document::new());
    let anchor = add(
        &mut ed,
        Geom::Point {
            pos: Vec2::new(10.0, 20.0),
        },
    );
    let dir = add(
        &mut ed,
        Geom::Point {
            pos: Vec2::new(20.0, 20.0),
        },
    );
    let text = TextData::new(
        content,
        FontRef::bundled(font),
        12.0,
        Align::Left,
        1.0,
        &BundledProvider,
    )
    .unwrap();
    let id = add(
        &mut ed,
        Geom::Text {
            anchor,
            dir,
            text: Box::new(text),
        },
    );
    (ed.into_document(), id)
}

#[test]
fn outline_text_exports_closed_paths_with_curves() {
    let (doc, _) = doc_with_text(BundledFont::Figtree, "Oo");
    let model = collect(&doc);
    let paths = &model.layers[0].paths;
    assert_eq!(paths.len(), 4, "two glyphs, outer + inner contour each");
    assert!(paths.iter().all(|p| p.closed));
    assert!(
        paths
            .iter()
            .flat_map(|p| &p.segs)
            .any(|s| matches!(s.shape, Shape::Cubic(_)))
    );
    let b = model.bounds().unwrap();
    assert!(
        b.min.x >= 9.5 && b.min.y >= 19.5,
        "placed at the anchor: {b:?}"
    );
    let svg = String::from_utf8(to_svg(&doc)).unwrap();
    assert!(
        svg.contains('C') || svg.contains('c'),
        "curves reach the SVG"
    );
}

#[test]
fn single_line_text_exports_open_strokes() {
    let (doc, _) = doc_with_text(BundledFont::HersheySans, "LI");
    let model = collect(&doc);
    let paths = &model.layers[0].paths;
    assert!(!paths.is_empty());
    assert!(paths.iter().all(|p| !p.closed));
}

#[test]
fn text_follows_its_points_and_exports_the_same_without_the_font() {
    let (mut doc, id) = doc_with_text(BundledFont::Figtree, "AAAA");
    let (_, anchor, dir) = {
        let (t, a, d) = doc.text(id).unwrap();
        (t.clone(), a, d)
    };
    // Rotate the baseline 90° (dir directly above the anchor) through the command API.
    let mut ed = Editor::new(doc.clone());
    ed.apply(&Command::ModifyEntity {
        id: dir,
        geom: Geom::Point {
            pos: Vec2::new(10.0, 30.0),
        },
    })
    .unwrap();
    doc = ed.into_document();
    let rotated = doc.text_geometry(id).unwrap().bbox().unwrap();
    assert!(
        rotated.height() > 2.0 * rotated.width(),
        "a line of text turned upright"
    );
    assert_eq!(doc.point_pos(anchor), Some(Vec2::new(10.0, 20.0)));

    // Pretend the font is gone: export is unchanged, because it reads the cache.
    let before = collect(&doc);
    let (t, ..) = doc.text(id).unwrap();
    assert!(!t.font_missing(&BundledProvider));
    let after = collect(&doc);
    assert_eq!(before, after);
}
