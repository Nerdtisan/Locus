// SPDX-License-Identifier: AGPL-3.0-or-later

//! Validator plugins in the export pipeline (#234, §15.2, §8).
//!
//! The built-in §8 checks always run first and on their own
//! (`locus_export::prepare`); validator plugins add findings next to them.
//! Each plugin runs in its own sandboxed call, so one that traps, times out
//! or returns garbage only produces a warning saying so: it can never hide
//! or block the built-in checks, or the other validators.

use locus_doc::{EntityId, LayerId};

use crate::doc::defaults;
use crate::export::to_model;
use crate::runtime::Plugin;
use crate::types::Severity;

/// One finding from a validator plugin, mapped back to the document.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginFinding {
    /// The plugin's name, for the report.
    pub plugin: String,
    pub severity: Severity,
    pub message: String,
    pub layer: Option<LayerId>,
    /// Entities to highlight: those of the named path, if any.
    pub entities: Vec<EntityId>,
}

impl std::fmt::Display for PluginFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{sev} ({}): {}", self.plugin, self.message)?;
        if !self.entities.is_empty() {
            let ids: Vec<String> = self.entities.iter().map(ToString::to_string).collect();
            write!(f, " [entities {}]", ids.join(", "))?;
        }
        Ok(())
    }
}

/// Runs every validator on the export model, in order. Parameters are the
/// schema defaults. A validator that fails yields one warning naming it.
#[must_use]
pub fn run(validators: &[&Plugin], model: &locus_export::ExportModel) -> Vec<PluginFinding> {
    let plugin_model = to_model(model);
    let mut out = Vec::new();
    for p in validators {
        let name = p.info().name.clone();
        let result = p
            .validator_schema()
            .and_then(|schema| p.validate(&plugin_model, &defaults(&schema)));
        match result {
            Ok(found) => out.extend(found.into_iter().map(|f| {
                let layer = f.layer.and_then(|l| model.layers.get(l as usize));
                let entities = match (layer, f.path) {
                    (Some(l), Some(i)) => l
                        .paths
                        .get(i as usize)
                        .map(|path| path.segs.iter().map(|s| s.entity).collect())
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                PluginFinding {
                    plugin: name.clone(),
                    severity: f.severity,
                    message: crate::validate::sanitise_message(&f.message),
                    layer: layer.map(|l| l.id),
                    entities,
                }
            })),
            Err(e) => out.push(PluginFinding {
                plugin: name,
                severity: Severity::Warning,
                message: format!("the validator failed and was skipped: {e}"),
                layer: None,
                entities: Vec::new(),
            }),
        }
    }
    out
}

/// True when any finding is an error. Like the built-in errors, these block
/// export unless the user overrides (§8).
#[must_use]
pub fn has_errors(found: &[PluginFinding]) -> bool {
    found.iter().any(|f| f.severity == Severity::Error)
}
