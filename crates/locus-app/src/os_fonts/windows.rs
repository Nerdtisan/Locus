// SPDX-License-Identifier: AGPL-3.0-or-later
//! Windows: DirectWrite's system font collection through `dwrote` (#477).
//!
//! The collection includes fonts registered for the session, which is how
//! Adobe Creative Cloud activates its fonts. Spike S4 (ADR 0019) listed all
//! 22 activated Adobe families (32 faces) of 226 families in 25–100 ms.
//! Bytes are read through DirectWrite's font-file stream: no path is asked
//! for, kept or logged, and the file is never copied.
//!
//! `dwrote` asserts on a few COM failures that only happen when DirectWrite
//! itself is broken. Listing runs on a worker thread (a panic there leaves
//! the folder scan), and loading is wrapped so a panic is a missing font.

use super::{Face, Locator, MAX_FONT_BYTES};
use dwrote::{FontCollection, FontSimulations};

pub(super) fn faces() -> Vec<Face> {
    let collection = FontCollection::system();
    let mut out = Vec::new();
    for i in 0..collection.get_font_family_count() {
        let Ok(family) = collection.font_family(i) else {
            continue;
        };
        let Ok(name) = family.family_name() else {
            continue;
        };
        for j in 0..family.get_font_count() {
            let Ok(font) = family.font(j) else { continue };
            // Bold and oblique made up from another face aren't fonts of
            // their own.
            if font.simulations() != FontSimulations::None {
                continue;
            }
            out.push(Face {
                family: name.clone(),
                style: font.face_name(),
                at: Locator::DirectWrite,
            });
        }
    }
    out
}

/// The bytes and collection index of `family`'s face `style`.
pub(super) fn load(family: &str, style: &str) -> Option<(Vec<u8>, u32)> {
    std::panic::catch_unwind(|| load_face(family, style))
        .ok()
        .flatten()
}

fn load_face(family: &str, style: &str) -> Option<(Vec<u8>, u32)> {
    let collection = FontCollection::system();
    let fam = collection.font_family_by_name(family).ok()??;
    let font = (0..fam.get_font_count())
        .filter_map(|j| fam.font(j).ok())
        .filter(|f| f.simulations() == FontSimulations::None)
        .find(|f| f.face_name() == style)?;
    let face = font.create_font_face();
    let files = face.files().ok()?;
    // A face in several files (Type 1) can't be outlined.
    let [file] = files.as_slice() else {
        return None;
    };
    let bytes = file.font_file_bytes().ok()?;
    if bytes.is_empty() || u64::try_from(bytes.len()).ok()? > MAX_FONT_BYTES {
        return None;
    }
    Some((bytes, face.get_index()))
}
