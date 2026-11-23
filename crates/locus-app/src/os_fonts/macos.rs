// SPDX-License-Identifier: AGPL-3.0-or-later
//! macOS: Core Text's available fonts through `core-text` (#477).
//!
//! `CTFontCollectionCreateFromAvailableFonts` includes fonts registered for
//! the session, which is how Adobe Creative Cloud activates its fonts. Each
//! face's file comes from Core Text's font URL; the face in a collection
//! (`.ttc`) is found again by its names when it loads. Paths stay in memory
//! and are never logged.

use super::{Face, Locator};

pub(super) fn faces() -> Vec<Face> {
    let collection = core_text::font_collection::create_for_all_families();
    let Some(descriptors) = collection.get_descriptors() else {
        return Vec::new();
    };
    descriptors
        .iter()
        .filter_map(|d| {
            let path = d.font_path()?;
            Some(Face {
                family: d.family_name(),
                style: d.style_name(),
                at: Locator::File { path, index: None },
            })
        })
        .collect()
}
