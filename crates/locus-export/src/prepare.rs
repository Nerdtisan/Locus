// SPDX-License-Identifier: AGPL-3.0-or-later

//! The shared "always through validation" front door for every exporter
//! (SPEC §8 pipeline, #128): collect → optional auto-fix → validate → order.
//!
//! The app's pre-export panel and `locus-cli` both call this, so they cannot
//! disagree about what is wrong or what a fix does. The document is read only.

use crate::fix::{Fix, autofix};
use crate::order::order;
use crate::pipeline::{ExportModel, collect};
use crate::validate::{Report, validate};
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
    let mut model = collect(doc);
    let fixes = if auto_fix {
        autofix(&mut model)
    } else {
        Vec::new()
    };
    // Validate before ordering: the order stage only permutes and re-starts paths.
    let report = validate(&model);
    order(&mut model);
    Prepared {
        model,
        fixes,
        report,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_document_prepares_cleanly() {
        let p = prepare(&Document::new(), true);
        assert!(!p.blocked());
        assert!(p.fixes.is_empty());
        assert!(p.model.is_empty());
    }
}
