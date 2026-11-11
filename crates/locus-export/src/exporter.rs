// SPDX-License-Identifier: AGPL-3.0-or-later

//! The internal `Exporter` boundary and its registry (SPEC §13 M2, §15.2, #136).
//!
//! An exporter takes validated export geometry plus the layer operations the
//! model carries and returns bytes with the file extension and MIME type. The
//! built-in SVG, DXF and PDF writers implement exactly this interface, so the
//! app and CLI pick writers from a [`Registry`] and the plugin host (§15) can
//! later register its exporters beside them without either caller changing.
//!
//! Exporters never see the [`Document`](locus_doc::Document) (SPEC §11) and
//! never do I/O: the caller saves the bytes.

use crate::pipeline::ExportModel;
use thiserror::Error;

/// Why an exporter produced no output.
#[derive(Debug, Error)]
pub enum ExportError {
    /// The exporter cannot represent something in the model.
    #[error("{0}")]
    Unsupported(String),
    /// A plugin exporter trapped, timed out or returned malformed output (§15.5).
    #[error("{0}")]
    Failed(String),
}

/// What an exporter returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exported {
    pub bytes: Vec<u8>,
    /// File extension without the dot, e.g. `svg`.
    pub extension: String,
    pub mime: String,
}

/// A file-format writer.
pub trait Exporter: Send + Sync {
    /// Stable identifier used on the command line and in plugin manifests, e.g. `svg`.
    fn id(&self) -> &str;
    /// Human-readable name for menus, e.g. `SVG`.
    fn name(&self) -> &str;
    /// File extension without the dot.
    fn extension(&self) -> &str;
    fn mime(&self) -> &str;
    /// Write validated, ordered geometry (see [`prepare`](crate::prepare)).
    ///
    /// # Errors
    /// [`ExportError`] when the model cannot be written.
    fn export(&self, model: &ExportModel) -> Result<Exported, ExportError>;
}

macro_rules! builtin {
    ($ty:ident, $id:literal, $name:literal, $ext:literal, $mime:literal, $write:path) => {
        #[derive(Debug, Clone, Copy, Default)]
        pub struct $ty;

        impl Exporter for $ty {
            fn id(&self) -> &str {
                $id
            }
            fn name(&self) -> &str {
                $name
            }
            fn extension(&self) -> &str {
                $ext
            }
            fn mime(&self) -> &str {
                $mime
            }
            fn export(&self, model: &ExportModel) -> Result<Exported, ExportError> {
                Ok(Exported {
                    bytes: $write(model),
                    extension: $ext.to_owned(),
                    mime: $mime.to_owned(),
                })
            }
        }
    };
}

builtin!(
    SvgExporter,
    "svg",
    "SVG",
    "svg",
    "image/svg+xml",
    crate::svg::to_svg_model
);
builtin!(
    PdfExporter,
    "pdf",
    "PDF",
    "pdf",
    "application/pdf",
    crate::pdf::to_pdf_model
);
builtin!(
    DxfExporter,
    "dxf",
    "DXF (R12)",
    "dxf",
    "application/dxf",
    crate::dxf::to_dxf_model
);

/// The exporters available to the app and CLI, in menu order.
#[derive(Default)]
pub struct Registry {
    exporters: Vec<Box<dyn Exporter>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.exporters.iter().map(|e| e.id()))
            .finish()
    }
}

impl Registry {
    /// SVG, PDF and DXF.
    #[must_use]
    pub fn builtin() -> Self {
        let mut r = Self::default();
        r.register(Box::new(SvgExporter));
        r.register(Box::new(PdfExporter));
        r.register(Box::new(DxfExporter));
        r
    }

    /// Add an exporter. An existing one with the same id is replaced, so a
    /// plugin cannot end up listed twice.
    pub fn register(&mut self, exporter: Box<dyn Exporter>) {
        match self.exporters.iter().position(|e| e.id() == exporter.id()) {
            Some(i) => self.exporters[i] = exporter,
            None => self.exporters.push(exporter),
        }
    }

    /// Add an exporter only if its id is free: plugin exporters (§15.2) may
    /// sit beside the built-ins but never replace one. Returns whether it
    /// was added.
    pub fn add(&mut self, exporter: Box<dyn Exporter>) -> bool {
        if self.get(exporter.id()).is_some() {
            return false;
        }
        self.exporters.push(exporter);
        true
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&dyn Exporter> {
        self.exporters
            .iter()
            .find(|e| e.id() == id)
            .map(AsRef::as_ref)
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn Exporter> {
        self.exporters.iter().map(AsRef::as_ref)
    }

    /// Ids in menu order, for usage messages.
    #[must_use]
    pub fn ids(&self) -> Vec<&str> {
        self.iter().map(Exporter::id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::testutil::*;
    use locus_doc::OperationKind;

    fn model() -> ExportModel {
        ExportModel {
            layers: vec![layer(OperationKind::Cut, rect(1, 0.0, 0.0, 10.0, 10.0))],
        }
    }

    #[test]
    fn builtins_match_the_free_functions_byte_for_byte() {
        let m = model();
        let r = Registry::builtin();
        assert_eq!(r.ids(), ["svg", "pdf", "dxf"]);
        let svg = r.get("svg").unwrap().export(&m).unwrap();
        assert_eq!(svg.bytes, crate::to_svg_model(&m));
        assert_eq!(
            (svg.extension.as_str(), svg.mime.as_str()),
            ("svg", "image/svg+xml")
        );
        assert_eq!(
            r.get("pdf").unwrap().export(&m).unwrap().bytes,
            crate::to_pdf_model(&m)
        );
        assert_eq!(
            r.get("dxf").unwrap().export(&m).unwrap().bytes,
            crate::to_dxf_model(&m)
        );
        assert!(r.get("hpgl").is_none());
    }

    struct Fake(&'static str);
    impl Exporter for Fake {
        fn id(&self) -> &str {
            self.0
        }
        fn name(&self) -> &str {
            "Fake"
        }
        fn extension(&self) -> &str {
            "fake"
        }
        fn mime(&self) -> &str {
            "text/plain"
        }
        fn export(&self, _: &ExportModel) -> Result<Exported, ExportError> {
            Err(ExportError::Failed("boom".into()))
        }
    }

    #[test]
    fn register_adds_and_replaces_by_id() {
        let mut r = Registry::builtin();
        r.register(Box::new(Fake("fake")));
        assert_eq!(r.ids(), ["svg", "pdf", "dxf", "fake"]);
        r.register(Box::new(Fake("svg")));
        assert_eq!(r.ids().len(), 4);
        assert_eq!(r.get("svg").unwrap().name(), "Fake");
        assert!(r.get("fake").unwrap().export(&model()).is_err());
    }

    #[test]
    fn add_never_replaces() {
        let mut r = Registry::builtin();
        assert!(!r.add(Box::new(Fake("svg"))));
        assert_eq!(r.get("svg").unwrap().name(), "SVG");
        assert!(r.add(Box::new(Fake("hpgl"))));
        assert_eq!(r.ids(), ["svg", "pdf", "dxf", "hpgl"]);
    }
}
