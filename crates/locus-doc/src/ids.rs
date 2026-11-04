// SPDX-License-Identifier: AGPL-3.0-or-later

//! Document-scoped identifiers (ADR-0003, SPEC §4).
//!
//! One monotonic counter serves every kind of ID in a document; each kind has
//! its own newtype so they cannot be mixed up. IDs are never reused.

use core::fmt;
use serde::{Deserialize, Serialize};

/// IDs stay below 2^53 so they are exact in JSON numbers on JS clients
/// (ADR-0003 rule 5).
pub const MAX_ID: u64 = 1 << 53;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            #[must_use]
            pub const fn new(raw: u64) -> Self {
                Self(raw)
            }

            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_type!(
    /// Identifies a sketch entity (point, line, circle, arc).
    EntityId
);
id_type!(
    /// Identifies a constraint.
    ConstraintId
);
id_type!(
    /// Identifies a layer.
    LayerId
);
