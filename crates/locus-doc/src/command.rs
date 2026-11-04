// SPDX-License-Identifier: AGPL-3.0-or-later

//! The serialisable command API with undo/redo (SPEC §11).
//!
//! Every document mutation is a [`Command`] applied through an [`Editor`]. The
//! GUI, CLI, plugins and MCP server are all clients of this one path;
//! [`Document`] has no public mutators.
//!
//! One user action = one command = one undo step. Multi-step operations use
//! [`Command::Compound`], which is atomic: either every step applies, or the
//! document is left untouched.
//!
//! Undo/redo replays recorded state changes (including the exact IDs that were
//! allocated), so redo and "undo a delete" restore the same IDs (ADR-0003).
//! The ID counter itself never goes backwards. Solver output is not command
//! state: re-solve after every apply, undo and redo.

use crate::constraint::Constraint;
use crate::document::Document;
use crate::entity::{Entity, EntityKind, Geom};
use crate::error::DocError;
use crate::ids::{ConstraintId, EntityId, LayerId};
use locus_geom::Vec2;
use serde::{Deserialize, Serialize};

/// A document mutation. Plain data: serialise it, log it, replay it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Create an entity. The document assigns the ID (see [`Applied`]).
    AddEntity {
        layer: LayerId,
        construction: bool,
        geom: Geom,
    },
    /// Replace an entity's geometry. The kind (point, line, …) must not change.
    ModifyEntity {
        id: EntityId,
        geom: Geom,
    },
    /// Delete an entity. Fails with [`DocError::EntityInUse`] while other
    /// entities or constraints still reference it: delete those first, in a
    /// [`Command::Compound`], so nothing is ever dropped silently (SPEC §4).
    DeleteEntity {
        id: EntityId,
    },
    ToggleConstruction {
        id: EntityId,
    },
    AddConstraint {
        constraint: Constraint,
    },
    RemoveConstraint {
        id: ConstraintId,
    },
    /// Change the value of a dimensional constraint.
    SetDimension {
        id: ConstraintId,
        value: f64,
    },
    /// Switch a dimensional constraint between driving and driven (reference).
    SetDriven {
        id: ConstraintId,
        driven: bool,
    },
    /// Several commands as one labelled, atomic, single-step undo.
    Compound {
        label: String,
        commands: Vec<Command>,
    },
    /// Write solver output back into the stored positions (SPEC §10). Derived
    /// state: **not** recorded in the undo history and does not clear redo.
    ApplySolution {
        points: Vec<(EntityId, Vec2)>,
        radii: Vec<(EntityId, f64)>,
    },
}

impl Command {
    /// Undo-menu label.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Command::AddEntity { .. } => "Add entity".to_owned(),
            Command::ModifyEntity { .. } => "Modify entity".to_owned(),
            Command::DeleteEntity { .. } => "Delete entity".to_owned(),
            Command::ToggleConstruction { .. } => "Toggle construction".to_owned(),
            Command::AddConstraint { constraint } => {
                format!("Add {} constraint", constraint.name())
            }
            Command::RemoveConstraint { .. } => "Remove constraint".to_owned(),
            Command::SetDimension { .. } => "Set dimension".to_owned(),
            Command::SetDriven { .. } => "Set driven".to_owned(),
            Command::Compound { label, .. } => label.clone(),
            Command::ApplySolution { .. } => "Apply solution".to_owned(),
        }
    }
}

/// What a successful command created, in creation order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    pub entities: Vec<EntityId>,
    pub constraints: Vec<ConstraintId>,
}

/// One reversible state change: the state of an item before and after.
/// `None` means "absent".
#[derive(Debug, Clone)]
enum Patch {
    Entity {
        id: EntityId,
        before: Option<Entity>,
        after: Option<Entity>,
    },
    Constraint {
        id: ConstraintId,
        before: Option<Constraint>,
        after: Option<Constraint>,
    },
}

impl Patch {
    fn set(&self, doc: &mut Document, forward: bool) {
        match self {
            Patch::Entity { id, before, after } => {
                match if forward { after } else { before }.clone() {
                    Some(e) => doc.entities.insert(*id, e),
                    None => doc.entities.remove(*id),
                };
            }
            Patch::Constraint { id, before, after } => {
                match if forward { after } else { before }.clone() {
                    Some(c) => doc.constraints.insert(*id, c),
                    None => doc.constraints.remove(id),
                };
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Transaction {
    label: String,
    patches: Vec<Patch>,
}

/// Apply `cmd`, recording reversible changes in `patches`. On error the caller
/// rolls `patches` back.
fn exec(
    doc: &mut Document,
    cmd: &Command,
    patches: &mut Vec<Patch>,
    out: &mut Applied,
) -> Result<(), DocError> {
    match cmd {
        Command::AddEntity {
            layer,
            construction,
            geom,
        } => {
            let e = Entity {
                layer: *layer,
                construction: *construction,
                geom: geom.clone(),
            };
            doc.check_entity(&e)?;
            let id = EntityId::new(doc.alloc_id()?);
            record(
                doc,
                patches,
                Patch::Entity {
                    id,
                    before: None,
                    after: Some(e),
                },
            );
            out.entities.push(id);
        }
        Command::ModifyEntity { id, geom } => {
            let before = doc.entity(*id).ok_or(DocError::UnknownEntity(*id))?.clone();
            if before.geom.kind() != geom.kind() {
                return Err(DocError::WrongKind {
                    what: "modify",
                    entity: *id,
                    expected: "same kind as the existing entity",
                    found: geom.kind(),
                });
            }
            let after = Entity {
                geom: geom.clone(),
                ..before.clone()
            };
            // A line may not be made to reference itself, etc.
            doc.check_entity(&after)?;
            if after.geom.point_refs().contains(id) {
                return Err(DocError::SameEntity {
                    what: "modify",
                    entity: *id,
                });
            }
            record(
                doc,
                patches,
                Patch::Entity {
                    id: *id,
                    before: Some(before),
                    after: Some(after),
                },
            );
        }
        Command::DeleteEntity { id } => {
            doc.check_removable(*id)?;
            let before = doc.entity(*id).cloned();
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
        Command::ToggleConstruction { id } => {
            let before = doc.entity(*id).ok_or(DocError::UnknownEntity(*id))?.clone();
            let after = Entity {
                construction: !before.construction,
                ..before.clone()
            };
            record(
                doc,
                patches,
                Patch::Entity {
                    id: *id,
                    before: Some(before),
                    after: Some(after),
                },
            );
        }
        Command::AddConstraint { constraint } => {
            constraint.validate(doc)?;
            let id = ConstraintId::new(doc.alloc_id()?);
            record(
                doc,
                patches,
                Patch::Constraint {
                    id,
                    before: None,
                    after: Some(constraint.clone()),
                },
            );
            out.constraints.push(id);
        }
        Command::RemoveConstraint { id } => {
            let before = doc
                .constraint(*id)
                .ok_or(DocError::UnknownConstraint(*id))?
                .clone();
            record(
                doc,
                patches,
                Patch::Constraint {
                    id: *id,
                    before: Some(before),
                    after: None,
                },
            );
        }
        Command::SetDimension { id, value } => {
            edit_dimension(doc, *id, patches, |d| d.value = *value)?;
        }
        Command::SetDriven { id, driven } => {
            edit_dimension(doc, *id, patches, |d| d.driven = *driven)?;
        }
        Command::Compound { commands, .. } => {
            for c in commands {
                exec(doc, c, patches, out)?;
            }
        }
        Command::ApplySolution { points, radii } => apply_solution(doc, points, radii)?,
    }
    Ok(())
}

fn record(doc: &mut Document, patches: &mut Vec<Patch>, p: Patch) {
    p.set(doc, true);
    patches.push(p);
}

fn edit_dimension(
    doc: &mut Document,
    id: ConstraintId,
    patches: &mut Vec<Patch>,
    f: impl FnOnce(&mut crate::constraint::Dimension),
) -> Result<(), DocError> {
    let before = doc
        .constraint(id)
        .ok_or(DocError::UnknownConstraint(id))?
        .clone();
    let mut after = before.clone();
    f(after.dimension_mut().ok_or(DocError::NotADimension(id))?);
    after.validate(doc)?;
    let p = Patch::Constraint {
        id,
        before: Some(before),
        after: Some(after),
    };
    record(doc, patches, p);
    Ok(())
}

/// Validate everything first, then write: a bad solution changes nothing.
fn apply_solution(
    doc: &mut Document,
    points: &[(EntityId, Vec2)],
    radii: &[(EntityId, f64)],
) -> Result<(), DocError> {
    for (id, p) in points {
        if doc.point_pos(*id).is_none() {
            return Err(DocError::UnknownEntity(*id));
        }
        if !p.is_finite() {
            return Err(DocError::BadValue {
                what: "solved position",
                value: if p.x.is_finite() { p.y } else { p.x },
            });
        }
    }
    for (id, r) in radii {
        match doc.entity(*id).map(|e| e.geom.kind()) {
            None => return Err(DocError::UnknownEntity(*id)),
            Some(EntityKind::Circle) => {}
            Some(found) => {
                return Err(DocError::WrongKind {
                    what: "solved radius",
                    entity: *id,
                    expected: "circle",
                    found,
                });
            }
        }
        if !(r.is_finite() && *r > 0.0) {
            return Err(DocError::BadValue {
                what: "solved radius",
                value: *r,
            });
        }
    }
    for (id, p) in points {
        if let Some(Entity {
            geom: Geom::Point { pos },
            ..
        }) = doc.entities.get_mut(*id)
        {
            *pos = *p;
        }
    }
    for (id, r) in radii {
        if let Some(Entity {
            geom: Geom::Circle { radius, .. },
            ..
        }) = doc.entities.get_mut(*id)
        {
            *radius = *r;
        }
    }
    Ok(())
}

/// A document plus its undo/redo history: the only way to change a document.
#[derive(Debug, Clone)]
pub struct Editor {
    doc: Document,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
}

impl Editor {
    #[must_use]
    pub fn new(doc: Document) -> Self {
        Self {
            doc,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    #[must_use]
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Give up the history and take the document.
    #[must_use]
    pub fn into_document(self) -> Document {
        self.doc
    }

    /// Apply a command as one undo step. Atomic: on error the document is
    /// exactly as it was and the history is unchanged.
    ///
    /// # Errors
    /// Whatever validation rejected the command.
    pub fn apply(&mut self, cmd: &Command) -> Result<Applied, DocError> {
        let mut patches = Vec::new();
        let mut out = Applied::default();
        if let Err(e) = exec(&mut self.doc, cmd, &mut patches, &mut out) {
            for p in patches.iter().rev() {
                p.set(&mut self.doc, false);
            }
            return Err(e);
        }
        if !patches.is_empty() {
            self.undo.push(Transaction {
                label: cmd.label(),
                patches,
            });
            self.redo.clear();
        }
        Ok(out)
    }

    /// Undo the last step, returning its label. Re-solve afterwards.
    ///
    /// # Errors
    /// [`DocError::NothingToDo`] when there is nothing to undo.
    pub fn undo(&mut self) -> Result<String, DocError> {
        let t = self.undo.pop().ok_or(DocError::NothingToDo("undo"))?;
        for p in t.patches.iter().rev() {
            p.set(&mut self.doc, false);
        }
        let label = t.label.clone();
        self.redo.push(t);
        Ok(label)
    }

    /// Redo the last undone step, returning its label. Re-solve afterwards.
    ///
    /// # Errors
    /// [`DocError::NothingToDo`] when there is nothing to redo.
    pub fn redo(&mut self) -> Result<String, DocError> {
        let t = self.redo.pop().ok_or(DocError::NothingToDo("redo"))?;
        for p in &t.patches {
            p.set(&mut self.doc, true);
        }
        let label = t.label.clone();
        self.undo.push(t);
        Ok(label)
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    #[must_use]
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|t| t.label.as_str())
    }

    #[must_use]
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|t| t.label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::Dimension;

    const L: LayerId = Document::default_layer();

    fn add_pt(ed: &mut Editor, x: f64, y: f64) -> EntityId {
        let r = ed
            .apply(&Command::AddEntity {
                layer: L,
                construction: false,
                geom: Geom::Point {
                    pos: Vec2::new(x, y),
                },
            })
            .unwrap();
        r.entities[0]
    }

    /// Content equality; the ID counter legitimately only moves forward.
    fn same_content(a: &Document, b: &Document) -> bool {
        let mut a = a.clone();
        a.next_id = b.next_id;
        a == *b
    }

    fn square(ed: &mut Editor) -> (Vec<EntityId>, EntityId) {
        let p: Vec<_> = [(0.0, 0.0), (4.0, 0.0), (4.0, 3.0)]
            .iter()
            .map(|(x, y)| add_pt(ed, *x, *y))
            .collect();
        let l = ed
            .apply(&Command::AddEntity {
                layer: L,
                construction: false,
                geom: Geom::Line {
                    start: p[0],
                    end: p[1],
                },
            })
            .unwrap()
            .entities[0];
        (p, l)
    }

    #[test]
    fn apply_undo_redo_restores_document_and_ids() {
        let mut ed = Editor::new(Document::new());
        let original = ed.document().clone();
        let a = add_pt(&mut ed, 1.0, 2.0);
        let after = ed.document().clone();
        assert_eq!(ed.undo().unwrap(), "Add entity");
        assert!(same_content(ed.document(), &original));
        assert!(
            ed.document().next_id() > original.next_id(),
            "counter never rewinds"
        );
        assert_eq!(ed.redo().unwrap(), "Add entity");
        assert_eq!(ed.document(), &after);
        assert!(
            ed.document().entity(a).is_some(),
            "redo restores the same ID"
        );
        // A fresh add after undo gets a new, higher ID.
        ed.undo().unwrap();
        let b = add_pt(&mut ed, 1.0, 2.0);
        assert!(b > a);
    }

    #[test]
    fn every_command_round_trips_through_undo() {
        let mut ed = Editor::new(Document::new());
        let (p, l) = square(&mut ed);
        let mut steps: Vec<Command> = vec![
            Command::ModifyEntity {
                id: p[2],
                geom: Geom::Point {
                    pos: Vec2::new(9.0, 9.0),
                },
            },
            Command::ToggleConstruction { id: l },
            Command::AddConstraint {
                constraint: Constraint::Horizontal { line: l },
            },
            Command::AddConstraint {
                constraint: Constraint::Distance {
                    a: p[0],
                    b: p[1],
                    dim: Dimension::driving(4.0),
                },
            },
        ];
        for s in &steps {
            let before = ed.document().clone();
            ed.apply(s).unwrap();
            assert!(!same_content(ed.document(), &before));
            ed.undo().unwrap();
            assert!(same_content(ed.document(), &before), "{s:?}");
            ed.redo().unwrap();
        }
        // Dimension edits and removals need the constraint IDs.
        let cids: Vec<_> = ed.document().constraints().keys().copied().collect();
        steps = vec![
            Command::SetDimension {
                id: cids[1],
                value: 7.0,
            },
            Command::SetDriven {
                id: cids[1],
                driven: true,
            },
            Command::RemoveConstraint { id: cids[0] },
        ];
        for s in &steps {
            let before = ed.document().clone();
            ed.apply(s).unwrap();
            ed.undo().unwrap();
            assert!(same_content(ed.document(), &before), "{s:?}");
            ed.redo().unwrap();
        }
        // Delete is only possible once nothing references the entity.
        let lone = add_pt(&mut ed, 50.0, 50.0);
        let before = ed.document().clone();
        ed.apply(&Command::DeleteEntity { id: lone }).unwrap();
        assert!(ed.document().entity(lone).is_none());
        ed.undo().unwrap();
        assert!(same_content(ed.document(), &before));
        assert!(
            ed.document().entity(lone).is_some(),
            "undo of delete restores the same ID"
        );
    }

    #[test]
    fn compound_is_one_labelled_atomic_step() {
        let mut ed = Editor::new(Document::new());
        let original = ed.document().clone();
        let cmd = Command::Compound {
            label: "Draw segment".to_owned(),
            commands: vec![
                Command::AddEntity {
                    layer: L,
                    construction: false,
                    geom: Geom::Point { pos: Vec2::ZERO },
                },
                Command::AddEntity {
                    layer: L,
                    construction: false,
                    geom: Geom::Point {
                        pos: Vec2::new(1.0, 0.0),
                    },
                },
            ],
        };
        let out = ed.apply(&cmd).unwrap();
        assert_eq!(out.entities.len(), 2);
        assert_eq!(ed.document().entities().len(), 2);
        assert_eq!(ed.undo_label(), Some("Draw segment"));
        ed.undo().unwrap();
        assert!(
            same_content(ed.document(), &original),
            "one undo removes both"
        );
        assert!(!ed.can_undo());
        ed.redo().unwrap();
        assert_eq!(ed.document().entities().len(), 2);
    }

    #[test]
    fn failing_compound_leaves_document_untouched() {
        let mut ed = Editor::new(Document::new());
        let a = add_pt(&mut ed, 0.0, 0.0);
        let before = ed.document().clone();
        let depth = ed.undo.len();
        let cmd = Command::Compound {
            label: "Doomed".to_owned(),
            commands: vec![
                Command::ToggleConstruction { id: a },
                Command::AddEntity {
                    layer: L,
                    construction: false,
                    geom: Geom::Point {
                        pos: Vec2::new(5.0, 5.0),
                    },
                },
                Command::DeleteEntity {
                    id: EntityId::new(9999),
                },
            ],
        };
        assert_eq!(
            ed.apply(&cmd),
            Err(DocError::UnknownEntity(EntityId::new(9999)))
        );
        assert!(same_content(ed.document(), &before));
        assert_eq!(ed.undo.len(), depth, "history unchanged");
    }

    #[test]
    fn delete_refuses_while_referenced_then_compound_deletes_all() {
        let mut ed = Editor::new(Document::new());
        let (p, l) = square(&mut ed);
        ed.apply(&Command::AddConstraint {
            constraint: Constraint::Horizontal { line: l },
        })
        .unwrap();
        let cid = *ed.document().constraints().keys().next().unwrap();
        let err = ed.apply(&Command::DeleteEntity { id: l }).unwrap_err();
        assert!(
            matches!(err, DocError::EntityInUse { ref constraints, .. } if constraints == &[cid])
        );
        let before = ed.document().clone();
        ed.apply(&Command::Compound {
            label: "Delete line".to_owned(),
            commands: vec![
                Command::RemoveConstraint { id: cid },
                Command::DeleteEntity { id: l },
                Command::DeleteEntity { id: p[0] },
                Command::DeleteEntity { id: p[1] },
            ],
        })
        .unwrap();
        assert!(ed.document().entity(l).is_none());
        ed.undo().unwrap();
        assert!(
            same_content(ed.document(), &before),
            "atomic restore incl. constraint"
        );
    }

    #[test]
    fn new_command_clears_redo() {
        let mut ed = Editor::new(Document::new());
        add_pt(&mut ed, 0.0, 0.0);
        ed.undo().unwrap();
        assert!(ed.can_redo());
        add_pt(&mut ed, 1.0, 1.0);
        assert!(!ed.can_redo());
        assert_eq!(ed.redo(), Err(DocError::NothingToDo("redo")));
    }

    #[test]
    fn rejected_commands() {
        let mut ed = Editor::new(Document::new());
        let (p, l) = square(&mut ed);
        let bad = [
            Command::ModifyEntity {
                id: l,
                geom: Geom::Point { pos: Vec2::ZERO },
            },
            Command::ModifyEntity {
                id: l,
                geom: Geom::Line {
                    start: l,
                    end: p[0],
                },
            },
            Command::ModifyEntity {
                id: p[0],
                geom: Geom::Point {
                    pos: Vec2::new(f64::NAN, 0.0),
                },
            },
            Command::AddEntity {
                layer: LayerId::new(42),
                construction: false,
                geom: Geom::Point { pos: Vec2::ZERO },
            },
            Command::SetDimension {
                id: ConstraintId::new(5),
                value: 1.0,
            },
            Command::AddConstraint {
                constraint: Constraint::Horizontal { line: p[0] },
            },
            Command::ToggleConstruction {
                id: EntityId::new(1234),
            },
        ];
        for c in bad {
            let before = ed.document().clone();
            assert!(ed.apply(&c).is_err(), "{c:?}");
            assert_eq!(ed.document(), &before);
        }
        // SetDimension on a non-dimensional constraint.
        ed.apply(&Command::AddConstraint {
            constraint: Constraint::Horizontal { line: l },
        })
        .unwrap();
        let cid = *ed.document().constraints().keys().next().unwrap();
        assert_eq!(
            ed.apply(&Command::SetDimension {
                id: cid,
                value: 1.0
            }),
            Err(DocError::NotADimension(cid))
        );
        // Nothing to undo on a fresh editor.
        assert_eq!(
            Editor::new(Document::new()).undo(),
            Err(DocError::NothingToDo("undo"))
        );
    }

    #[test]
    fn apply_solution_is_not_an_undo_step() {
        let mut ed = Editor::new(Document::new());
        let a = add_pt(&mut ed, 0.0, 0.0);
        let depth = ed.undo.len();
        ed.apply(&Command::ApplySolution {
            points: vec![(a, Vec2::new(3.0, 4.0))],
            radii: vec![],
        })
        .unwrap();
        assert_eq!(ed.document().point_pos(a), Some(Vec2::new(3.0, 4.0)));
        assert_eq!(ed.undo.len(), depth);
        let bad = Command::ApplySolution {
            points: vec![(a, Vec2::new(1.0, 1.0)), (a, Vec2::new(f64::INFINITY, 0.0))],
            radii: vec![],
        };
        assert!(ed.apply(&bad).is_err());
        assert_eq!(
            ed.document().point_pos(a),
            Some(Vec2::new(3.0, 4.0)),
            "all-or-nothing"
        );
    }

    #[test]
    fn commands_serialise() {
        let c = Command::Compound {
            label: "x".to_owned(),
            commands: vec![
                Command::AddConstraint {
                    constraint: Constraint::Fix {
                        point: EntityId::new(2),
                    },
                },
                Command::SetDimension {
                    id: ConstraintId::new(3),
                    value: 1.5,
                },
            ],
        };
        let text = ron::to_string(&c).unwrap();
        assert_eq!(ron::from_str::<Command>(&text).unwrap(), c);
    }
}
