// SPDX-License-Identifier: AGPL-3.0-or-later

//! Linear and circular patterns (SPEC §6.2.1, #168, #169, ADR 0011).
//!
//! The copies are ordinary entities with the recipe's own IDs (see the ID
//! policy in `crate::recipe`) but they are not solver variables: after every
//! command and solve, [`regenerate`] re-evaluates the pattern's expressions,
//! creates or removes copies to match the count, and writes every copy's
//! positions from its sources' solved positions, in the same undo step.

use crate::command::{Patch, record};
use crate::document::Document;
use crate::entity::{Entity, EntityKind};
use crate::error::DocError;
use crate::ids::{EntityId, RecipeId};
use crate::recipe::{self, MAX_INSTANCE_ENTITIES, MAX_INSTANCES, Recipe};

/// Bring recipe `rid`'s copies in line with the document: right number of
/// copies, right IDs, right positions. A pattern whose expressions don't
/// evaluate right now (a source was deleted, say) is left as it is; the
/// command that caused it is refused elsewhere.
pub(crate) fn regenerate(
    doc: &mut Document,
    rid: RecipeId,
    patches: &mut Vec<Patch>,
) -> Result<(), DocError> {
    let Some(r) = doc.recipe(rid).cloned() else {
        return Ok(());
    };
    if !r.kind.is_pattern() {
        return Ok(());
    }
    let Ok(slots) = recipe::slots(doc, &r.kind, &r.sources) else {
        return Ok(());
    };
    let cl: Vec<EntityId> = recipe::closure(doc, &r.sources).into_iter().collect();
    let n = slots.len();
    if n > MAX_INSTANCES || n.saturating_mul(cl.len()) > MAX_INSTANCE_ENTITIES {
        return Err(DocError::InstanceLimit {
            requested: n,
            limit: MAX_INSTANCES,
        });
    }
    let mut next = r.clone();
    // The recipe keeps every ID it ever allocated; only grow the pool.
    while next.pool.len() < n {
        let mut set = Vec::with_capacity(cl.len());
        for _ in &cl {
            set.push(EntityId::new(doc.alloc_id()?));
        }
        next.pool.push(set);
    }
    next.active = n;
    next.suppressed.retain(|k| *k < n);
    // Points before the shapes that refer to them.
    let mut order: Vec<usize> = (0..cl.len()).collect();
    order.sort_by_key(|i| {
        doc.entity(cl[*i])
            .map_or(1, |e| u8::from(e.geom.kind() != EntityKind::Point))
    });
    #[allow(clippy::needless_range_loop)]
    for k in 0..next.pool.len().max(r.active) {
        let set = &next.pool[k];
        let want = next.is_live(k);
        let have = set.first().is_some_and(|id| doc.entity(*id).is_some());
        if !want {
            if have {
                remove_copy(doc, set, patches)?;
            }
            continue;
        }
        let Some(xf) = slots.get(k).copied() else {
            continue;
        };
        let map = |id: EntityId| {
            cl.iter()
                .position(|c| *c == id)
                .and_then(|i| set.get(i).copied())
                .unwrap_or(id)
        };
        for i in &order {
            let Some(src) = doc.entity(cl[*i]).cloned() else {
                continue;
            };
            let id = set[*i];
            // A copy keeps a style it was given; a new copy takes its source's.
            let style = doc.entity(id).map_or(src.style, |e| e.style);
            let e = Entity {
                layer: src.layer,
                construction: src.construction,
                geom: recipe::copy_geom(&src.geom, xf, &map),
                style,
            };
            if doc.entity(id) != Some(&e) {
                let before = doc.entity(id).cloned();
                record(
                    doc,
                    patches,
                    Patch::Entity {
                        id,
                        before,
                        after: Some(e),
                    },
                );
            }
        }
    }
    if next != r {
        record(
            doc,
            patches,
            Patch::Recipe {
                id: rid,
                before: Some(r),
                after: Some(next),
            },
        );
    }
    Ok(())
}

/// Delete one copy, unless something outside it still refers to it.
fn remove_copy(
    doc: &mut Document,
    set: &[EntityId],
    patches: &mut Vec<Patch>,
) -> Result<(), DocError> {
    for id in set {
        let (ents, cons) = doc.dependents(*id);
        let ents: Vec<_> = ents.into_iter().filter(|e| !set.contains(e)).collect();
        if !ents.is_empty() || !cons.is_empty() {
            return Err(DocError::EntityInUse {
                id: *id,
                entities: ents,
                constraints: cons,
            });
        }
    }
    for id in set {
        let before = doc.entity(*id).cloned();
        if before.is_some() {
            record(
                doc,
                patches,
                Patch::Entity {
                    id: *id,
                    before,
                    after: None,
                },
            );
        }
    }
    Ok(())
}

/// A fresh, empty pattern recipe (copies are created by [`regenerate`]).
#[must_use]
pub(crate) fn empty(kind: crate::recipe::RecipeKind, sources: Vec<EntityId>) -> Recipe {
    Recipe {
        kind,
        sources,
        pool: Vec::new(),
        active: 0,
        suppressed: std::collections::BTreeSet::new(),
        constraints: Vec::new(),
    }
}
