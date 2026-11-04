// SPDX-License-Identifier: AGPL-3.0-or-later

//! Structured errors for document operations. Nothing in this crate panics on
//! bad input (CLAUDE.md, untrusted input); callers get one of these.

use crate::entity::EntityKind;
use crate::ids::{ConstraintId, EntityId, LayerId};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum DocError {
    #[error("unknown entity {0}")]
    UnknownEntity(EntityId),
    #[error("unknown constraint {0}")]
    UnknownConstraint(ConstraintId),
    #[error("layer {layer} cannot be deleted: {} entity(ies) on other layers are built on it", .entities.len())]
    LayerInUse {
        layer: LayerId,
        entities: Vec<EntityId>,
    },
    #[error("layer {0} is locked")]
    LayerLocked(LayerId),
    #[error("unknown layer {0}")]
    UnknownLayer(LayerId),
    #[error("{what} on entity {entity}: expected {expected}, found {found:?}")]
    WrongKind {
        what: &'static str,
        entity: EntityId,
        expected: &'static str,
        found: EntityKind,
    },
    #[error("{what} needs two different entities, got {entity} twice")]
    SameEntity {
        what: &'static str,
        entity: EntityId,
    },
    #[error("{what} must be finite and in range, got {value}")]
    BadValue { what: &'static str, value: f64 },
    #[error("entity {id} is still used by {} other item(s)", .entities.len() + .constraints.len())]
    EntityInUse {
        id: EntityId,
        entities: Vec<EntityId>,
        constraints: Vec<ConstraintId>,
    },
    #[error("constraint {0} has no dimension")]
    NotADimension(ConstraintId),
    #[error("the ID space is exhausted")]
    IdSpaceExhausted,
    #[error("nothing to {0}")]
    NothingToDo(&'static str),
    #[error("command failed: {0}")]
    Invalid(String),
    /// The constraint can't hold together with these (SPEC §5 Solver UX: refuse
    /// it, highlight the set, offer it as a driven dimension).
    #[error("conflicts with {} existing constraint(s)", .constraints.len())]
    Conflict {
        constraints: Vec<ConstraintId>,
        entities: Vec<EntityId>,
    },
    /// The constraint is already implied by these: it would over-constrain the
    /// sketch without restricting anything (SPEC §5). Offer it as driven.
    #[error("already implied by {} existing constraint(s)", .constraints.len())]
    Redundant {
        constraints: Vec<ConstraintId>,
        entities: Vec<EntityId>,
    },
    /// The solver could not satisfy these after the change, so it was undone.
    #[error("the sketch cannot be solved with this change ({} constraint(s) unsatisfied)", .unsatisfied.len())]
    Unsolvable { unsatisfied: Vec<ConstraintId> },
    /// The document could not be turned into solver input.
    #[error("solver input rejected: {0}")]
    Solver(String),
}
