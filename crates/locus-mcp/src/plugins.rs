// SPDX-License-Identifier: AGPL-3.0-or-later

//! [`Plugins`] over the installed-plugin registry (SPEC §15), for
//! `run_generator`, `apply_modifier` and `list_plugins` (§16.3).
//!
//! Only translation: plugin parameter specs to JSON Schema, JSON values to
//! plugin parameters. Checking, defaults and running are `locus-plugin-host`'s
//! (`resolve`, the sandbox limits of §15.4). The direction is one-way: the
//! host calls plugins; plugins have no import that reaches MCP (§16.4).

use std::collections::BTreeMap;

use locus_doc::generated::{GenPath, GeneratedGeometry, PluginRef};
use locus_plugin_host::doc::{paths_to_cache, plugin_ref, resolve, to_cache, to_paths};
use locus_plugin_host::registry::Registry;
use locus_plugin_host::types::{Kind, Param, ParamKind, ParamSpec, ParamValue};
use locus_plugin_host::{Host, Plugin};
use serde_json::{Map, Value, json};

use crate::host::{PluginEntry, Plugins};

/// Installed plugins, loaded on first use.
#[derive(Debug)]
pub struct RegistryPlugins {
    registry: Registry,
    host: Option<Host>,
    loaded: BTreeMap<String, Plugin>,
}

impl RegistryPlugins {
    #[must_use]
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            host: None,
            loaded: BTreeMap::new(),
        }
    }

    fn plugin(&mut self, id: &str) -> Result<&Plugin, String> {
        if !self.loaded.contains_key(id) {
            if self.host.is_none() {
                self.host = Some(Host::new().map_err(|e| e.to_string())?);
            }
            let host = self.host.as_ref().ok_or("the plugin host didn't start")?;
            let (_, p) = self
                .registry
                .load(host, id)
                .map_err(|e| format!("plugin `{id}`: {e}"))?;
            self.loaded.insert(id.to_owned(), p);
        }
        self.loaded
            .get(id)
            .ok_or_else(|| "plugin not loaded".into())
    }
}

/// JSON Schema for a plugin's parameters (SPEC §15.3).
#[must_use]
pub fn schema(specs: &[ParamSpec]) -> Value {
    let props: Map<String, Value> = specs
        .iter()
        .map(|s| {
            let desc = if s.help.is_empty() {
                s.label.clone()
            } else {
                format!("{}: {}", s.label, s.help)
            };
            let mut v = match &s.kind {
                ParamKind::Number(n) => {
                    let mut v = json!({ "type": "number", "default": n.default });
                    if let Some(m) = n.min {
                        v["minimum"] = json!(m);
                    }
                    if let Some(m) = n.max {
                        v["maximum"] = json!(m);
                    }
                    if !n.unit.is_empty() {
                        v["x-unit"] = json!(n.unit);
                    }
                    v
                }
                ParamKind::Integer(n) => {
                    let mut v = json!({ "type": "integer", "default": n.default });
                    if let Some(m) = n.min {
                        v["minimum"] = json!(m);
                    }
                    if let Some(m) = n.max {
                        v["maximum"] = json!(m);
                    }
                    v
                }
                ParamKind::Boolean(b) => json!({ "type": "boolean", "default": b }),
                ParamKind::Choice(c) => json!({
                    "type": "string",
                    "enum": c.options,
                    "default": c.options.get(c.default as usize),
                }),
                ParamKind::Text(t) => json!({
                    "type": "string", "default": t.default, "maxLength": t.max_length,
                }),
                ParamKind::DocParameter => json!({ "type": "number", "default": 0.0 }),
            };
            v["description"] = json!(desc);
            (s.key.clone(), v)
        })
        .collect();
    json!({ "type": "object", "properties": props, "additionalProperties": false })
}

/// JSON arguments as plugin parameters, by each key's spec. Unknown keys
/// are passed on so `resolve` refuses them by name.
///
/// # Errors
/// A value of the wrong JSON type.
pub fn params(specs: &[ParamSpec], given: &Map<String, Value>) -> Result<Vec<Param>, String> {
    given
        .iter()
        .map(|(key, v)| {
            let bad = || format!("parameter `{key}` has the wrong type");
            let value = match specs.iter().find(|s| &s.key == key).map(|s| &s.kind) {
                Some(ParamKind::Number(_) | ParamKind::DocParameter) | None if v.is_number() => {
                    ParamValue::Number(v.as_f64().ok_or_else(bad)?)
                }
                Some(ParamKind::Integer(_)) => ParamValue::Integer(v.as_i64().ok_or_else(bad)?),
                Some(ParamKind::Boolean(_)) => ParamValue::Boolean(v.as_bool().ok_or_else(bad)?),
                Some(ParamKind::Choice(c)) => {
                    let i = match v {
                        Value::String(s) => c.options.iter().position(|o| o == s),
                        Value::Number(n) => n.as_u64().and_then(|n| usize::try_from(n).ok()),
                        _ => None,
                    }
                    .ok_or_else(|| format!("parameter `{key}` is not one of the options"))?;
                    ParamValue::Choice(u32::try_from(i).map_err(|_| bad())?)
                }
                Some(ParamKind::Text(_)) | None => {
                    ParamValue::Text(v.as_str().ok_or_else(bad)?.to_owned())
                }
                Some(ParamKind::Number(_) | ParamKind::DocParameter) => return Err(bad()),
            };
            Ok(Param {
                key: key.clone(),
                value,
            })
        })
        .collect()
}

impl Plugins for RegistryPlugins {
    fn list(&mut self) -> Vec<PluginEntry> {
        let installed = self.registry.list();
        installed
            .into_iter()
            .map(|i| {
                let m = &i.manifest;
                let mut e = PluginEntry {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    version: m.version.clone(),
                    kinds: m.kinds.iter().map(|k| k.name().to_owned()).collect(),
                    generator_params: None,
                    modifier_params: None,
                };
                let (is_gen, is_mod) = (
                    m.kinds.contains(&Kind::Generator),
                    m.kinds.contains(&Kind::Modifier),
                );
                if let Ok(p) = self.plugin(&m.id) {
                    if is_gen {
                        e.generator_params = p.generator_schema().ok().map(|s| schema(&s));
                    }
                    if is_mod {
                        e.modifier_params = p.modifier_schema().ok().map(|s| schema(&s));
                    }
                }
                e
            })
            .collect()
    }

    fn generate(
        &mut self,
        id: &str,
        given: &Map<String, Value>,
    ) -> Result<(PluginRef, GeneratedGeometry), String> {
        let p = self.plugin(id)?;
        let specs = p.generator_schema().map_err(|e| e.to_string())?;
        let values = resolve(&specs, &params(&specs, given)?).map_err(|e| e.to_string())?;
        let out = p.generate(&values).map_err(|e| e.to_string())?;
        let info = p.info();
        Ok((plugin_ref(&info.id, &info.version, &values), to_cache(&out)))
    }

    fn modify(
        &mut self,
        id: &str,
        given: &Map<String, Value>,
        input: &[GenPath],
    ) -> Result<(PluginRef, GeneratedGeometry), String> {
        let p = self.plugin(id)?;
        let specs = p.modifier_schema().map_err(|e| e.to_string())?;
        let values = resolve(&specs, &params(&specs, given)?).map_err(|e| e.to_string())?;
        let out = p
            .modify(&to_paths(input), &values)
            .map_err(|e| e.to_string())?;
        let info = p.info();
        Ok((
            plugin_ref(&info.id, &info.version, &values),
            paths_to_cache(&out),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use locus_plugin_host::types::{ChoiceSpec, NumberSpec};

    fn specs() -> Vec<ParamSpec> {
        vec![
            ParamSpec {
                key: "width".into(),
                label: "Width".into(),
                help: String::new(),
                kind: ParamKind::Number(NumberSpec {
                    unit: "mm".into(),
                    min: Some(1.0),
                    max: None,
                    default: 10.0,
                }),
            },
            ParamSpec {
                key: "style".into(),
                label: "Style".into(),
                help: "Corner style".into(),
                kind: ParamKind::Choice(ChoiceSpec {
                    options: vec!["sharp".into(), "round".into()],
                    default: 0,
                }),
            },
        ]
    }

    #[test]
    fn schema_has_ranges_defaults_and_options() {
        let s = schema(&specs());
        assert_eq!(s["properties"]["width"]["minimum"], 1.0);
        assert_eq!(s["properties"]["width"]["default"], 10.0);
        assert_eq!(s["properties"]["style"]["enum"], json!(["sharp", "round"]));
        assert_eq!(
            s["properties"]["style"]["description"],
            "Style: Corner style"
        );
    }

    #[test]
    fn params_from_json() {
        let given = json!({ "width": 12.5, "style": "round" });
        let p = params(&specs(), given.as_object().unwrap()).unwrap();
        assert!(p.contains(&Param {
            key: "width".into(),
            value: ParamValue::Number(12.5)
        }));
        assert!(p.contains(&Param {
            key: "style".into(),
            value: ParamValue::Choice(1)
        }));
        let bad = json!({ "style": "wavy" });
        assert!(params(&specs(), bad.as_object().unwrap()).is_err());
        let bad = json!({ "width": "wide" });
        assert!(params(&specs(), bad.as_object().unwrap()).is_err());
        // Out of range is the host's `resolve` to refuse.
        let low = json!({ "width": 0.5 });
        let p = params(&specs(), low.as_object().unwrap()).unwrap();
        assert!(resolve(&specs(), &p).is_err());
    }
}
