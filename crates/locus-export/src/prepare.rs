// SPDX-License-Identifier: AGPL-3.0-or-later

//! The shared "always through validation" front door for every exporter
//! (SPEC §8 pipeline, #128): collect → optional auto-fix → operation
//! transforms (kerf, blade offset, hatch) → validate → order.
//!
//! The app's pre-export panel and `locus-cli` both call this, so they cannot
//! disagree about what is wrong or what a fix does. The document is read only.

use crate::fix::{Fix, autofix};
use crate::ops::apply_operations;
use crate::order::order;
use crate::pipeline::{ExportModel, collect};
use crate::profile::MachineProfile;
use crate::validate::{Report, validate_for};
use locus_doc::Document;

/// Result of preparing a document for export.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    /// Geometry ready for any writer (ordered; auto-fixed if requested).
    pub model: ExportModel,
    /// Fixes applied to `model` (empty unless `auto_fix`).
    pub fixes: Vec<Fix>,
    /// Validation of `model` as returned: after the fixes when `auto_fix`.
    pub report: Report,
}

impl Prepared {
    /// Blocking errors remain: exporters must refuse unless the user overrides.
    #[must_use]
    pub fn blocked(&self) -> bool {
        self.report.has_errors()
    }
}

/// Collect, optionally fix, validate and order the document's export geometry.
#[must_use]
pub fn prepare(doc: &Document, auto_fix: bool) -> Prepared {
    prepare_for(doc, auto_fix, None)
}

/// [`prepare`] for a machine profile (SPEC §7): its colour mapping is applied
/// to the layers before validation and ordering.
#[must_use]
pub fn prepare_for(doc: &Document, auto_fix: bool, profile: Option<&MachineProfile>) -> Prepared {
    let mut model = collect(doc);
    if let Some(p) = profile {
        p.apply(&mut model);
    }
    let fixes = if auto_fix {
        autofix(&mut model)
    } else {
        Vec::new()
    };
    // Styled machine layers cut the stroke outline (#428), before kerf and
    // hatch, so those apply to what is cut.
    crate::pipeline::apply_stroke_styles(&mut model);
    // SPEC §8: operation transforms before validation, so what is checked
    // is what will be cut.
    apply_operations(&mut model);
    // The job placed on the bed (SPEC §7, #443), before the bed check.
    if let Some((offset, bed)) = placement(doc, profile) {
        model.translate(offset);
        model.frame = Some(bed.rect());
    }
    // Validate before ordering: the order stage only permutes and re-starts paths.
    let report = validate_for(&model, profile.and_then(|p| p.bed.as_ref()));
    order(&mut model);
    Prepared {
        model,
        fixes,
        report,
    }
}

/// The job's placement, when it applies: the document has one and the
/// profile has a bed (SPEC §7, #443). Export then moves the design by the
/// offset and writes the whole bed as the page, so the job lands where it
/// was placed.
#[must_use]
pub fn placement(
    doc: &Document,
    profile: Option<&MachineProfile>,
) -> Option<(locus_geom::Vec2, crate::profile::Bed)> {
    Some((doc.placement()?, profile?.bed?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #443: a placed job is moved onto the bed, the page is the bed, and
    /// the bed check sees the moved geometry; unplaced, nothing changes.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_placed_job_lands_on_the_bed() {
        use locus_doc::{Command, Editor, Geom};
        use locus_geom::Vec2;
        let mut ed = Editor::new(Document::new());
        let l = Document::default_layer();
        let c = ed
            .apply(&Command::AddEntity {
                layer: l,
                construction: false,
                style: Default::default(),
                geom: Geom::Point {
                    pos: Vec2::new(0.0, 0.0),
                },
            })
            .unwrap()
            .entities[0];
        ed.apply(&Command::AddEntity {
            layer: l,
            construction: false,
            style: Default::default(),
            geom: Geom::Circle {
                center: c,
                radius: 10.0,
            },
        })
        .unwrap();
        let lb = crate::profile::find("lightburn", &[]).unwrap();
        // Unplaced: the design's own bounds, centred on the sketch origin.
        let free = prepare_for(ed.document(), false, Some(&lb));
        let b = free.model.bounds().unwrap();
        assert!((b.min.x + 10.0).abs() < 1e-9);
        assert!(free.report.issues.iter().any(|i| i.message.contains("bed")));
        // Placed 50 mm in: inside the bed, which is now the page.
        ed.apply(&Command::SetPlacement {
            offset: Some(Vec2::new(50.0, 60.0)),
        })
        .unwrap();
        let placed = prepare_for(ed.document(), false, Some(&lb));
        assert_eq!(placed.model.bounds(), Some(lb.bed.unwrap().rect()));
        let shapes: Vec<_> = placed.model.layers[0].paths[0].segs.clone();
        assert!(matches!(
            shapes[0].shape,
            crate::pipeline::Shape::Circle(c) if (c.center - Vec2::new(50.0, 60.0)).length() < 1e-9
        ));
        assert!(
            !placed
                .report
                .issues
                .iter()
                .any(|i| i.message.contains("bed"))
        );
        // Without a bed the placement has nothing to place on.
        assert!(
            placement(
                ed.document(),
                crate::profile::find("generic-svg", &[]).as_ref()
            )
            .is_none()
        );
        let svg = String::from_utf8(crate::to_svg_model(&placed.model)).unwrap();
        assert!(svg.contains("width=\"400mm\" height=\"400mm\""), "{svg}");
    }

    #[test]
    fn empty_document_prepares_cleanly() {
        let p = prepare(&Document::new(), true);
        assert!(!p.blocked());
        assert!(p.fixes.is_empty());
        assert!(p.model.is_empty());
    }
}
