// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exporter plugins in the `locus-export` registry (#184, §15.2, ADR 0009):
//! a plugin exporter receives exactly what the built-in SVG, DXF and PDF
//! writers receive (the validated, ordered export model) and returns bytes.
//! The host does the file dialog and the write (§15.4).

use locus_export::{ExportError, Exported, Exporter};

use crate::error::CallError;
pub use crate::model::to_model;
use crate::runtime::Plugin;
use crate::types::FormatInfo;

/// A loaded exporter plugin, usable wherever a built-in exporter is.
#[derive(Debug)]
pub struct PluginExporter {
    plugin: Plugin,
    format: FormatInfo,
}

impl PluginExporter {
    /// Asks the plugin for its format; fails if it isn't an exporter.
    pub fn new(plugin: Plugin) -> Result<PluginExporter, CallError> {
        let format = plugin.export_format()?;
        Ok(PluginExporter { plugin, format })
    }

    pub fn plugin(&self) -> &Plugin {
        &self.plugin
    }
}

impl Exporter for PluginExporter {
    fn id(&self) -> &str {
        &self.format.id
    }

    fn name(&self) -> &str {
        &self.format.name
    }

    fn extension(&self) -> &str {
        &self.format.extension
    }

    fn mime(&self) -> &str {
        &self.format.mime
    }

    fn export(&self, model: &locus_export::ExportModel) -> Result<Exported, ExportError> {
        let bytes = self.plugin.export(&to_model(model)).map_err(|e| match e {
            CallError::Plugin(crate::types::PluginFailure::Unsupported(m)) => {
                ExportError::Unsupported(m)
            }
            other => ExportError::Failed(other.to_string()),
        })?;
        Ok(Exported {
            bytes,
            extension: self.format.extension.clone(),
            mime: self.format.mime.clone(),
        })
    }
}
