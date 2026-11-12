// SPDX-License-Identifier: AGPL-3.0-or-later

//! The wasmtime runtime (§15.1, §15.4, ADR 0009, #181).
//!
//! Sandbox, by construction:
//! - **Nothing is linked.** The linker is empty, and a component that imports
//!   anything at all is rejected at load time, so there is no filesystem,
//!   network, clock, environment or randomness to reach (§15.4).
//! - **Fresh instance per call.** No state survives between calls, which also
//!   keeps output a function of the inputs (§15.5).
//! - **Limits per call:** fuel (a deterministic CPU budget), an epoch deadline
//!   (wall-clock timeout), and a memory cap across all of the instance's
//!   memories (default 256 MB). Running out of any of them aborts the call
//!   with an error; it never takes the app down.
//! - **Untrusted output:** everything a plugin returns is validated before
//!   it is handed back (see [`crate::validate`]).
//!
//! Callers get either validated data or a [`CallError`]; the document is
//! never touched here (§15.3), so a failed call leaves it unchanged.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use wasmtime::component::types::ComponentItem;
use wasmtime::component::{Component, InstancePre, Linker};
use wasmtime::{Config, Engine, ResourceLimiter, Store, Trap};

use crate::api::{PACKAGE as API_PACKAGE, SUPPORTED_API_MAJORS};
use crate::types::{
    Compensation, Constraint, Entity, ExportModel, Finding, FormatInfo, Generated, Kind, Operation,
    Param, ParamKind, ParamSpec, ParamValue, Path, PluginFailure, PluginInfo, Segment, Severity,
    Shape, Vec2,
};
use crate::validate::{self, Malformed};

mod bindings {
    // One world per kind (ADR 0009). The modifier and exporter worlds reuse
    // the generator world's shared `types` and `params` modules.
    pub mod generator {
        wasmtime::component::bindgen!({
            path: "../locus-plugin-api/wit",
            world: "generator-plugin",
        });
    }
    pub mod modifier {
        wasmtime::component::bindgen!({
            path: "../locus-plugin-api/wit",
            world: "modifier-plugin",
            with: {
                "locus:plugin/types": super::generator::locus::plugin::types,
                "locus:plugin/params": super::generator::locus::plugin::params,
            },
        });
    }
    pub mod exporter {
        wasmtime::component::bindgen!({
            path: "../locus-plugin-api/wit",
            world: "exporter-plugin",
            with: {
                "locus:plugin/types": super::generator::locus::plugin::types,
            },
        });
    }
    pub mod importer {
        wasmtime::component::bindgen!({
            path: "../locus-plugin-api/wit",
            world: "importer-plugin",
            with: {
                "locus:plugin/types": super::generator::locus::plugin::types,
            },
        });
    }
    pub mod validator {
        wasmtime::component::bindgen!({
            path: "../locus-plugin-api/wit",
            world: "validator-plugin",
            with: {
                "locus:plugin/types": super::generator::locus::plugin::types,
                "locus:plugin/params": super::generator::locus::plugin::params,
            },
        });
    }
}

use bindings::exporter::exports::locus::plugin::exporter as wexp;
use bindings::generator::exports::locus::plugin::generator as wgen;
use bindings::generator::exports::locus::plugin::info as winfo;
use bindings::generator::locus::plugin::params as wparams;
use bindings::generator::locus::plugin::types as wtypes;
use bindings::importer::exports::locus::plugin::importer as wimp;
use bindings::modifier::exports::locus::plugin::modifier as wmod;
use bindings::validator::exports::locus::plugin::validator as wval;

/// Largest component accepted, before compiling it.
pub const MAX_COMPONENT_BYTES: usize = 64 * 1024 * 1024;

/// How often the epoch ticks; timeouts are rounded up to this.
const TICK: Duration = Duration::from_millis(10);

/// Per-call resource limits (§15.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Fuel per call: roughly one unit per WebAssembly instruction. This is
    /// the deterministic CPU limit: the same call on the same inputs runs out
    /// at the same point on every machine.
    pub fuel: u64,
    /// Total linear memory across the instance, in bytes.
    pub memory_bytes: usize,
    /// Wall-clock limit per call.
    pub timeout: Duration,
}

impl Default for Limits {
    /// 256 MB and 5 s are the §15.4 defaults. The fuel budget is set so a
    /// well-behaved plugin hits the timeout first on a typical desktop.
    fn default() -> Self {
        Limits {
            fuel: 20_000_000_000,
            memory_bytes: 256 * 1024 * 1024,
            timeout: Duration::from_secs(5),
        }
    }
}

/// Why a component can't be loaded.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("the plugin is larger than {MAX_COMPONENT_BYTES} bytes")]
    TooLarge,
    #[error("not a valid WebAssembly component: {0}")]
    Invalid(String),
    /// The sandbox: plugins may import nothing (§15.4, ADR 0009).
    #[error("the plugin asks for host access it can't have: {}", .0.join(", "))]
    Imports(Vec<String>),
    #[error("the plugin doesn't export the Locus plugin interface")]
    NotAPlugin,
    #[error("the plugin targets plugin API {found}; this Locus supports {supported}")]
    UnsupportedApi { found: String, supported: String },
    #[error("the plugin provides no kind this Locus can run")]
    NoSupportedKind,
    #[error("the plugin failed while describing itself: {0}")]
    Describe(#[source] CallError),
}

/// Why a call produced no result. Every variant leaves the document unchanged.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("the plugin used more than its CPU budget")]
    CpuLimit,
    #[error("the plugin took longer than {0:?}")]
    Timeout(Duration),
    #[error("the plugin needed more than {} MB of memory", .0 / (1024 * 1024))]
    MemoryLimit(usize),
    /// A trap: a panic, `unreachable`, out-of-bounds access, stack overflow…
    #[error("the plugin crashed ({0})")]
    Trap(String),
    /// The plugin returned data that failed validation, or that the host
    /// couldn't even read (bad pointers, invalid UTF-8, …).
    #[error("the plugin returned malformed output: {0}")]
    Malformed(String),
    /// The plugin reported an error itself.
    #[error(transparent)]
    Plugin(PluginFailure),
    #[error("the plugin doesn't provide a {0}")]
    NotProvided(&'static str),
    #[error("plugin host error: {0}")]
    Host(String),
}

impl From<Malformed> for CallError {
    fn from(m: Malformed) -> Self {
        CallError::Malformed(m.0)
    }
}

/// Owns the wasmtime engine and the epoch ticker. Create one per process
/// and load plugins from it.
pub struct Host {
    engine: Engine,
    limits: Limits,
    ticker: Arc<Ticker>,
}

/// Stops the epoch thread once the host and every plugin are gone, so a
/// plugin outliving its `Host` still has a working timeout.
struct Ticker(Arc<AtomicBool>);

impl Drop for Ticker {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("limits", &self.limits)
            .finish()
    }
}

impl Host {
    pub fn new() -> Result<Host, CallError> {
        Host::with_limits(Limits::default())
    }

    pub fn with_limits(limits: Limits) -> Result<Host, CallError> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .consume_fuel(true)
            .epoch_interruption(true)
            // Determinism (§15.5): identical NaN bits and relaxed-SIMD results
            // on every CPU.
            .cranelift_nan_canonicalization(true)
            .relaxed_simd_deterministic(true);
        let engine = Engine::new(&config).map_err(|e| CallError::Host(e.to_string()))?;

        let stop = Arc::new(AtomicBool::new(false));
        let (tick_engine, tick_stop) = (engine.clone(), stop.clone());
        thread::Builder::new()
            .name("locus-plugin-epoch".into())
            .spawn(move || {
                while !tick_stop.load(Ordering::Relaxed) {
                    thread::sleep(TICK);
                    tick_engine.increment_epoch();
                }
            })
            .map_err(|e| CallError::Host(e.to_string()))?;
        Ok(Host {
            engine,
            limits,
            ticker: Arc::new(Ticker(stop)),
        })
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Compiles and checks a component, then asks it to describe itself.
    pub fn load(&self, wasm: &[u8]) -> Result<Plugin, LoadError> {
        if wasm.len() > MAX_COMPONENT_BYTES {
            return Err(LoadError::TooLarge);
        }
        // `from_binary`, never `new`: installed plugins are binaries, not WAT.
        let component = Component::from_binary(&self.engine, wasm)
            .map_err(|e| LoadError::Invalid(e.to_string()))?;
        let ty = component.component_type();

        // Interfaces used only for their types (`types`, `params`) appear as
        // imports that carry no functions. Anything else is host access.
        let imports: Vec<String> = ty
            .imports(&self.engine)
            .filter(|(_, item)| !types_only(&self.engine, &item.ty))
            .map(|(n, _)| n.to_owned())
            .collect();
        if !imports.is_empty() {
            return Err(LoadError::Imports(imports));
        }

        let api = api_version(ty.exports(&self.engine))?;
        let pre = Linker::<State>::new(&self.engine)
            .instantiate_pre(&component)
            .map_err(|e| LoadError::Invalid(e.to_string()))?;
        let info_idx = winfo::GuestIndices::new(&pre).map_err(|_| LoadError::NotAPlugin)?;
        let kinds = Kinds {
            generator: wgen::GuestIndices::new(&pre).ok(),
            modifier: wmod::GuestIndices::new(&pre).ok(),
            exporter: wexp::GuestIndices::new(&pre).ok(),
            importer: wimp::GuestIndices::new(&pre).ok(),
            validator: wval::GuestIndices::new(&pre).ok(),
        };
        if kinds.provided().is_empty() {
            return Err(LoadError::NoSupportedKind);
        }

        let mut plugin = Plugin {
            engine: self.engine.clone(),
            _ticker: self.ticker.clone(),
            limits: self.limits,
            pre,
            info_idx,
            kinds,
            api,
            info: PluginInfo {
                id: String::new(),
                name: String::new(),
                version: String::new(),
            },
        };
        let info = plugin
            .call(|store, p, inst| {
                let g = p.info_idx.load(&mut *store, inst)?;
                let i = g.call_describe(&mut *store)?;
                Ok(Ok(PluginInfo {
                    id: i.id,
                    name: i.name,
                    version: i.version,
                }))
            })
            .and_then(|i| {
                validate::info(&i)?;
                Ok(i)
            })
            .map_err(LoadError::Describe)?;
        plugin.info = info;
        Ok(plugin)
    }
}

/// True for an import that grants nothing: a type, or an instance of types
/// only. Functions, resources, modules and components all grant something.
fn types_only(engine: &Engine, item: &ComponentItem) -> bool {
    match item {
        ComponentItem::Type(_) => true,
        ComponentItem::ComponentInstance(i) => i
            .exports(engine)
            .all(|(_, e)| matches!(e.ty, ComponentItem::Type(_))),
        _ => false,
    }
}

/// Finds the `locus:plugin` exports and checks their API major (§15.3).
fn api_version<'a, I>(exports: impl Iterator<Item = (&'a str, I)>) -> Result<String, LoadError> {
    let supported = crate::manifest::supported_list();
    let mut found: Option<String> = None;
    for (name, _) in exports {
        let Some(rest) = name
            .strip_prefix(API_PACKAGE)
            .and_then(|r| r.strip_prefix('/'))
        else {
            continue;
        };
        let version = rest.split_once('@').map_or("", |(_, v)| v).to_owned();
        let major = version
            .split('.')
            .next()
            .and_then(|m| m.parse::<u64>().ok());
        if !major.is_some_and(|m| SUPPORTED_API_MAJORS.contains(&m)) {
            return Err(LoadError::UnsupportedApi {
                found: if version.is_empty() {
                    "(unversioned)".into()
                } else {
                    version
                },
                supported,
            });
        }
        if found.as_ref().is_some_and(|f| *f != version) {
            return Err(LoadError::UnsupportedApi {
                found: format!("a mix of {} and {version}", found.unwrap_or_default()),
                supported,
            });
        }
        found = Some(version);
    }
    found.ok_or(LoadError::NotAPlugin)
}

struct Kinds {
    generator: Option<wgen::GuestIndices>,
    modifier: Option<wmod::GuestIndices>,
    exporter: Option<wexp::GuestIndices>,
    importer: Option<wimp::GuestIndices>,
    validator: Option<wval::GuestIndices>,
}

impl Kinds {
    fn provided(&self) -> Vec<Kind> {
        let mut k = Vec::new();
        if self.generator.is_some() {
            k.push(Kind::Generator);
        }
        if self.modifier.is_some() {
            k.push(Kind::Modifier);
        }
        if self.exporter.is_some() {
            k.push(Kind::Exporter);
        }
        if self.importer.is_some() {
            k.push(Kind::Importer);
        }
        if self.validator.is_some() {
            k.push(Kind::Validator);
        }
        k
    }
}

/// Per-call store data: the memory limiter (§15.4).
struct State {
    cap: usize,
    used: usize,
    exceeded: bool,
}

impl ResourceLimiter for State {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        // Track the total across every memory in the instance, so a plugin
        // can't get around the cap by declaring several memories.
        let next = self.used.saturating_add(desired.saturating_sub(current));
        if next > self.cap {
            self.exceeded = true;
            return Ok(false);
        }
        self.used = next;
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= 1_000_000)
    }

    fn instances(&self) -> usize {
        64
    }

    fn tables(&self) -> usize {
        64
    }

    fn memories(&self) -> usize {
        16
    }
}

/// A loaded, checked plugin component. Cheap to call repeatedly: each call
/// instantiates afresh from the precompiled component.
pub struct Plugin {
    engine: Engine,
    _ticker: Arc<Ticker>,
    limits: Limits,
    pre: InstancePre<State>,
    info_idx: winfo::GuestIndices,
    kinds: Kinds,
    api: String,
    info: PluginInfo,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugin")
            .field("info", &self.info)
            .field("kinds", &self.kinds())
            .field("api", &self.api)
            .finish_non_exhaustive()
    }
}

type Inner<T> = wasmtime::Result<Result<T, wtypes::PluginError>>;

impl Plugin {
    /// What the plugin said about itself (validated).
    pub fn info(&self) -> &PluginInfo {
        &self.info
    }

    /// The plugin kinds this component exports and this host can run.
    pub fn kinds(&self) -> Vec<Kind> {
        self.kinds.provided()
    }

    /// The `locus:plugin` version the component was built against.
    pub fn api_version(&self) -> &str {
        &self.api
    }

    /// Replaces the per-call limits, e.g. a shorter timeout for previews.
    pub fn set_limits(&mut self, limits: Limits) {
        self.limits = limits;
    }

    /// Runs `f` in a fresh, limited instance and maps every failure mode.
    fn call<T>(
        &self,
        f: impl FnOnce(&mut Store<State>, &Self, &wasmtime::component::Instance) -> Inner<T>,
    ) -> Result<T, CallError> {
        let mut store = Store::new(
            &self.engine,
            State {
                cap: self.limits.memory_bytes,
                used: 0,
                exceeded: false,
            },
        );
        store.limiter(|s| s);
        store
            .set_fuel(self.limits.fuel)
            .map_err(|e| CallError::Host(e.to_string()))?;
        store.epoch_deadline_trap();
        let ticks = self.limits.timeout.as_nanos().div_ceil(TICK.as_nanos()) + 1;
        store.set_epoch_deadline(u64::try_from(ticks).unwrap_or(u64::MAX));

        let result = self
            .pre
            .instantiate(&mut store)
            .and_then(|inst| f(&mut store, self, &inst));
        match result {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(CallError::Plugin(failure(e))),
            Err(e) => Err(self.classify(&store, &e)),
        }
    }

    fn classify(&self, store: &Store<State>, e: &wasmtime::Error) -> CallError {
        if store.data().exceeded {
            return CallError::MemoryLimit(self.limits.memory_bytes);
        }
        match e.downcast_ref::<Trap>() {
            Some(Trap::OutOfFuel) => CallError::CpuLimit,
            Some(Trap::Interrupt) => CallError::Timeout(self.limits.timeout),
            Some(t) => CallError::Trap(t.to_string()),
            // Not a trap: the host couldn't lift what the plugin returned
            // (out-of-bounds pointers, invalid UTF-8, bad discriminants, …).
            None => CallError::Malformed(validate::sanitise_message(&e.to_string())),
        }
    }

    // ── Generators ──────────────────────────────────────────────────────────

    pub fn generator_schema(&self) -> Result<Vec<ParamSpec>, CallError> {
        let idx = self
            .kinds
            .generator
            .as_ref()
            .ok_or(CallError::NotProvided("generator"))?;
        let specs = self.call(|store, _, inst| {
            Ok(Ok(idx.load(&mut *store, inst)?.call_schema(&mut *store)?))
        })?;
        let specs: Vec<ParamSpec> = specs.into_iter().map(from_spec).collect();
        validate::schema(&specs)?;
        Ok(specs)
    }

    pub fn generate(&self, params: &[Param]) -> Result<Generated, CallError> {
        let idx = self
            .kinds
            .generator
            .as_ref()
            .ok_or(CallError::NotProvided("generator"))?;
        let params: Vec<wparams::Param> = params.iter().map(to_param).collect();
        let g = self.call(|store, _, inst| {
            idx.load(&mut *store, inst)?
                .call_generate(&mut *store, &params)
        })?;
        let g = from_generated(g);
        validate::generated(&g)?;
        Ok(g)
    }

    // ── Modifiers ───────────────────────────────────────────────────────────

    pub fn modifier_schema(&self) -> Result<Vec<ParamSpec>, CallError> {
        let idx = self
            .kinds
            .modifier
            .as_ref()
            .ok_or(CallError::NotProvided("modifier"))?;
        let specs = self.call(|store, _, inst| {
            Ok(Ok(idx.load(&mut *store, inst)?.call_schema(&mut *store)?))
        })?;
        let specs: Vec<ParamSpec> = specs.into_iter().map(from_spec).collect();
        validate::schema(&specs)?;
        Ok(specs)
    }

    pub fn modify(&self, input: &[Path], params: &[Param]) -> Result<Vec<Path>, CallError> {
        let idx = self
            .kinds
            .modifier
            .as_ref()
            .ok_or(CallError::NotProvided("modifier"))?;
        validate::paths(input).map_err(|m| CallError::Host(m.0))?;
        let input: Vec<wtypes::Path> = input.iter().map(to_path).collect();
        let params: Vec<wparams::Param> = params.iter().map(to_param).collect();
        let out = self.call(|store, _, inst| {
            idx.load(&mut *store, inst)?
                .call_modify(&mut *store, &input, &params)
        })?;
        let out: Vec<Path> = out.into_iter().map(from_path).collect();
        validate::paths(&out)?;
        Ok(out)
    }

    // ── Exporters ───────────────────────────────────────────────────────────

    pub fn export_format(&self) -> Result<FormatInfo, CallError> {
        let idx = self
            .kinds
            .exporter
            .as_ref()
            .ok_or(CallError::NotProvided("exporter"))?;
        let f = self.call(|store, _, inst| {
            Ok(Ok(idx.load(&mut *store, inst)?.call_format(&mut *store)?))
        })?;
        let f = FormatInfo {
            id: f.id,
            name: f.name,
            extension: f.extension,
            mime: f.mime,
        };
        validate::format_info(&f)?;
        Ok(f)
    }

    pub fn export(&self, model: &ExportModel) -> Result<Vec<u8>, CallError> {
        let idx = self
            .kinds
            .exporter
            .as_ref()
            .ok_or(CallError::NotProvided("exporter"))?;
        validate::export_model(model).map_err(|m| CallError::Host(m.0))?;
        let model = to_model(model);
        let bytes = self.call(|store, _, inst| {
            idx.load(&mut *store, inst)?
                .call_export(&mut *store, &model)
        })?;
        validate::export_bytes(&bytes)?;
        Ok(bytes)
    }

    // ── Importers (M4, #233) ────────────────────────────────────────────────

    /// The file extensions the importer accepts, lowercase, without dots.
    pub fn import_extensions(&self) -> Result<Vec<String>, CallError> {
        let idx = self
            .kinds
            .importer
            .as_ref()
            .ok_or(CallError::NotProvided("importer"))?;
        let exts = self.call(|store, _, inst| {
            Ok(Ok(idx
                .load(&mut *store, inst)?
                .call_extensions(&mut *store)?))
        })?;
        validate::extensions(&exts)?;
        Ok(exts)
    }

    /// File bytes (read by the host, §15.4) → reference geometry.
    pub fn import(&self, bytes: &[u8]) -> Result<Vec<Path>, CallError> {
        let idx = self
            .kinds
            .importer
            .as_ref()
            .ok_or(CallError::NotProvided("importer"))?;
        if bytes.len() > validate::MAX_IMPORT_BYTES {
            return Err(CallError::Host(format!(
                "the file is larger than {} MB",
                validate::MAX_IMPORT_BYTES / (1024 * 1024)
            )));
        }
        let out = self
            .call(|store, _, inst| idx.load(&mut *store, inst)?.call_import(&mut *store, bytes))?;
        let out: Vec<Path> = out.into_iter().map(from_path).collect();
        validate::imported(&out)?;
        Ok(out)
    }

    // ── Validators (M4, #234) ───────────────────────────────────────────────

    pub fn validator_schema(&self) -> Result<Vec<ParamSpec>, CallError> {
        let idx = self
            .kinds
            .validator
            .as_ref()
            .ok_or(CallError::NotProvided("validator"))?;
        let specs = self.call(|store, _, inst| {
            Ok(Ok(idx.load(&mut *store, inst)?.call_schema(&mut *store)?))
        })?;
        let specs: Vec<ParamSpec> = specs.into_iter().map(from_spec).collect();
        validate::schema(&specs)?;
        Ok(specs)
    }

    /// Runs the plugin's checks on the export model. Every layer and path
    /// index in the findings is checked against `model`.
    pub fn validate(
        &self,
        model: &ExportModel,
        params: &[Param],
    ) -> Result<Vec<Finding>, CallError> {
        let idx = self
            .kinds
            .validator
            .as_ref()
            .ok_or(CallError::NotProvided("validator"))?;
        validate::export_model(model).map_err(|m| CallError::Host(m.0))?;
        let wmodel = to_model(model);
        let params: Vec<wparams::Param> = params.iter().map(to_param).collect();
        let found = self.call(|store, _, inst| {
            idx.load(&mut *store, inst)?
                .call_validate(&mut *store, &wmodel, &params)
        })?;
        let found: Vec<Finding> = found
            .into_iter()
            .map(|f| Finding {
                severity: match f.severity {
                    wval::Severity::Error => Severity::Error,
                    wval::Severity::Warning => Severity::Warning,
                },
                message: f.message,
                layer: f.layer,
                path: f.path,
            })
            .collect();
        validate::findings(&found, model)?;
        Ok(found)
    }
}

// ── Conversions between the bindings and the host types ─────────────────────

fn failure(e: wtypes::PluginError) -> PluginFailure {
    match e {
        wtypes::PluginError::BadParameter(m) => {
            PluginFailure::BadParameter(validate::sanitise_message(&m))
        }
        wtypes::PluginError::Unsupported(m) => {
            PluginFailure::Unsupported(validate::sanitise_message(&m))
        }
        wtypes::PluginError::Failed(m) => PluginFailure::Failed(validate::sanitise_message(&m)),
    }
}

fn v(p: wtypes::Vec2) -> Vec2 {
    Vec2 { x: p.x, y: p.y }
}

fn wv(p: Vec2) -> wtypes::Vec2 {
    wtypes::Vec2 { x: p.x, y: p.y }
}

fn from_spec(s: wparams::ParamSpec) -> ParamSpec {
    use wparams::ParamKind as K;
    let kind = match s.kind {
        K::Number(n) => ParamKind::Number(crate::types::NumberSpec {
            unit: n.unit,
            min: n.min,
            max: n.max,
            default: n.default,
        }),
        K::Integer(n) => ParamKind::Integer(crate::types::IntegerSpec {
            min: n.min,
            max: n.max,
            default: n.default,
        }),
        K::Boolean(b) => ParamKind::Boolean(b),
        K::Choice(c) => ParamKind::Choice(crate::types::ChoiceSpec {
            options: c.options,
            default: c.default,
        }),
        K::Text(t) => ParamKind::Text(crate::types::TextSpec {
            default: t.default,
            max_length: t.max_length,
        }),
        K::DocParameter => ParamKind::DocParameter,
    };
    ParamSpec {
        key: s.key,
        label: s.label,
        help: s.help,
        kind,
    }
}

fn to_param(p: &Param) -> wparams::Param {
    use wparams::ParamValue as W;
    wparams::Param {
        key: p.key.clone(),
        value: match &p.value {
            ParamValue::Number(n) => W::Number(*n),
            ParamValue::Integer(n) => W::Integer(*n),
            ParamValue::Boolean(b) => W::Boolean(*b),
            ParamValue::Choice(c) => W::Choice(*c),
            ParamValue::Text(t) => W::Text(t.clone()),
        },
    }
}

fn from_generated(g: wgen::Generated) -> Generated {
    Generated {
        points: g.points.into_iter().map(v).collect(),
        entities: g
            .entities
            .into_iter()
            .map(|e| match e {
                wgen::Entity::Point(p) => Entity::Point(p),
                wgen::Entity::Line((a, b)) => Entity::Line(a, b),
                wgen::Entity::Circle((center, radius)) => Entity::Circle { center, radius },
                wgen::Entity::Arc((center, start, end)) => Entity::Arc { center, start, end },
            })
            .collect(),
        constraints: g
            .constraints
            .into_iter()
            .map(|c| match c {
                wgen::Constraint::Coincident((a, b)) => Constraint::Coincident(a, b),
                wgen::Constraint::Horizontal(a) => Constraint::Horizontal(a),
                wgen::Constraint::Vertical(a) => Constraint::Vertical(a),
                wgen::Constraint::Parallel((a, b)) => Constraint::Parallel(a, b),
                wgen::Constraint::Perpendicular((a, b)) => Constraint::Perpendicular(a, b),
                wgen::Constraint::Tangent((a, b)) => Constraint::Tangent(a, b),
                wgen::Constraint::Equal((a, b)) => Constraint::Equal(a, b),
                wgen::Constraint::Distance((a, b, d)) => Constraint::Distance(a, b, d),
                wgen::Constraint::Radius((a, r)) => Constraint::Radius(a, r),
            })
            .collect(),
    }
}

fn from_shape(s: wtypes::Shape) -> Shape {
    match s {
        wtypes::Shape::Line(l) => Shape::Line {
            a: v(l.a),
            b: v(l.b),
        },
        wtypes::Shape::Arc(a) => Shape::Arc {
            center: v(a.center),
            start: v(a.start),
            end: v(a.end),
        },
        wtypes::Shape::Circle(c) => Shape::Circle {
            center: v(c.center),
            radius: c.radius,
        },
    }
}

fn to_shape(s: &Shape) -> wtypes::Shape {
    match *s {
        Shape::Line { a, b } => wtypes::Shape::Line(wtypes::Line { a: wv(a), b: wv(b) }),
        Shape::Arc { center, start, end } => wtypes::Shape::Arc(wtypes::Arc {
            center: wv(center),
            start: wv(start),
            end: wv(end),
        }),
        Shape::Circle { center, radius } => wtypes::Shape::Circle(wtypes::Circle {
            center: wv(center),
            radius,
        }),
    }
}

fn from_path(p: wtypes::Path) -> Path {
    Path {
        segments: p
            .segments
            .into_iter()
            .map(|s| Segment {
                shape: from_shape(s.shape),
                reversed: s.reversed,
            })
            .collect(),
        closed: p.closed,
    }
}

fn to_path(p: &Path) -> wtypes::Path {
    wtypes::Path {
        segments: p
            .segments
            .iter()
            .map(|s| wtypes::Segment {
                shape: to_shape(&s.shape),
                reversed: s.reversed,
            })
            .collect(),
        closed: p.closed,
    }
}

fn to_operation(o: &Operation) -> wtypes::Operation {
    use wtypes::Operation as W;
    match *o {
        Operation::Cut {
            kerf_width,
            compensation,
            passes,
        } => W::Cut(wtypes::CutParams {
            kerf_width,
            compensation: match compensation {
                Compensation::None => wtypes::Compensation::None,
                Compensation::Inside => wtypes::Compensation::Inside,
                Compensation::Outside => wtypes::Compensation::Outside,
            },
            passes,
        }),
        Operation::Score => W::Score,
        Operation::Engrave {
            hatch_spacing,
            hatch_angle,
        } => W::Engrave(wtypes::EngraveParams {
            hatch_spacing,
            hatch_angle,
        }),
        Operation::Draw {
            pen_number,
            pen_width,
        } => W::Draw(wtypes::DrawParams {
            pen_number,
            pen_width,
        }),
        Operation::DragKnife { blade_offset } => {
            W::DragKnife(wtypes::DragKnifeParams { blade_offset })
        }
        Operation::Reference => W::Reference,
    }
}

fn to_model(m: &ExportModel) -> wtypes::ExportModel {
    wtypes::ExportModel {
        layers: m
            .layers
            .iter()
            .map(|l| wtypes::Layer {
                name: l.name.clone(),
                colour: l.colour,
                operation: to_operation(&l.operation),
                paths: l.paths.iter().map(to_path).collect(),
            })
            .collect(),
    }
}
