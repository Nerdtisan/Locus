// SPDX-License-Identifier: AGPL-3.0-or-later

//! One file per layer (SPEC §8, #291).
//!
//! [`split_layers`] cuts a prepared model into one model per layer that is
//! actually written. Collection already dropped export-disabled and Reference
//! layers; empty layers are skipped here. Every piece carries the whole
//! design's bounds as its [`ExportModel::frame`], so the files share an origin
//! and canvas and line up when imported together. Machine profiles and the
//! operation transforms were applied before the split (see
//! [`prepare_for`](crate::prepare_for)), so each file is what the single-file
//! export would hold for that layer.

use crate::pipeline::ExportModel;
use locus_doc::LayerId;
use std::collections::BTreeSet;

/// One layer's share of an export.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerFile {
    pub layer: LayerId,
    /// The layer's name, as written in the document.
    pub layer_name: String,
    pub model: ExportModel,
}

/// A model per non-empty layer, in export order, all framed by the bounds of
/// the whole model.
#[must_use]
pub fn split_layers(model: &ExportModel) -> Vec<LayerFile> {
    let frame = model.bounds();
    model
        .layers
        .iter()
        .filter(|l| l.paths.iter().any(|p| !p.segs.is_empty()))
        .map(|l| LayerFile {
            layer: l.id,
            layer_name: l.name.clone(),
            model: ExportModel {
                layers: vec![l.clone()],
                frame,
            },
        })
        .collect()
}

/// Longest layer part of a file name, in characters.
const MAX_PART: usize = 64;

/// Makes `s` safe in a file name on every desktop OS: letters, digits, space,
/// `.`, `_` and `-` stay, anything else (separators, `:*?"<>|`, controls)
/// becomes `_`. No leading or trailing dots or spaces (Windows), never empty,
/// never a reserved device name.
#[must_use]
pub fn safe_part(s: &str) -> String {
    let mapped: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(MAX_PART)
        .collect();
    let t = mapped.trim_matches(|c| c == '.' || c == ' ');
    let t = if t.is_empty() { "layer" } else { t };
    let stem = t.split('.').next().unwrap_or(t).to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.ends_with(|c: char| c.is_ascii_digit()));
    if reserved {
        format!("_{t}")
    } else {
        t.to_owned()
    }
}

/// `<document>-<layer>.<ext>` for each file, unique within the set (a clash,
/// case-insensitively, gets `-2`, `-3`, …).
#[must_use]
pub fn file_names(document: &str, files: &[LayerFile], ext: &str) -> Vec<String> {
    let doc = safe_part(document);
    let mut used: BTreeSet<String> = BTreeSet::new();
    files
        .iter()
        .map(|f| {
            let base = format!("{doc}-{}", safe_part(&f.layer_name));
            let mut name = format!("{base}.{ext}");
            let mut n = 2;
            while !used.insert(name.to_lowercase()) {
                name = format!("{base}-{n}.{ext}");
                n += 1;
            }
            name
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_characters_are_replaced() {
        assert_eq!(safe_part("Cut/outer: v2?"), "Cut_outer_ v2_");
        assert_eq!(safe_part("..\\evil"), "_evil");
        assert_eq!(safe_part("  "), "layer");
        assert_eq!(safe_part("CON"), "_CON");
        assert_eq!(safe_part("com1.txt"), "_com1.txt");
        assert_eq!(safe_part(&"x".repeat(200)).chars().count(), MAX_PART);
    }
}
