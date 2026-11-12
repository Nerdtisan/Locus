// SPDX-License-Identifier: AGPL-3.0-or-later

//! The core module inside a plugin component (ADR 0016, #253).
//!
//! Every Locus plugin is a component that `wit-component` made from one
//! `wit-bindgen` core module, and the plugin worlds import nothing (ADR 0009).
//! The web host runs that core module directly on the browser's engine, so
//! before anything runs it is taken out of the component and checked:
//!
//! - the component holds **exactly one** core module, which **imports
//!   nothing** (so instantiating it with an empty import object is the whole
//!   sandbox, as the desktop's empty linker is);
//! - it defines exactly one 32-bit, unshared memory, exported as `memory`, and
//!   exports `cabi_realloc`;
//! - it validates without threads or relaxed SIMD (relaxed SIMD results differ
//!   between CPUs, which would break §15.5 determinism);
//! - its memory **maximum is rewritten** to at most 256 MB, which the engine
//!   then enforces (§15.4).
//!
//! The plugin's interface comes from the core export names wit-bindgen uses:
//! `locus:plugin/<interface>@<version>#<function>`.

use wasmparser::{Encoding, ExternalKind, Parser, Payload, TypeRef, Validator, WasmFeatures};

use crate::api::{PACKAGE, SUPPORTED_API_MAJORS};
use crate::types::Kind;

/// Largest memory a plugin may grow to, in 64 KiB pages (256 MB, §15.4).
pub const MAX_MEMORY_PAGES: u64 = 4096;

/// Why a component can't run on the core-module host.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModuleError {
    #[error("not a valid WebAssembly component: {0}")]
    Invalid(String),
    #[error("the plugin has {0} core modules; Locus runs plugins with exactly one")]
    Modules(usize),
    /// The sandbox: the core module may import nothing (§15.4, ADR 0009).
    #[error("the plugin asks for host access it can't have: {}", .0.join(", "))]
    Imports(Vec<String>),
    #[error("the plugin's memory layout isn't supported: {0}")]
    Memory(String),
    #[error("the plugin doesn't export the Locus plugin interface")]
    NotAPlugin,
    #[error("the plugin targets plugin API {found}; this Locus supports {supported}")]
    UnsupportedApi { found: String, supported: String },
}

/// A checked core module, ready to compile and instantiate with no imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreModule {
    /// The module bytes, with the memory maximum capped.
    pub wasm: Vec<u8>,
    /// The `locus:plugin` version from the export names, e.g. `1.0.0`.
    pub api: String,
    /// The plugin kinds whose functions are all exported.
    pub kinds: Vec<Kind>,
}

fn invalid(e: impl std::fmt::Display) -> ModuleError {
    ModuleError::Invalid(crate::validate::sanitise_message(&e.to_string()))
}

impl CoreModule {
    /// Takes the core module out of a plugin component and checks it.
    pub fn from_component(component: &[u8]) -> Result<CoreModule, ModuleError> {
        if component.len() > crate::validate::MAX_COMPONENT_BYTES {
            return Err(ModuleError::Invalid("the plugin is too large".into()));
        }
        let mut modules = Vec::new();
        let mut is_component = false;
        for payload in Parser::new(0).parse_all(component) {
            match payload.map_err(invalid)? {
                Payload::Version { encoding, .. } if modules.is_empty() => {
                    is_component |= encoding == Encoding::Component;
                }
                Payload::ModuleSection {
                    unchecked_range, ..
                } => modules.push(unchecked_range),
                _ => {}
            }
        }
        if !is_component {
            return Err(ModuleError::Invalid("not a component".into()));
        }
        let [range] = modules.as_slice() else {
            return Err(ModuleError::Modules(modules.len()));
        };
        let module = component
            .get(usize_range(range)?)
            .ok_or_else(|| ModuleError::Invalid("module out of range".into()))?;
        check_module(module)
    }
}

/// Validates the module and builds the capped copy.
fn check_module(module: &[u8]) -> Result<CoreModule, ModuleError> {
    let features = WasmFeatures::default() & !(WasmFeatures::THREADS | WasmFeatures::RELAXED_SIMD);
    Validator::new_with_features(features)
        .validate_all(module)
        .map_err(invalid)?;

    let mut imports = Vec::new();
    let mut memories = Vec::new();
    let mut memory_section = None;
    let mut exports: Vec<(String, ExternalKind)> = Vec::new();
    for payload in Parser::new(0).parse_all(module) {
        match payload.map_err(invalid)? {
            Payload::ImportSection(s) => {
                for i in s.into_imports() {
                    let i = i.map_err(invalid)?;
                    let what = match i.ty {
                        TypeRef::Func(_) | TypeRef::FuncExact(_) => "function",
                        TypeRef::Table(_) => "table",
                        TypeRef::Memory(_) => "memory",
                        TypeRef::Global(_) => "global",
                        TypeRef::Tag(_) => "tag",
                    };
                    let name: String = format!("{}::{} ({what})", i.module, i.name)
                        .chars()
                        .take(128)
                        .collect();
                    imports.push(crate::validate::sanitise_message(&name));
                }
            }
            Payload::MemorySection(s) => {
                memory_section = Some(s.range());
                for m in s {
                    memories.push(m.map_err(invalid)?);
                }
            }
            Payload::ExportSection(s) => {
                for e in s {
                    let e = e.map_err(invalid)?;
                    exports.push((e.name.to_owned(), e.kind));
                }
            }
            _ => {}
        }
    }
    if !imports.is_empty() {
        return Err(ModuleError::Imports(imports));
    }
    let [memory] = memories.as_slice() else {
        return Err(ModuleError::Memory(format!(
            "{} memories (exactly one is needed)",
            memories.len()
        )));
    };
    if memory.memory64 || memory.shared {
        return Err(ModuleError::Memory("64-bit or shared memory".into()));
    }
    if memory.initial > MAX_MEMORY_PAGES {
        return Err(ModuleError::Memory(
            "its initial size is over the 256 MB cap".into(),
        ));
    }
    let has = |name: &str, kind: ExternalKind| exports.iter().any(|(n, k)| n == name && *k == kind);
    if !has("memory", ExternalKind::Memory) || !has("cabi_realloc", ExternalKind::Func) {
        return Err(ModuleError::NotAPlugin);
    }

    let api = api_version(exports.iter().map(|(n, _)| n.as_str()))?;
    let kinds: Vec<Kind> = Kind::ALL
        .into_iter()
        .filter(|k| {
            functions(*k)
                .iter()
                .all(|f| has(&export_name(k.name(), &api, f), ExternalKind::Func))
        })
        .collect();
    if !has(&export_name("info", &api, "describe"), ExternalKind::Func) {
        return Err(ModuleError::NotAPlugin);
    }

    let max = memory
        .maximum
        .map_or(MAX_MEMORY_PAGES, |m| m.min(MAX_MEMORY_PAGES));
    let range = usize_range(&memory_section.ok_or(ModuleError::NotAPlugin)?)?;
    let wasm = replace_memory_section(module, range, memory.initial, max)?;
    Ok(CoreModule { wasm, api, kinds })
}

/// The functions a kind's interface exports (ADR 0009).
pub(crate) const fn functions(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Generator => &["schema", "generate"],
        Kind::Modifier => &["schema", "modify"],
        Kind::Exporter => &["format", "export"],
        Kind::Importer => &["extensions", "import"],
        Kind::Validator => &["schema", "validate"],
    }
}

/// `locus:plugin/<interface>@<version>#<function>`, wit-bindgen's export name.
pub(crate) fn export_name(interface: &str, api: &str, function: &str) -> String {
    format!("{PACKAGE}/{interface}@{api}#{function}")
}

/// The API version of the `locus:plugin` exports; one version, supported major.
fn api_version<'a>(names: impl Iterator<Item = &'a str>) -> Result<String, ModuleError> {
    let supported = crate::manifest::supported_list();
    let mut found: Option<String> = None;
    for name in names {
        let Some(rest) = name.strip_prefix(PACKAGE).and_then(|r| r.strip_prefix('/')) else {
            continue;
        };
        let version = rest
            .split_once('@')
            .and_then(|(_, v)| v.split_once('#'))
            .map_or("", |(v, _)| v)
            .to_owned();
        let major = version
            .split('.')
            .next()
            .and_then(|m| m.parse::<u64>().ok());
        if !major.is_some_and(|m| SUPPORTED_API_MAJORS.contains(&m)) {
            return Err(ModuleError::UnsupportedApi {
                found: if version.is_empty() {
                    "(unversioned)".into()
                } else {
                    crate::validate::sanitise_message(&version)
                },
                supported,
            });
        }
        if found.as_ref().is_some_and(|f| *f != version) {
            return Err(ModuleError::UnsupportedApi {
                found: "a mix of versions".into(),
                supported,
            });
        }
        found = Some(version);
    }
    found.ok_or(ModuleError::NotAPlugin)
}

fn usize_range(r: &std::ops::Range<u64>) -> Result<std::ops::Range<usize>, ModuleError> {
    let to =
        |v: u64| usize::try_from(v).map_err(|_| ModuleError::Invalid("offset out of range".into()));
    Ok(to(r.start)?..to(r.end)?)
}

fn leb(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// The module with its memory section replaced by one memory of `initial`
/// to `max` pages. Every other byte is copied as it was.
fn replace_memory_section(
    module: &[u8],
    range: std::ops::Range<usize>,
    initial: u64,
    max: u64,
) -> Result<Vec<u8>, ModuleError> {
    // `range` is the section's contents; its header (id + size) sits just
    // before. Find the header start by walking back over the size LEB.
    let mut body = Vec::new();
    leb(&mut body, 1); // one memory
    body.push(0x01); // limits: has a maximum
    leb(&mut body, initial);
    leb(&mut body, max);
    let mut start = range.start;
    // The size field is 1–5 bytes, preceded by the id byte 5.
    let header = (1..=5)
        .find_map(|n| {
            let s = range.start.checked_sub(n + 1)?;
            let (&id, size) = (module.get(s)?, module.get(s + 1..range.start)?);
            let mut v = 0u64;
            for (i, b) in size.iter().enumerate() {
                v |= u64::from(b & 0x7f) << (7 * i);
                if (b & 0x80 == 0) != (i == size.len() - 1) {
                    return None;
                }
            }
            (id == 5 && v == (range.end - range.start) as u64).then_some(s)
        })
        .ok_or_else(|| ModuleError::Invalid("memory section header".into()))?;
    start = start.min(header);
    let mut out = Vec::with_capacity(module.len());
    out.extend_from_slice(&module[..start]);
    out.push(5);
    leb(&mut out, body.len() as u64);
    out.extend_from_slice(&body);
    out.extend_from_slice(&module[range.end..]);
    Ok(out)
}
