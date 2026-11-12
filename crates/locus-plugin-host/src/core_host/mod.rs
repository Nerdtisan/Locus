// SPDX-License-Identifier: AGPL-3.0-or-later

//! The core-module plugin host (ADR 0016, #253): runs a plugin component's
//! core module on any WebAssembly engine, with the canonical ABI done here.
//!
//! The web build runs it in a Web Worker on the browser's engine. On desktop
//! the same code runs over wasmtime's core-module API ([`native`]), which is
//! how the §15.8 suite checks both hosts give the same answers (#254).
//!
//! Like the desktop host: a fresh instance per call (no state survives, so
//! output depends only on the inputs, §15.5), nothing imported, and every
//! output goes through [`crate::validate`] before the caller sees it. The
//! engine enforces the memory cap (the module's maximum, [`module`]) and the
//! timeout (desktop: epoch; web: terminating the worker).

pub mod abi;
pub mod module;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;

pub use abi::{Instance, Val};
pub use module::{CoreModule, ModuleError};

use crate::error::CallError;
use crate::types::{
    ExportModel, Finding, FormatInfo, Generated, Kind, Param, ParamSpec, Path, PluginInfo,
};
use crate::validate;

/// Makes fresh instances of one compiled core module.
pub trait Engine {
    type Instance: Instance;
    fn instantiate(&self) -> Result<Self::Instance, CallError>;
}

/// A checked plugin on the core-module host: the counterpart of the desktop
/// `Plugin`, with the same calls and the same validation.
#[derive(Debug)]
pub struct CorePlugin<E> {
    engine: E,
    api: String,
    kinds: Vec<Kind>,
    info: PluginInfo,
}

impl<E: Engine> CorePlugin<E> {
    /// Wraps a compiled module and asks it to describe itself.
    pub fn new(engine: E, module: &CoreModule) -> Result<CorePlugin<E>, CallError> {
        let info = abi::describe(&mut engine.instantiate()?, &module.api)?;
        validate::info(&info)?;
        Ok(CorePlugin {
            engine,
            api: module.api.clone(),
            kinds: module.kinds.clone(),
            info,
        })
    }

    pub fn info(&self) -> &PluginInfo {
        &self.info
    }

    pub fn kinds(&self) -> &[Kind] {
        &self.kinds
    }

    pub fn api_version(&self) -> &str {
        &self.api
    }

    fn fresh(&self, kind: Kind) -> Result<E::Instance, CallError> {
        if !self.kinds.contains(&kind) {
            return Err(CallError::NotProvided(kind.name()));
        }
        self.engine.instantiate()
    }

    fn schema(&self, kind: Kind) -> Result<Vec<ParamSpec>, CallError> {
        let specs = abi::schema(&mut self.fresh(kind)?, &self.api, kind.name())?;
        validate::schema(&specs)?;
        Ok(specs)
    }

    pub fn generator_schema(&self) -> Result<Vec<ParamSpec>, CallError> {
        self.schema(Kind::Generator)
    }

    pub fn generate(&self, params: &[Param]) -> Result<Generated, CallError> {
        let g = abi::generate(&mut self.fresh(Kind::Generator)?, &self.api, params)?;
        validate::generated(&g)?;
        Ok(g)
    }

    pub fn modifier_schema(&self) -> Result<Vec<ParamSpec>, CallError> {
        self.schema(Kind::Modifier)
    }

    pub fn modify(&self, input: &[Path], params: &[Param]) -> Result<Vec<Path>, CallError> {
        validate::paths(input).map_err(|m| CallError::Host(m.0))?;
        let out = abi::modify(&mut self.fresh(Kind::Modifier)?, &self.api, input, params)?;
        validate::paths(&out)?;
        Ok(out)
    }

    pub fn export_format(&self) -> Result<FormatInfo, CallError> {
        let f = abi::format(&mut self.fresh(Kind::Exporter)?, &self.api)?;
        validate::format_info(&f)?;
        Ok(f)
    }

    pub fn export(&self, model: &ExportModel) -> Result<Vec<u8>, CallError> {
        validate::export_model(model).map_err(|m| CallError::Host(m.0))?;
        let bytes = abi::export(&mut self.fresh(Kind::Exporter)?, &self.api, model)?;
        validate::export_bytes(&bytes)?;
        Ok(bytes)
    }

    pub fn import_extensions(&self) -> Result<Vec<String>, CallError> {
        let exts = abi::extensions(&mut self.fresh(Kind::Importer)?, &self.api)?;
        validate::extensions(&exts)?;
        Ok(exts)
    }

    pub fn import(&self, bytes: &[u8]) -> Result<Vec<Path>, CallError> {
        if bytes.len() > validate::MAX_IMPORT_BYTES {
            return Err(CallError::Host(format!(
                "the file is larger than {} MB",
                validate::MAX_IMPORT_BYTES / (1024 * 1024)
            )));
        }
        let out = abi::import(&mut self.fresh(Kind::Importer)?, &self.api, bytes)?;
        validate::imported(&out)?;
        Ok(out)
    }

    pub fn validator_schema(&self) -> Result<Vec<ParamSpec>, CallError> {
        self.schema(Kind::Validator)
    }

    pub fn validate(
        &self,
        model: &ExportModel,
        params: &[Param],
    ) -> Result<Vec<Finding>, CallError> {
        validate::export_model(model).map_err(|m| CallError::Host(m.0))?;
        let found = abi::validate(&mut self.fresh(Kind::Validator)?, &self.api, model, params)?;
        validate::findings(&found, model)?;
        Ok(found)
    }
}
