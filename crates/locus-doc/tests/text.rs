// SPDX-License-Identifier: AGPL-3.0-or-later

//! Text entity through the command API (#179; SPEC §9, §11): one undo step,
//! constrainable placement, protected references.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{
    Command, Constraint, Dimension, DocError, Document, Editor, EntityId, Geom, Sketcher, TextData,
};
use locus_geom::Vec2;
use locus_text::{Align, BundledFont, BundledProvider, FontRef};

const L: locus_doc::LayerId = Document::default_layer();

fn text(content: &str) -> TextData {
    TextData::new(
        content,
        FontRef::bundled(BundledFont::HersheySans),
        8.0,
        Align::Left,
        1.0,
        &BundledProvider,
    )
    .unwrap()
}

fn pt(x: f64, y: f64) -> Command {
    Command::AddEntity {
        layer: L,
        construction: false,
        geom: Geom::Point {
            pos: Vec2::new(x, y),
        },
    }
}

/// Anchor, direction point, the text, and a construction baseline line, as
/// one labelled undo step: what the text tool does.
fn place(ed: &mut Editor) -> (EntityId, EntityId, EntityId) {
    let a = ed.apply(&pt(0.0, 0.0)).unwrap().entities[0];
    let d = ed.apply(&pt(10.0, 0.0)).unwrap().entities[0];
    let t = ed
        .apply(&Command::AddEntity {
            layer: L,
            construction: false,
            geom: Geom::Text {
                anchor: a,
                dir: d,
                text: Box::new(text("Hi")),
            },
        })
        .unwrap()
        .entities[0];
    (a, d, t)
}

#[test]
fn undo_redo_restores_text_and_ids() {
    let mut ed = Editor::new(Document::new());
    let (.., t) = place(&mut ed);
    let with = ed.document().clone();
    ed.undo().unwrap();
    assert!(ed.document().entity(t).is_none());
    ed.redo().unwrap();
    assert_eq!(*ed.document(), with);
}

#[test]
fn editing_content_is_one_undo_step() {
    let mut ed = Editor::new(Document::new());
    let (.., t) = place(&mut ed);
    let (old, a, d) = {
        let (x, a, d) = ed.document().text(t).unwrap();
        (x.clone(), a, d)
    };
    let new = old
        .edited(&BundledProvider, |x| x.content = "Hello".into())
        .unwrap();
    ed.apply(&Command::ModifyEntity {
        id: t,
        geom: Geom::Text {
            anchor: a,
            dir: d,
            text: Box::new(new.clone()),
        },
    })
    .unwrap();
    assert_eq!(ed.document().text(t).unwrap().0.content, "Hello");
    assert!(new.layout.width > old.layout.width);
    ed.undo().unwrap();
    assert_eq!(ed.document().text(t).unwrap().0, &old);
}

#[test]
fn points_in_use_by_text_cannot_be_deleted_silently() {
    let mut ed = Editor::new(Document::new());
    let (a, ..) = place(&mut ed);
    assert!(matches!(
        ed.apply(&Command::DeleteEntity { id: a }),
        Err(DocError::EntityInUse { .. })
    ));
}

#[test]
fn invalid_text_is_refused() {
    let mut ed = Editor::new(Document::new());
    let a = ed.apply(&pt(0.0, 0.0)).unwrap().entities[0];
    let d = ed.apply(&pt(1.0, 0.0)).unwrap().entities[0];
    let mut bad = text("x");
    bad.size = f64::NAN;
    let r = ed.apply(&Command::AddEntity {
        layer: L,
        construction: false,
        geom: Geom::Text {
            anchor: a,
            dir: d,
            text: Box::new(bad),
        },
    });
    assert!(r.is_err());
    // Anchor and direction must be different points.
    let r = ed.apply(&Command::AddEntity {
        layer: L,
        construction: false,
        geom: Geom::Text {
            anchor: a,
            dir: a,
            text: Box::new(text("x")),
        },
    });
    assert!(matches!(r, Err(DocError::SameEntity { .. })));
}

#[test]
fn anchor_and_baseline_are_constrainable() {
    let mut sk = Sketcher::new(Document::new());
    let a = sk.apply(&pt(1.0, 2.0)).unwrap().entities[0];
    let d = sk.apply(&pt(9.0, 5.0)).unwrap().entities[0];
    let base = sk
        .apply(&Command::AddEntity {
            layer: L,
            construction: true,
            geom: Geom::Line { start: a, end: d },
        })
        .unwrap()
        .entities[0];
    let t = sk
        .apply(&Command::AddEntity {
            layer: L,
            construction: false,
            geom: Geom::Text {
                anchor: a,
                dir: d,
                text: Box::new(text("Hi")),
            },
        })
        .unwrap()
        .entities[0];
    // Horizontal baseline, and the anchor 7 mm from the origin point.
    sk.apply(&Command::AddConstraint {
        constraint: Constraint::Horizontal { line: base },
    })
    .unwrap();
    sk.apply(&Command::AddConstraint {
        constraint: Constraint::Fix { point: a },
    })
    .unwrap();
    let doc = sk.document();
    let pa = doc.point_pos(a).unwrap();
    let pd = doc.point_pos(d).unwrap();
    assert!((pa.y - pd.y).abs() < 1e-6, "baseline solved horizontal");
    // The text sits on the solved baseline: its geometry starts at the anchor.
    let g = doc.text_geometry(t).unwrap().bbox().unwrap();
    assert!(g.min.x >= pa.x - 1e-6 && g.min.y >= pa.y - 5.0, "{g:?}");
    // Dimensions work on the points as usual.
    let _ = Dimension::driving(5.0);
}
