// SPDX-License-Identifier: AGPL-3.0-or-later
//! Deleting a selection (SPEC §4 rule 5, §11).
//!
//! [`Command::DeleteEntity`] refuses while anything still references the
//! entity, so a user-level "delete" has to say what goes with it. This plans
//! that as one [`Command::Compound`] — a single undo step: the selected
//! entities, everything built on them (a deleted point takes its lines), the
//! constraints that mention any of them, and the points the removed curves
//! leave orphaned.
//!
//! Recipes go with them (#365): deleting a recipe's output (an offset, a
//! pattern copy) removes that recipe, and deleting a source removes the
//! recipes built on it, and any recipe built on their output in turn.

use crate::command::Command;
use crate::document::Document;
use crate::entity::EntityKind;
use crate::ids::{ConstraintId, EntityId, RecipeId};
use std::collections::BTreeSet;

/// A planned delete: the one-step command, and the recipes it removes that
/// were picked only through a source (the UI asks before those, #365).
#[derive(Debug, Clone, PartialEq)]
pub struct DeletePlan {
    pub command: Command,
    /// Recipes removed because something they are built on goes.
    pub dependent_recipes: Vec<RecipeId>,
}

/// Plans deleting `ids`; `None` when none of them exists. Unknown IDs are
/// ignored. Refusals that only the document can know (a locked layer, an
/// entity owned by a pattern recipe) surface when the command is applied.
#[must_use]
pub fn delete_entities(doc: &Document, ids: &[EntityId]) -> Option<Command> {
    delete_plan(doc, ids).map(|p| p.command)
}

/// [`delete_entities`] with the recipes it takes along (#365).
#[must_use]
pub fn delete_plan(doc: &Document, ids: &[EntityId]) -> Option<DeletePlan> {
    let mut gone: BTreeSet<EntityId> = ids
        .iter()
        .copied()
        .filter(|i| doc.entity(*i).is_some())
        .collect();
    if gone.is_empty() {
        return None;
    }
    // A picked recipe output means "remove that recipe".
    let picked_recipes: BTreeSet<RecipeId> =
        gone.iter().filter_map(|i| doc.driven_by(*i)).collect();
    let mut recipes: BTreeSet<RecipeId> = BTreeSet::new();
    // Everything built on a doomed entity goes too: entities that use it,
    // and recipes that use it or own it, whose outputs then go as well.
    let mut queue: Vec<EntityId> = gone.iter().copied().collect();
    while let Some(id) = queue.pop() {
        for dep in doc.dependents(id).0 {
            if gone.insert(dep) {
                queue.push(dep);
            }
        }
        for rid in doc.recipes_involving(id) {
            if recipes.insert(rid)
                && let Some(r) = doc.recipe(rid)
            {
                for inst in r.instances() {
                    if gone.insert(inst) {
                        queue.push(inst);
                    }
                }
            }
        }
    }
    // A removed recipe's outputs go with the recipe, not one by one.
    let owned: BTreeSet<EntityId> = recipes
        .iter()
        .filter_map(|r| doc.recipe(*r))
        .flat_map(|r| r.instances().collect::<Vec<_>>())
        .collect();
    let recipe_constraints: BTreeSet<ConstraintId> = recipes
        .iter()
        .filter_map(|r| doc.recipe(*r))
        .flat_map(|r| r.constraints.iter().copied())
        .collect();
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
    constraints.retain(|c| !recipe_constraints.contains(c));
    // Recipes built on another's output first, so each removal is allowed.
    let mut order: Vec<RecipeId> = Vec::new();
    let mut left: Vec<RecipeId> = recipes.iter().copied().collect();
    while !left.is_empty() {
        let before = left.len();
        left.retain(|rid| {
            let outs: Vec<EntityId> = doc
                .recipe(*rid)
                .map(|r| r.instances().collect())
                .unwrap_or_default();
            let used_by_left = recipes.iter().any(|other| {
                other != rid
                    && !order.contains(other)
                    && doc
                        .recipe(*other)
                        .is_some_and(|o| outs.iter().any(|i| o.inputs(doc).contains(i)))
            });
            if used_by_left {
                true
            } else {
                order.push(*rid);
                false
            }
        });
        if left.len() == before {
            // A cycle can't happen; keep going rather than loop.
            order.append(&mut left);
        }
    }
    // Curves first, points last: a point is removable once its curves are.
    let (points, curves): (Vec<EntityId>, Vec<EntityId>) = gone
        .iter()
        .filter(|i| !owned.contains(i))
        .partition(|i| doc.entity(**i).map(|e| e.geom.kind()) == Some(EntityKind::Point));
    let commands = constraints
        .into_iter()
        .map(|id| Command::RemoveConstraint { id })
        .chain(order.iter().map(|id| Command::RemoveRecipe { id: *id }))
        .chain(
            curves
                .into_iter()
                .chain(points)
                .map(|id| Command::DeleteEntity { id }),
        )
        .collect();
    Some(DeletePlan {
        command: Command::Compound {
            label: "Delete".to_owned(),
            commands,
        },
        dependent_recipes: order
            .into_iter()
            .filter(|r| !picked_recipes.contains(r))
            .collect(),
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
            style: Default::default(),
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

    /// #365: deleting an offset's output removes the offset; deleting its
    /// source removes the source and the offset (and a pattern of the
    /// offset) in one undo step.
    #[test]
    fn deleting_an_offset_or_its_source_takes_the_recipes() {
        use crate::recipe::{Axis, LinearDir, RecipeKind};
        use crate::solve::Sketcher;
        let build = || {
            let mut ed = Editor::new(Document::new());
            let (_, lines) = polyline(&mut ed);
            let mut sk = Sketcher::new(ed.document().clone());
            let off = sk
                .apply(&Command::AddRecipe {
                    kind: RecipeKind::Offset {
                        distance: 2.0,
                        sharp: true,
                        expr: None,
                    },
                    sources: lines.to_vec(),
                })
                .unwrap()
                .recipes[0];
            (sk, lines, off)
        };
        // The output: only the offset goes.
        let (mut sk, lines, off) = build();
        let out: EntityId = sk
            .document()
            .recipe(off)
            .unwrap()
            .instances()
            .next()
            .unwrap();
        let plan = delete_plan(sk.document(), &[out]).unwrap();
        assert!(plan.dependent_recipes.is_empty(), "the user picked it");
        sk.apply(&plan.command).unwrap();
        assert!(sk.document().recipes().is_empty());
        assert!(lines.iter().all(|l| sk.document().entity(*l).is_some()));
        // The source, with a pattern of the offset on top: everything goes.
        let (mut sk, lines, off) = build();
        let outs: Vec<EntityId> = sk.document().recipe(off).unwrap().instances().collect();
        sk.apply(&Command::AddRecipe {
            kind: RecipeKind::Linear {
                dir1: LinearDir {
                    axis: Axis::X,
                    count: "2".to_owned(),
                    spacing: "50".to_owned(),
                    symmetric: false,
                },
                dir2: None,
            },
            sources: outs,
        })
        .unwrap();
        let plan = delete_plan(sk.document(), &[lines[0]]).unwrap();
        assert_eq!(
            plan.dependent_recipes.len(),
            2,
            "the offset and the pattern"
        );
        sk.apply(&plan.command).unwrap();
        assert!(sk.document().recipes().is_empty());
        assert!(sk.document().entity(lines[0]).is_none());
        // One undo step brings it all back.
        sk.undo().unwrap();
        assert_eq!(sk.document().recipes().len(), 2);
    }

    #[test]
    fn nothing_to_delete_is_none() {
        let doc = Document::new();
        assert!(delete_entities(&doc, &[]).is_none());
        assert!(delete_entities(&doc, &[EntityId::new(42)]).is_none());
    }
}
