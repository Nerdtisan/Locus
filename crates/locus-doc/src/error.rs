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
}
