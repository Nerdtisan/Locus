// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-object style through the command API (SPEC §4, §11; #287).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{
    Command, DocError, Document, EntityId, Fill, Geom, LayerProps, RecipeKind, Sketcher, Stroke,
    Style, StylePatch, modify,
};
use locus_geom::Vec2;

const L: locus_doc::LayerId = Document::default_layer();

fn add(s: &mut Sketcher, geom: Geom, style: Style) -> EntityId {
    s.apply(&Command::AddEntity {
        layer: L,
        construction: false,
        geom,
        style,
    })
    .unwrap()
    .entities[0]
}

fn line(s: &mut Sketcher, a: (f64, f64), b: (f64, f64), style: Style) -> EntityId {
    let p = |s: &mut Sketcher, (x, y): (f64, f64)| {
        add(
            s,
            Geom::Point {
                pos: Vec2::new(x, y),
            },
            Style::default(),
        )
    };
    let (start, end) = (p(s, a), p(s, b));
    add(s, Geom::Line { start, end }, style)
}

fn style(s: &Sketcher, id: EntityId) -> Style {
    s.document().entity(id).unwrap().style
}

const RED: Stroke = Stroke::Colour([255, 0, 0]);
const BLUE: Stroke = Stroke::Colour([0, 0, 255]);

fn width(w: f64) -> StylePatch {
    StylePatch {
        width: Some(w),
        ..StylePatch::default()
    }
}

#[test]
fn new_entities_take_the_style_they_are_given() {
    let mut s = Sketcher::new(Document::new());
    let red = Style {
        stroke: RED,
        width: 0.25,
        ..Style::default()
    };
    let l = line(&mut s, (0.0, 0.0), (10.0, 0.0), red);
    assert_eq!(style(&s, l), red);
}

/// A multi-selection is one command and one undo step, and a mixed selection
/// keeps the fields the patch doesn't set (m5-design §2.1).
#[test]
fn a_multi_selection_is_one_undo_step_and_keeps_unset_fields() {
    let mut s = Sketcher::new(Document::new());
    let a = line(
        &mut s,
        (0.0, 0.0),
        (10.0, 0.0),
        Style {
            stroke: RED,
            ..Style::default()
        },
    );
    let b = line(
        &mut s,
        (0.0, 5.0),
        (10.0, 5.0),
        Style {
            stroke: BLUE,
            ..Style::default()
        },
    );
    let before = s.document().clone();
    s.apply(&Command::SetStyle {
        entities: vec![a, b],
        patch: width(0.8),
    })
    .unwrap();
    assert_eq!((style(&s, a).stroke, style(&s, a).width), (RED, 0.8));
    assert_eq!((style(&s, b).stroke, style(&s, b).width), (BLUE, 0.8));
    assert_eq!(s.editor().undo_label(), Some("Set style"));
    s.undo().unwrap();
    assert_eq!(s.document(), &before, "one undo restores both");
    assert!(s.editor().can_redo());
}

#[test]
fn bad_and_empty_patches_are_refused_without_change() {
    let mut s = Sketcher::new(Document::new());
    let a = line(&mut s, (0.0, 0.0), (10.0, 0.0), Style::default());
    let before = s.document().clone();
    for (patch, want_bad) in [
        (width(0.0), true),
        (
            StylePatch {
                pen: Some(0),
                ..StylePatch::default()
            },
            true,
        ),
        (
            StylePatch {
                opacity: Some(-0.1),
                ..StylePatch::default()
            },
            true,
        ),
        (StylePatch::default(), false),
    ] {
        let r = s.apply(&Command::SetStyle {
            entities: vec![a],
            patch,
        });
        if want_bad {
            assert!(matches!(r, Err(DocError::BadValue { .. })), "{r:?}");
        } else {
            assert!(matches!(r, Err(DocError::NothingToDo(_))), "{r:?}");
        }
        assert_eq!(s.document(), &before);
    }
    assert!(matches!(
        s.apply(&Command::SetStyle {
            entities: vec![EntityId::new(999)],
            patch: width(1.0),
        }),
        Err(DocError::UnknownEntity(_))
    ));
}

#[test]
fn a_locked_layer_refuses_style_changes() {
    let mut s = Sketcher::new(Document::new());
    let a = line(&mut s, (0.0, 0.0), (10.0, 0.0), Style::default());
    s.apply(&Command::SetLayerLocked {
        id: L,
        locked: true,
    })
    .unwrap();
    assert!(matches!(
        s.apply(&Command::SetStyle {
            entities: vec![a],
            patch: width(1.0),
        }),
        Err(DocError::LayerLocked(_))
    ));
    // Layer props never touch object style (SPEC §4: style is per object).
    s.apply(&Command::SetLayerLocked {
        id: L,
        locked: false,
    })
    .unwrap();
    s.apply(&Command::SetLayerProps {
        id: L,
        props: LayerProps {
            colour: Some([9, 9, 9]),
            ..Default::default()
        },
    })
    .unwrap();
    assert_eq!(style(&s, a), Style::default());
}

/// Trimming keeps the pieces' style (they are the same object, shortened).
#[test]
fn trimmed_pieces_keep_their_style() {
    let mut s = Sketcher::new(Document::new());
    let st = Style {
        stroke: RED,
        width: 0.6,
        pen: 4,
        fill: Fill::None,
        opacity: 0.5,
    };
    let a = line(&mut s, (0.0, 0.0), (10.0, 0.0), st);
    line(&mut s, (5.0, -5.0), (5.0, 5.0), Style::default());
    let plan = modify::trim(s.document(), a, Vec2::new(8.0, 0.0)).unwrap();
    s.apply(&plan.command).unwrap();
    let styled: Vec<Style> = s
        .document()
        .entities()
        .iter()
        .filter(|(_, e)| matches!(e.geom, Geom::Line { .. }))
        .map(|(_, e)| e.style)
        .filter(|x| *x == st)
        .collect();
    assert_eq!(styled.len(), 1, "the kept piece of the trimmed line");
}

/// Mirror copies follow their source's style, as they follow construction
/// (SPEC §6.2.1).
#[test]
fn mirror_copies_follow_their_source() {
    let mut s = Sketcher::new(Document::new());
    let axis = line(&mut s, (0.0, -10.0), (0.0, 10.0), Style::default());
    let src = line(&mut s, (3.0, 1.0), (5.0, 2.0), Style::default());
    let rid = s
        .apply(&Command::AddRecipe {
            kind: RecipeKind::Mirror { axis },
            sources: vec![src],
        })
        .unwrap()
        .recipes[0];
    s.apply(&Command::SetStyle {
        entities: vec![src],
        patch: StylePatch {
            stroke: Some(RED),
            ..StylePatch::default()
        },
    })
    .unwrap();
    let copies: Vec<EntityId> = s.document().recipe(rid).unwrap().instances().collect();
    let line_copy = copies
        .iter()
        .copied()
        .find(|c| matches!(s.document().entity(*c).unwrap().geom, Geom::Line { .. }))
        .unwrap();
    assert_eq!(style(&s, line_copy).stroke, RED);
}
