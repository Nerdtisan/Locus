// SPDX-License-Identifier: AGPL-3.0-or-later

//! The messages between the web app and its plugin worker (ADR 0016, #253).
//!
//! Each call goes to a fresh Web Worker as the plugin component plus one
//! [`Request`] (JSON); the worker answers with one `Result<Reply, String>`
//! (JSON) and is then terminated, or terminated early on timeout. [`handle`]
//! is the worker's side, written against any [`Engine`] so the same code is
//! tested on desktop over wasmtime.
//!
//! Replies are validated in the worker ([`CorePlugin`]) and, being untrusted
//! data that crossed a boundary, are parsed with bounds on the app side too
//! ([`parse_reply`]).

use serde::{Deserialize, Serialize};

use super::{CoreModule, CorePlugin, Engine};
use crate::error::CallError;
use crate::types::{
    ExportModel, Finding, FormatInfo, Generated, Kind, Param, ParamSpec, Path, PluginInfo,
};

/// One plugin call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// Load and describe: the install check.
    Describe,
    Schema(Kind),
    Generate(Vec<Param>),
    Modify {
        input: Vec<Path>,
        params: Vec<Param>,
    },
    Format,
    Export(ExportModel),
    Extensions,
    Import(Vec<u8>),
    Validate {
        model: ExportModel,
        params: Vec<Param>,
    },
}

/// A successful call's output, already validated by the worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Reply {
    Described {
        info: PluginInfo,
        api: String,
        kinds: Vec<Kind>,
    },
    Schema(Vec<ParamSpec>),
    Generated(Generated),
    Paths(Vec<Path>),
    Format(FormatInfo),
    Bytes(Vec<u8>),
    Extensions(Vec<String>),
    Findings(Vec<Finding>),
}

/// What a worker posts once it can take a call; the app sends the call only
/// after this, since messages sent earlier would be dropped.
pub const WORKER_READY: &str = "locus-plugin-worker:ready";

/// Largest reply accepted from a worker: the export cap, as JSON numbers.
pub const MAX_REPLY_BYTES: usize = 4 * crate::validate::MAX_EXPORT_BYTES;

/// Why a call failed, as it crosses to the app: the `CallError` variant (or
/// `Refused` for a component that didn't pass the checks, `Timeout` when the
/// app ended the worker, `Worker` when the worker itself failed) and the
/// message for the user. The kind lets #254 compare failures across hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireError {
    pub kind: String,
    pub message: String,
}

impl WireError {
    pub fn new(kind: &str, message: &str) -> WireError {
        WireError {
            kind: kind.to_owned(),
            message: crate::validate::sanitise_message(message),
        }
    }
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<CallError> for WireError {
    fn from(e: CallError) -> WireError {
        WireError::new(e.kind(), &e.to_string())
    }
}

/// The worker's side: check the component, run one call, report.
pub fn handle<E: Engine>(
    engine: impl FnOnce(&CoreModule) -> Result<E, CallError>,
    component: &[u8],
    request: Request,
) -> Result<Reply, WireError> {
    let module = CoreModule::from_component(component)
        .map_err(|e| WireError::new("Refused", &e.to_string()))?;
    let plugin = CorePlugin::new(engine(&module)?, &module)?;
    let reply = match request {
        Request::Describe => Reply::Described {
            info: plugin.info().clone(),
            api: plugin.api_version().to_owned(),
            kinds: plugin.kinds().to_vec(),
        },
        Request::Schema(Kind::Generator) => Reply::Schema(plugin.generator_schema()?),
        Request::Schema(Kind::Modifier) => Reply::Schema(plugin.modifier_schema()?),
        Request::Schema(Kind::Validator) => Reply::Schema(plugin.validator_schema()?),
        Request::Schema(k) => {
            return Err(WireError::new(
                "NotProvided",
                &format!("a {} has no parameters", k.name()),
            ));
        }
        Request::Generate(params) => Reply::Generated(plugin.generate(&params)?),
        Request::Modify { input, params } => Reply::Paths(plugin.modify(&input, &params)?),
        Request::Format => Reply::Format(plugin.export_format()?),
        Request::Export(model) => Reply::Bytes(plugin.export(&model)?),
        Request::Extensions => Reply::Extensions(plugin.import_extensions()?),
        Request::Import(bytes) => Reply::Paths(plugin.import(&bytes)?),
        Request::Validate { model, params } => Reply::Findings(plugin.validate(&model, &params)?),
    };
    Ok(reply)
}

/// The worker's answer as JSON.
#[must_use]
pub fn reply_json(r: &Result<Reply, WireError>) -> String {
    serde_json::to_string(r).unwrap_or_else(|_| {
        r#"{"Err":{"kind":"Worker","message":"the plugin's reply couldn't be encoded"}}"#.to_owned()
    })
}

/// Reads a worker's answer, bounded; anything unreadable is an error.
pub fn parse_reply(json: &str) -> Result<Reply, WireError> {
    if json.len() > MAX_REPLY_BYTES {
        return Err(WireError::new("Worker", "the plugin's reply is too large"));
    }
    serde_json::from_str::<Result<Reply, WireError>>(json)
        .map_err(|_| WireError::new("Worker", "the plugin's reply is unreadable"))?
        .map_err(|e| WireError::new(&e.kind, &e.message))
}
