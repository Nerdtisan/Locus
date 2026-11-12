//! Deleting a selection (SPEC §4 rule 5, §11).
//!
//! [`Command::DeleteEntity`] refuses while anything still references the
//! entity, so a user-level "delete" has to say what goes with it. This plans
//! that as one [`Command::Compound`] — a single undo step: the selected
//! entities, everything built on them (a deleted point takes its lines), the
//! constraints that mention any of them, and the points the removed curves
//! leave orphaned.

use crate::command::Command;
use crate::document::Document;
use crate::entity::EntityKind;
use crate::ids::{ConstraintId, EntityId};
use std::collections::BTreeSet;

/// Plans deleting `ids`; `None` when none of them exists. Unknown IDs are
/// ignored. Refusals that only the document can know (a locked layer, an
/// entity owned by a pattern recipe) surface when the command is applied.
#[must_use]
pub fn delete_entities(doc: &Document, ids: &[EntityId]) -> Option<Command> {
    let mut gone: BTreeSet<EntityId> = ids
        .iter()
        .copied()
        .filter(|i| doc.entity(*i).is_some())
        .collect();
    if gone.is_empty() {
        return None;
    }
    // Everything built on a doomed entity goes too.
    let mut queue: Vec<EntityId> = gone.iter().copied().collect();
    while let Some(id) = queue.pop() {
        for dep in doc.dependents(id).0 {
            if gone.insert(dep) {
                queue.push(dep);
            }
        }
    }
    // Points only the doomed curves used would be left dangling.
    let curve_points: BTreeSet<EntityId> = gone
        .iter()
        .filter_map(|i| doc.entity(*i))
        .flat_map(|e| e.geom.point_refs())
        .collect();
    for p in curve_points {
        if !gone.contains(&p) && doc.dependents(p).0.iter().all(|e| gone.contains(e)) {
            gone.insert(p);
        }
    }
    let mut constraints: BTreeSet<ConstraintId> = BTreeSet::new();
    for id in &gone {
        constraints.extend(doc.dependents(*id).1);
    }
    // Curves first, points last: a point is removable once its curves are.
    let (points, curves): (Vec<EntityId>, Vec<EntityId>) = gone
        .iter()
        .partition(|i| doc.entity(**i).map(|e| e.geom.kind()) == Some(EntityKind::Point));
    let commands = constraints
        .into_iter()
        .map(|id| Command::RemoveConstraint { id })
        .chain(
            curves
                .into_iter()
                .chain(points)
                .map(|id| Command::DeleteEntity { id }),
        )
        .collect();
    Some(Command::Compound {
        label: "Delete".to_owned(),
        commands,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::command::Editor;
    use crate::constraint::Constraint;
    use crate::entity::Geom;
    use locus_geom::Vec2;

    fn add(ed: &mut Editor, geom: Geom) -> EntityId {
        ed.apply(&Command::AddEntity {
            layer: Document::default_layer(),
            construction: false,
            geom,
        })
        .unwrap()
        .entities[0]
    }

    /// Two lines sharing the middle point, the first horizontal.
    fn polyline(ed: &mut Editor) -> ([EntityId; 3], [EntityId; 2]) {
        let p = [(0.0, 0.0), (4.0, 0.0), (4.0, 3.0)].map(|(x, y)| {
            add(
                ed,
                Geom::Point {
                    pos: Vec2::new(x, y),
                },
            )
        });
        let l0 = add(
            ed,
            Geom::Line {
                start: p[0],
                end: p[1],
            },
        );
        let l1 = add(
            ed,
            Geom::Line {
                start: p[1],
                end: p[2],
            },
        );
        ed.apply(&Command::AddConstraint {
            constraint: Constraint::Horizontal { line: l0 },
        })
        .unwrap();
        (p, [l0, l1])
    }

    #[test]
    fn deleting_a_line_takes_its_constraints_and_private_points_in_one_step() {
        let mut ed = Editor::new(Document::new());
        let (p, l) = polyline(&mut ed);
        let before = ed.document().clone();
        let cmd = delete_entities(ed.document(), &[l[0]]).unwrap();
        ed.apply(&cmd).unwrap();
        assert!(ed.document().entity(l[0]).is_none());
        assert!(ed.document().constraints().is_empty());
        assert!(ed.document().entity(p[0]).is_none(), "private endpoint");
        assert!(
            ed.document().entity(p[1]).is_some(),
            "shared endpoint stays"
        );
        assert!(ed.document().entity(l[1]).is_some());
        // A single undo restores everything: it was one step.
        ed.undo().unwrap();
        let mut now = ed.document().clone();
        now.next_id = before.next_id;
        assert_eq!(now, before);
    }

    #[test]
    fn deleting_a_shared_point_takes_both_lines() {
        let mut ed = Editor::new(Document::new());
        let (p, l) = polyline(&mut ed);
        let cmd = delete_entities(ed.document(), &[p[1]]).unwrap();
        ed.apply(&cmd).unwrap();
        for id in [p[0], p[1], p[2], l[0], l[1]] {
            assert!(ed.document().entity(id).is_none());
        }
        assert!(ed.document().constraints().is_empty());
    }

    #[test]
    fn nothing_to_delete_is_none() {
        let doc = Document::new();
        assert!(delete_entities(&doc, &[]).is_none());
        assert!(delete_entities(&doc, &[EntityId::new(42)]).is_none());
    }
}
