// SPDX-License-Identifier: AGPL-3.0-or-later

//! Safe auto-fixes on the export geometry (SPEC §8 Validation, #127).
//!
//! Fixes edit the [`ExportModel`] only. The document is never changed (§11);
//! anything that would need a document edit is left for the user to do.

use crate::overlap::{Overlap, overlap};
use crate::pipeline::{ExportModel, PathSeg, chain};
use locus_doc::{EntityId, LayerId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixKind {
    RemovedDegenerate,
    /// Overlapping segments merged so the curve is cut once.
    MergedDuplicate,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fix {
    pub kind: FixKind,
    pub layer: LayerId,
    /// For a merge: the surviving entity first, then the one folded into it.
    pub entities: Vec<EntityId>,
    pub message: String,
}

/// Merge duplicate/overlapping segments and drop degenerate ones, then
/// re-chain each touched layer. Returns what changed.
pub fn autofix(model: &mut ExportModel) -> Vec<Fix> {
    let mut fixes = Vec::new();
    for layer in &mut model.layers {
        for e in layer.degenerate.drain(..) {
            fixes.push(Fix {
                kind: FixKind::RemovedDegenerate,
                layer: layer.id,
                entities: vec![e],
                message: format!("layer \"{}\": removed a zero-length segment", layer.name),
            });
        }
        let mut segs: Vec<PathSeg> = layer
            .paths
            .iter()
            .flat_map(|p| p.segs.iter().copied())
            .collect();
        let before = fixes.len();
        merge_duplicates(&mut segs, layer.id, &layer.name, &mut fixes);
        if fixes.len() != before {
            for s in &mut segs {
                s.reversed = false;
            }
            layer.paths = chain(segs);
        }
    }
    fixes
}

fn merge_duplicates(segs: &mut Vec<PathSeg>, layer: LayerId, name: &str, fixes: &mut Vec<Fix>) {
    let mut note = |kept: EntityId, gone: EntityId| {
        fixes.push(Fix {
            kind: FixKind::MergedDuplicate,
            layer,
            entities: vec![kept, gone],
            message: format!("layer \"{name}\": merged overlapping segments"),
        });
    };
    let mut i = 0;
    while i < segs.len() {
        let mut j = i + 1;
        let mut drop_i = false;
        while j < segs.len() {
            match overlap(&segs[i].shape, &segs[j].shape) {
                Overlap::None => j += 1,
                Overlap::AContainsB => {
                    note(segs[i].entity, segs[j].entity);
                    segs.remove(j);
                }
                Overlap::BContainsA => {
                    note(segs[j].entity, segs[i].entity);
                    drop_i = true;
                    break;
                }
                Overlap::Partial(union) => {
                    note(segs[i].entity, segs[j].entity);
                    segs[i].shape = union;
                    segs.remove(j);
                    // The grown segment may now cover earlier non-matches.
                    j = i + 1;
                }
            }
        }
        if drop_i {
            segs.remove(i);
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::testutil::*;
    use crate::validate::{IssueKind, validate};
    use locus_doc::Operation;

    #[test]
    fn duplicates_and_degenerates_are_fixed() {
        let mut segs = rect(1, 0.0, 0.0, 10.0, 10.0);
        segs.push(seg(9, (10.0, 0.0), (0.0, 0.0))); // exact reverse of edge 1
        segs.push(seg(10, (0.0, 0.0), (6.0, 0.0))); // inside edge 1
        let mut l = layer(Operation::Cut, segs);
        l.degenerate.push(EntityId::new(77));
        let mut m = ExportModel { layers: vec![l] };
        assert!(validate(&m).has_errors());

        let fixes = autofix(&mut m);
        let count = |k| fixes.iter().filter(|f| f.kind == k).count();
        assert_eq!(count(FixKind::MergedDuplicate), 2);
        assert_eq!(count(FixKind::RemovedDegenerate), 1);
        let r = validate(&m);
        assert!(r.of_kind(IssueKind::DuplicateSegment).next().is_none());
        assert!(r.of_kind(IssueKind::Degenerate).next().is_none());
        assert_eq!(m.layers[0].paths.len(), 1);
        assert!(m.layers[0].paths[0].closed);
        assert_eq!(m.layers[0].paths[0].segs.len(), 4);
    }

    #[test]
    fn partial_overlap_merges_into_one_segment() {
        let segs = vec![
            seg(1, (0.0, 0.0), (6.0, 0.0)),
            seg(2, (4.0, 0.0), (10.0, 0.0)),
        ];
        let mut m = ExportModel {
            layers: vec![layer(Operation::Draw, segs)],
        };
        autofix(&mut m);
        let s = &m.layers[0].paths[0].segs;
        assert_eq!(s.len(), 1);
        assert!((s[0].start().distance(s[0].end()) - 10.0).abs() < 1e-9);
    }
}
