// SPDX-License-Identifier: AGPL-3.0-or-later
//! Selection state (design §7.3): selected and hovered entities, the
//! Béziers whose handles show, and region selection. UI state only, never
//! part of the document (SPEC §11).

use crate::hit::Selection;
use crate::views::canvas::RegionUi;
use locus_doc::EntityId;
use std::collections::BTreeSet;

#[derive(Default)]
pub struct SelectionState {
    pub(crate) items: Selection,
    pub(crate) hover: Option<EntityId>,
    /// Béziers whose handles are shown: selected, or with a selected point (#160).
    pub(crate) handles: BTreeSet<EntityId>,
    /// Region selection, hover and the boolean combine (#122, #124).
    pub(crate) regions: RegionUi,
}
