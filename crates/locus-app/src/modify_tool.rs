// SPDX-License-Identifier: AGPL-3.0-or-later
//! Modify tools (SPEC §6.2): click a curve to trim, extend or break it. The
//! plans come from `locus_doc::modify`; the canvas runs each as one command.

use locus_doc::modify::{self, ModifyError, Plan};
use locus_doc::{Document, EntityId};
use locus_geom::Vec2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modify {
    Trim,
    Extend,
    Break,
}

impl Modify {
    pub const ALL: [Modify; 3] = [Modify::Trim, Modify::Extend, Modify::Break];

    pub fn label(self) -> &'static str {
        match self {
            Modify::Trim => "Trim",
            Modify::Extend => "Extend",
            Modify::Break => "Break",
        }
    }

    pub fn prompt(self) -> &'static str {
        match self {
            Modify::Trim => "Trim: click the piece of a curve to remove (Esc to finish)",
            Modify::Extend => "Extend: click near the end of a line or arc (Esc to finish)",
            Modify::Break => "Break: click a curve to split it where others cross (Esc to finish)",
        }
    }

    /// The plan for a click on `id` at `at`.
    pub fn plan(self, doc: &Document, id: EntityId, at: Vec2) -> Result<Plan, ModifyError> {
        match self {
            Modify::Trim => modify::trim(doc, id, at),
            Modify::Extend => modify::extend(doc, id, at),
            Modify::Break => modify::split(doc, id, at),
        }
    }
}
