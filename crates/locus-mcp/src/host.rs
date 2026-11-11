// SPDX-License-Identifier: AGPL-3.0-or-later

//! What `locus-mcp` needs from its host: the document (through the command
//! API), plugins, exporters, and a human to approve things (SPEC §16.2,
//! §16.4).
//!
//! The document never leaves the thread that owns it. Tools run as [`Job`]s
//! sent to that thread: the GUI drains them in its frame loop, so AI edits go
//! through the GUI's own `Sketcher` and the canvas updates live (#229); the
//! headless CLI owns the document on a worker thread ([`LocalBackend`]).

use std::sync::Arc;
use std::sync::mpsc;

use locus_doc::generated::{GenPath, GeneratedGeometry, PluginRef};
use locus_doc::{Applied, Command, DocError, LayerId, Sketcher};
use serde_json::Value;

/// The document side of a session. Every mutation is a [`Command`] (§11).
pub trait Workspace {
    fn sketcher(&self) -> &Sketcher;
    /// Apply one command (and re-solve) as one undo step. The GUI routes this
    /// through its canvas so selection and views stay consistent.
    ///
    /// # Errors
    /// Whatever the document rejected; it is then unchanged.
    fn apply(&mut self, cmd: &Command) -> Result<Applied, DocError>;
    /// Where new geometry goes when the client names no layer: the GUI's
    /// current layer, else the first layer in export order.
    fn default_layer(&self) -> Option<LayerId> {
        None
    }
    /// Export formats: the built-ins plus exporter plugins.
    fn exporters(&self) -> &locus_export::Registry;
    /// Installed plugins, when the host loads any.
    fn plugins(&mut self) -> Option<&mut dyn Plugins> {
        None
    }
    /// Save to the file the user opened the session with (headless CLI only;
    /// never an arbitrary path, §16.4). `None` when the host can't.
    fn save(&mut self) -> Option<Result<(), String>> {
        None
    }
}

/// One installed plugin, as listed to clients.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    /// `generator`, `modifier`, `exporter`, …
    pub kinds: Vec<String>,
    /// JSON Schema of the generator parameters, when it is a generator.
    pub generator_params: Option<Value>,
    /// JSON Schema of the modifier parameters, when it is a modifier.
    pub modifier_params: Option<Value>,
}

/// Installed plugins (SPEC §15). Implemented over `locus-plugin-host`.
/// Plugins never see MCP: this is the host calling plugins, not the reverse
/// (§16.4).
pub trait Plugins {
    fn list(&mut self) -> Vec<PluginEntry>;
    /// Run a generator with `params` (keys left out take defaults).
    ///
    /// # Errors
    /// Unknown plugin, bad parameters, or the plugin failed.
    fn generate(
        &mut self,
        id: &str,
        params: &serde_json::Map<String, Value>,
    ) -> Result<(PluginRef, GeneratedGeometry), String>;
    /// Run a modifier on `input`.
    ///
    /// # Errors
    /// Unknown plugin, bad parameters, or the plugin failed.
    fn modify(
        &mut self,
        id: &str,
        params: &serde_json::Map<String, Value>,
        input: &[GenPath],
    ) -> Result<(PluginRef, GeneratedGeometry), String>;
}

/// Work to run on the thread that owns the document.
pub type Job = Box<dyn FnOnce(&mut dyn Workspace) + Send>;

/// Runs [`Job`]s against the document, in order.
pub trait Backend: Send + Sync {
    /// Queue a job. Returns `false` when the host is gone (the job is dropped).
    fn submit(&self, job: Job) -> bool;
}

/// What the user is asked to approve (SPEC §16.4 approval prompt).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub tool: String,
    pub scope: crate::policy::Scope,
    /// A one-line summary of what the AI wants to do, built from the request
    /// ("add 4 entities, 4 constraints on layer 1").
    pub summary: String,
}

/// Asks the user. The GUI shows a prompt; the headless CLI never asks (its
/// grants are the command-line scopes).
pub trait Approver: Send + Sync {
    /// Ask, and send the answer on `reply` (dropping it counts as "no").
    fn ask(&self, request: ApprovalRequest, reply: tokio::sync::oneshot::Sender<bool>);
}

/// Refuses everything that needs asking: for hosts without a user.
#[derive(Debug, Default, Clone, Copy)]
pub struct NeverApprove;

impl Approver for NeverApprove {
    fn ask(&self, _: ApprovalRequest, reply: tokio::sync::oneshot::Sender<bool>) {
        let _ = reply.send(false);
    }
}

/// Owns a [`Workspace`] on a worker thread (headless CLI, tests).
#[derive(Debug, Clone)]
pub struct LocalBackend {
    tx: mpsc::Sender<Job>,
}

impl LocalBackend {
    /// Start the worker. It stops when every clone of the backend is dropped.
    ///
    /// # Errors
    /// The thread could not be spawned.
    pub fn spawn<W: Workspace + Send + 'static>(mut ws: W) -> std::io::Result<Arc<Self>> {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("locus-mcp-doc".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job(&mut ws);
                }
            })?;
        Ok(Arc::new(Self { tx }))
    }
}

impl Backend for LocalBackend {
    fn submit(&self, job: Job) -> bool {
        self.tx.send(job).is_ok()
    }
}

/// A plain workspace over a [`Sketcher`]: headless hosts and tests.
#[derive(Debug)]
pub struct Headless {
    pub sketcher: Sketcher,
    pub exporters: locus_export::Registry,
    pub plugins: Option<Box<dyn PluginsDebug>>,
    /// Called by the `save` tool with the RON text; `None` hides the tool.
    pub saver: Option<Box<dyn SaverDebug>>,
}

/// [`Plugins`] that can be debug-printed (for [`Headless`]'s derive).
pub trait PluginsDebug: Plugins + Send + std::fmt::Debug {}
impl<T: Plugins + Send + std::fmt::Debug> PluginsDebug for T {}

/// Writes the saved document somewhere the user chose.
pub trait Saver: Send {
    /// # Errors
    /// The write failed; the old file must be left intact.
    fn save(&mut self, text: &str) -> Result<(), String>;
}
pub trait SaverDebug: Saver + std::fmt::Debug {}
impl<T: Saver + std::fmt::Debug> SaverDebug for T {}

impl Headless {
    #[must_use]
    pub fn new(doc: locus_doc::Document) -> Self {
        Self {
            sketcher: Sketcher::new(doc),
            exporters: locus_export::Registry::builtin(),
            plugins: None,
            saver: None,
        }
    }
}

impl Workspace for Headless {
    fn sketcher(&self) -> &Sketcher {
        &self.sketcher
    }

    fn apply(&mut self, cmd: &Command) -> Result<Applied, DocError> {
        self.sketcher.apply(cmd)
    }

    fn exporters(&self) -> &locus_export::Registry {
        &self.exporters
    }

    fn plugins(&mut self) -> Option<&mut dyn Plugins> {
        match &mut self.plugins {
            Some(p) => Some(p.as_mut()),
            None => None,
        }
    }

    fn save(&mut self) -> Option<Result<(), String>> {
        let saver = self.saver.as_mut()?;
        Some(
            locus_doc::file::save(self.sketcher.document())
                .map_err(|e| e.to_string())
                .and_then(|text| saver.save(&text)),
        )
    }
}
