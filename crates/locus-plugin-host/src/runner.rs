// SPDX-License-Identifier: AGPL-3.0-or-later

//! Re-running modifier plugins from the installed registry (#184, §15.2).
//!
//! The document asks for a re-run when a modifier object's sources change
//! (`locus_doc::generated::PluginRunner`). This runs the **installed** plugin
//! with the object's stored parameters, but only if its version is the one
//! the object was made with: a different version is an explicit update
//! (§15.5), never a side effect of an edit. Any failure keeps the cached
//! output, and the object is reported stale.

use std::collections::BTreeMap;
use std::sync::Mutex;

use locus_doc::generated::{GenPath, GeneratedGeometry, PluginRef, PluginRunner};

use crate::doc::{from_stored, paths_to_cache, to_paths};
use crate::registry::Registry;
use crate::runtime::{Host, Plugin};

#[derive(Debug)]
pub struct RegistryRunner {
    registry: Registry,
    host: Mutex<Option<Host>>,
    loaded: Mutex<BTreeMap<String, Plugin>>,
}

impl RegistryRunner {
    pub fn new(registry: Registry) -> RegistryRunner {
        RegistryRunner {
            registry,
            host: Mutex::new(None),
            loaded: Mutex::new(BTreeMap::new()),
        }
    }

    /// Forgets loaded plugins, e.g. after an install or uninstall.
    pub fn reload(&self) {
        if let Ok(mut l) = self.loaded.lock() {
            l.clear();
        }
    }

    fn with_plugin<T>(
        &self,
        id: &str,
        f: impl FnOnce(&Plugin) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut loaded = self.loaded.lock().map_err(|_| "runner lock poisoned")?;
        if !loaded.contains_key(id) {
            let mut host = self.host.lock().map_err(|_| "runner lock poisoned")?;
            if host.is_none() {
                *host = Some(Host::new().map_err(|e| e.to_string())?);
            }
            let host = host.as_ref().ok_or("the plugin host didn't start")?;
            let (_, p) = self.registry.load(host, id).map_err(|e| e.to_string())?;
            loaded.insert(id.to_owned(), p);
        }
        let p = loaded.get(id).ok_or("plugin not loaded")?;
        f(p)
    }
}

impl PluginRunner for RegistryRunner {
    fn modify(&self, plugin: &PluginRef, input: &[GenPath]) -> Result<GeneratedGeometry, String> {
        self.with_plugin(&plugin.id, |p| {
            if p.info().version != plugin.version {
                return Err(format!(
                    "installed version {} differs from {}; update the object explicitly",
                    p.info().version,
                    plugin.version
                ));
            }
            let schema = p.modifier_schema().map_err(|e| e.to_string())?;
            let params = from_stored(&schema, &plugin.params);
            let out = p
                .modify(&to_paths(input), &params)
                .map_err(|e| e.to_string())?;
            Ok(paths_to_cache(&out))
        })
    }
}
