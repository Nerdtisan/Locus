// SPDX-License-Identifier: AGPL-3.0-or-later

//! Associative recipes: mirror (SPEC §6.2.1, #166, #167, ADR 0011).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_doc::{
    Command, Constraint, DocError, Document, EntityId, Geom, RecipeId, RecipeKind, Sketcher, file,
};
use locus_geom::Vec2;

const L: locus_doc::LayerId = Document::default_layer();

fn add(s: &mut Sketcher, geom: Geom) -> EntityId {
    s.apply(&Command::AddEntity {
        layer: L,
        construction: false,
        style: Default::default(),
        geom,
    })
    .unwrap()
    .entities[0]
}

fn pt(s: &mut Sketcher, x: f64, y: f64) -> EntityId {
    add(
        s,
        Geom::Point {
            pos: Vec2::new(x, y),
        },
    )
}

struct Fx {
    s: Sketcher,
    axis: EntityId,
    line: EntityId,
    a: EntityId,
    b: EntityId,
    circle: EntityId,
}

/// A fixed vertical mirror line on x = 0, a line and a circle to its right.
fn fx() -> Fx {
    let mut s = Sketcher::new(Document::new());
    let p0 = pt(&mut s, 0.0, -10.0);
    let p1 = pt(&mut s, 0.0, 10.0);
    let axis = add(&mut s, Geom::Line { start: p0, end: p1 });
    for p in [p0, p1] {
        s.apply(&Command::AddConstraint {
            constraint: Constraint::Fix { point: p },
        })
        .unwrap();
    }
    let a = pt(&mut s, 3.0, 1.0);
    let b = pt(&mut s, 5.0, 2.0);
    let line = add(&mut s, Geom::Line { start: a, end: b });
    let c = pt(&mut s, 4.0, -4.0);
    let circle = add(
        &mut s,
        Geom::Circle {
            center: c,
            radius: 1.5,
        },
    );
    Fx {
        s,
        axis,
        line,
        a,
        b,
        circle,
    }
}

fn mirror(f: &mut Fx, sources: &[EntityId]) -> Result<RecipeId, DocError> {
    f.s.apply(&Command::AddRecipe {
        kind: RecipeKind::Mirror { axis: f.axis },
        sources: sources.to_vec(),
    })
    .map(|a| a.recipes[0])
}

fn pos(s: &Sketcher, id: EntityId) -> Vec2 {
    s.document().point_pos(id).unwrap()
}

fn copy_of(s: &Sketcher, rid: RecipeId, src: EntityId) -> EntityId {
    let r = s.document().recipe(rid).unwrap();
    let cl: Vec<_> = locus_doc::recipe::closure(s.document(), &r.sources)
        .into_iter()
        .collect();
    let i = cl.iter().position(|c| *c == src).unwrap();
    r.pool[0][i]
}

#[test]
fn mirror_creates_reflected_instances() {
    let mut f = fx();
    let rid = {
        let src = [f.line, f.circle];
        mirror(&mut f, &src)
    }
    .unwrap();
    let (ia, ib) = (copy_of(&f.s, rid, f.a), copy_of(&f.s, rid, f.b));
    assert!((pos(&f.s, ia) - Vec2::new(-3.0, 1.0)).length() < 1e-6);
    assert!((pos(&f.s, ib) - Vec2::new(-5.0, 2.0)).length() < 1e-6);
    let ic = copy_of(&f.s, rid, f.circle);
    match f.s.document().entity(ic).unwrap().geom {
        Geom::Circle { radius, center } => {
            assert!((radius - 1.5).abs() < 1e-6);
            assert!((pos(&f.s, center) - Vec2::new(-4.0, -4.0)).length() < 1e-6);
        }
        _ => panic!("circle copy"),
    }
    assert_eq!(f.s.document().driven_by(ia), Some(rid));
}

#[test]
fn editing_the_source_moves_the_copy() {
    let mut f = fx();
    let rid = {
        let src = [f.line];
        mirror(&mut f, &src)
    }
    .unwrap();
    let ib = copy_of(&f.s, rid, f.b);
    f.s.apply(&Command::AddConstraint {
        constraint: Constraint::Fix { point: f.a },
    })
    .unwrap();
    f.s.apply(&Command::AddConstraint {
        constraint: Constraint::Horizontal { line: f.line },
    })
    .unwrap();
    let y = pos(&f.s, f.b).y;
    assert!((y - 1.0).abs() < 1e-6, "horizontal pulls b to y = 1");
    assert!((pos(&f.s, ib).y - y).abs() < 1e-6);
    assert!((pos(&f.s, ib).x + pos(&f.s, f.b).x).abs() < 1e-6);
}

#[test]
fn undo_redo_restore_recipe_and_instances_exactly() {
    let mut f = fx();
    let before = f.s.document().clone();
    let rid = {
        let src = [f.line, f.circle];
        mirror(&mut f, &src)
    }
    .unwrap();
    let after = f.s.document().clone();
    assert!(!after.recipes().is_empty());
    f.s.undo().unwrap();
    assert!(f.s.document().recipes().is_empty());
    assert_eq!(f.s.document().entities(), before.entities());
    assert_eq!(f.s.document().constraints(), before.constraints());
    f.s.redo().unwrap();
    assert_eq!(*f.s.document(), after);
    // ADR 0011 ID policy: undo never hands the recipe's IDs to anything else.
    f.s.undo().unwrap();
    let next = f.s.document().next_id();
    assert!(next > rid.get(), "the counter never goes back");
    let e = pt(&mut f.s, 9.0, 9.0);
    assert!(e.get() >= next, "new IDs come after the undone recipe's");
}

#[test]
fn instances_are_driven() {
    let mut f = fx();
    let rid = {
        let src = [f.line];
        mirror(&mut f, &src)
    }
    .unwrap();
    let ia = copy_of(&f.s, rid, f.a);
    let il =
        f.s.document()
            .recipe(rid)
            .unwrap()
            .instances()
            .find(|e| matches!(f.s.document().entity(*e).unwrap().geom, Geom::Line { .. }))
            .unwrap();
    assert!(matches!(f.s.begin_drag(ia), Err(DocError::Driven { .. })));
    assert!(matches!(
        f.s.apply(&Command::ModifyEntity {
            id: ia,
            geom: Geom::Point {
                pos: Vec2::new(0.0, 0.0)
            }
        }),
        Err(DocError::Driven { .. })
    ));
    for id in [il, f.line, f.axis] {
        assert!(matches!(
            f.s.apply(&Command::DeleteEntity { id }),
            Err(DocError::RecipeInUse { .. })
        ));
    }
    let owned = f.s.document().recipe(rid).unwrap().constraints[0];
    assert!(matches!(
        f.s.apply(&Command::RemoveConstraint { id: owned }),
        Err(DocError::ConstraintOwned { .. })
    ));
}

#[test]
fn toggling_construction_on_the_source_follows() {
    let mut f = fx();
    let rid = {
        let src = [f.line];
        mirror(&mut f, &src)
    }
    .unwrap();
    f.s.apply(&Command::ToggleConstruction { id: f.line })
        .unwrap();
    let mut lines = 0;
    for e in f.s.document().recipe(rid).unwrap().instances() {
        let ent = f.s.document().entity(e).unwrap();
        if matches!(ent.geom, Geom::Line { .. }) {
            lines += 1;
            assert!(ent.construction);
        }
    }
    assert_eq!(lines, 1);
}

#[test]
fn explode_makes_plain_geometry_and_undo_brings_the_recipe_back() {
    let mut f = fx();
    let rid = {
        let src = [f.line];
        mirror(&mut f, &src)
    }
    .unwrap();
    let ia = copy_of(&f.s, rid, f.a);
    let with = f.s.document().clone();
    f.s.apply(&Command::ExplodeRecipe { id: rid }).unwrap();
    assert!(f.s.document().recipes().is_empty());
    assert!(f.s.document().driven_by(ia).is_none());
    assert!(f.s.document().entity(ia).is_some());
    f.s.begin_drag(ia).unwrap();
    f.s.cancel_drag();
    f.s.undo().unwrap();
    assert_eq!(*f.s.document(), with);
}

#[test]
fn remove_deletes_instances_and_ties() {
    let mut f = fx();
    let before = f.s.document().clone();
    let rid = {
        let src = [f.line];
        mirror(&mut f, &src)
    }
    .unwrap();
    f.s.apply(&Command::RemoveRecipe { id: rid }).unwrap();
    assert_eq!(f.s.document().entities(), before.entities());
    assert_eq!(f.s.document().constraints(), before.constraints());
    assert!(f.s.document().recipes().is_empty());
    assert!(
        f.s.document().next_id() > before.next_id(),
        "IDs stay spent"
    );
}

#[test]
fn bad_mirrors_are_refused() {
    let mut f = fx();
    assert!(mirror(&mut f, &[]).is_err());
    assert!(matches!(
        {
            let src = [f.axis];
            mirror(&mut f, &src)
        },
        Err(DocError::SameEntity { .. })
    ));
    let r = f.s.apply(&Command::AddRecipe {
        kind: RecipeKind::Mirror { axis: f.a },
        sources: vec![f.line],
    });
    assert!(matches!(r, Err(DocError::WrongKind { .. })));
    // Mirroring an instance is refused: instances are driven.
    let rid = {
        let src = [f.line];
        mirror(&mut f, &src)
    }
    .unwrap();
    let ia = copy_of(&f.s, rid, f.a);
    assert!(matches!(
        f.s.apply(&Command::AddRecipe {
            kind: RecipeKind::Mirror { axis: f.axis },
            sources: vec![ia],
        }),
        Err(DocError::Driven { .. })
    ));
}

#[test]
fn recipes_survive_save_and_load() {
    let mut f = fx();
    {
        let src = [f.line, f.circle];
        mirror(&mut f, &src)
    }
    .unwrap();
    let text = file::save(f.s.document()).unwrap();
    let back = file::load(&text).unwrap();
    assert_eq!(&back, f.s.document());
    assert_eq!(file::save(&back).unwrap(), text);
}

#[test]
fn hostile_recipe_files_are_rejected() {
    let mut f = fx();
    {
        let src = [f.line];
        mirror(&mut f, &src)
    }
    .unwrap();
    let text = file::save(f.s.document()).unwrap();
    // Point the recipe at an entity that does not exist.
    let bad = text.replacen("sources: [", "sources: [999999, ", 1);
    assert!(file::load(&bad).is_err());
}
