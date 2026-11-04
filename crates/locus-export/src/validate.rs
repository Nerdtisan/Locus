// SPDX-License-Identifier: AGPL-3.0-or-later

//! Export validation with a structured report (SPEC §8 Validation, #126).
//!
//! Pure function of the [`ExportModel`]; the document is never touched (§11).
//! Each issue carries the entity IDs a UI should highlight.

use crate::overlap::{Overlap, overlap};
use crate::pipeline::{ExportLayer, ExportModel, PathSeg};
use locus_doc::{EntityId, LayerId, Operation};
use locus_geom::{EXPORT_TOL, Vec2};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Blocks export until fixed or overridden.
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    /// A path on a Cut layer does not close.
    OpenPath,
    SelfIntersection,
    /// Two segments cover the same curve: the machine would cut it twice.
    DuplicateSegment,
    /// Zero-length segment; dropped from the export.
    Degenerate,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    pub kind: IssueKind,
    pub layer: LayerId,
    /// Entities to highlight.
    pub entities: Vec<EntityId>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Counts {
    pub errors: usize,
    pub warnings: usize,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Report {
    pub issues: Vec<Issue>,
}

impl Report {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.issues.iter().any(|i| i.severity == Severity::Error)
    }

    #[must_use]
    pub fn counts(&self) -> Counts {
        let errors = self
            .issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .count();
        Counts {
            errors,
            warnings: self.issues.len() - errors,
        }
    }

    pub fn of_kind(&self, kind: IssueKind) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(move |i| i.kind == kind)
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{sev}: {}", self.message)?;
        if !self.entities.is_empty() {
            let ids: Vec<String> = self.entities.iter().map(ToString::to_string).collect();
            write!(f, " [entities {}]", ids.join(", "))?;
        }
        Ok(())
    }
}

/// Run every check. Issues are ordered by layer, then check, then entity.
#[must_use]
pub fn validate(model: &ExportModel) -> Report {
    let mut issues = Vec::new();
    for layer in &model.layers {
        degenerate(layer, &mut issues);
        open_paths(layer, &mut issues);
        duplicates(layer, &mut issues);
        self_intersections(layer, &mut issues);
    }
    Report { issues }
}

fn degenerate(layer: &ExportLayer, out: &mut Vec<Issue>) {
    for &e in &layer.degenerate {
        out.push(Issue {
            severity: Severity::Warning,
            kind: IssueKind::Degenerate,
            layer: layer.id,
            entities: vec![e],
            message: format!(
                "layer \"{}\": zero-length segment (removed from the export)",
                layer.name
            ),
        });
    }
}

fn open_paths(layer: &ExportLayer, out: &mut Vec<Issue>) {
    if !matches!(layer.operation, Operation::Cut(_)) {
        return;
    }
    for p in layer.paths.iter().filter(|p| !p.closed) {
        out.push(Issue {
            severity: Severity::Error,
            kind: IssueKind::OpenPath,
            layer: layer.id,
            entities: p.entities(),
            message: format!(
                "layer \"{}\": open path on a Cut layer ({} segment(s))",
                layer.name,
                p.segs.len()
            ),
        });
    }
}

fn flat(layer: &ExportLayer) -> Vec<&PathSeg> {
    layer.paths.iter().flat_map(|p| &p.segs).collect()
}

fn duplicates(layer: &ExportLayer, out: &mut Vec<Issue>) {
    let segs = flat(layer);
    for (i, a) in segs.iter().enumerate() {
        for b in &segs[i + 1..] {
            if overlap(&a.shape, &b.shape) != Overlap::None {
                out.push(Issue {
                    severity: Severity::Error,
                    kind: IssueKind::DuplicateSegment,
                    layer: layer.id,
                    entities: vec![a.entity, b.entity],
                    message: format!(
                        "layer \"{}\": duplicate or overlapping segments (double cut)",
                        layer.name
                    ),
                });
            }
        }
    }
}

fn self_intersections(layer: &ExportLayer, out: &mut Vec<Issue>) {
    let segs = flat(layer);
    let boxes: Vec<_> = segs.iter().map(|s| s.shape.bbox()).collect();
    let touches_end = |s: &PathSeg, p: Vec2| {
        s.shape
            .ends()
            .is_some_and(|(a, b)| a.distance(p) <= EXPORT_TOL || b.distance(p) <= EXPORT_TOL)
    };
    for i in 0..segs.len() {
        for j in i + 1..segs.len() {
            let (a, b) = (segs[i], segs[j]);
            let (ba, bb) = (boxes[i], boxes[j]);
            if ba.max.x + EXPORT_TOL < bb.min.x
                || bb.max.x + EXPORT_TOL < ba.min.x
                || ba.max.y + EXPORT_TOL < bb.min.y
                || bb.max.y + EXPORT_TOL < ba.min.y
                || overlap(&a.shape, &b.shape) != Overlap::None
            {
                continue;
            }
            // Meeting at a shared endpoint is a join, not a crossing.
            let crossing = a
                .shape
                .intersections(&b.shape)
                .into_iter()
                .any(|p| !(touches_end(a, p) && touches_end(b, p)));
            if crossing {
                out.push(Issue {
                    severity: Severity::Warning,
                    kind: IssueKind::SelfIntersection,
                    layer: layer.id,
                    entities: vec![a.entity, b.entity],
                    message: format!("layer \"{}\": segments intersect", layer.name),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::testutil::*;
    use locus_doc::OperationKind;

    fn model(op: Operation, segs: Vec<PathSeg>) -> ExportModel {
        ExportModel {
            layers: vec![layer(op, segs)],
        }
    }

    #[test]
    fn closed_rectangle_is_clean() {
        let r = validate(&model(
            OperationKind::Cut.default_operation(),
            rect(1, 0.0, 0.0, 10.0, 10.0),
        ));
        assert!(r.issues.is_empty(), "{:?}", r.issues);
    }

    #[test]
    fn open_path_is_an_error_on_cut_only() {
        let mut segs = rect(1, 0.0, 0.0, 10.0, 10.0);
        segs.pop();
        let r = validate(&model(OperationKind::Cut.default_operation(), segs.clone()));
        let issue = r.of_kind(IssueKind::OpenPath).next().unwrap();
        assert_eq!(issue.severity, Severity::Error);
        assert_eq!(issue.entities.len(), 3);
        assert!(r.has_errors());
        // An open path is normal for a Draw layer.
        assert!(
            validate(&model(OperationKind::Draw.default_operation(), segs))
                .issues
                .is_empty()
        );
    }

    #[test]
    fn duplicate_and_collinear_overlap_are_errors() {
        let mut segs = rect(1, 0.0, 0.0, 10.0, 10.0);
        segs.push(seg(9, (0.0, 0.0), (5.0, 0.0))); // lies on edge 1
        let r = validate(&model(OperationKind::Cut.default_operation(), segs));
        let d: Vec<_> = r.of_kind(IssueKind::DuplicateSegment).collect();
        assert_eq!(d.len(), 1);
        assert!(d[0].entities.contains(&EntityId::new(9)));
        assert!(d[0].entities.contains(&EntityId::new(1)));
        assert_eq!(d[0].severity, Severity::Error);
    }

    #[test]
    fn crossing_is_a_warning_but_a_shared_corner_is_not() {
        // Bow-tie: two crossing diagonals closed by two edges.
        let segs = vec![
            seg(1, (0.0, 0.0), (10.0, 10.0)),
            seg(2, (10.0, 10.0), (10.0, 0.0)),
            seg(3, (10.0, 0.0), (0.0, 10.0)),
            seg(4, (0.0, 10.0), (0.0, 0.0)),
        ];
        let r = validate(&model(OperationKind::Cut.default_operation(), segs));
        let x: Vec<_> = r.of_kind(IssueKind::SelfIntersection).collect();
        assert_eq!(x.len(), 1);
        assert_eq!(x[0].severity, Severity::Warning);
        assert!(!r.has_errors());
    }

    #[test]
    fn degenerate_is_reported() {
        let mut m = model(
            OperationKind::Cut.default_operation(),
            rect(1, 0.0, 0.0, 10.0, 10.0),
        );
        m.layers[0].degenerate.push(EntityId::new(42));
        let r = validate(&m);
        let d = r.of_kind(IssueKind::Degenerate).next().unwrap();
        assert_eq!(d.entities, vec![EntityId::new(42)]);
    }
}
