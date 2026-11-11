// SPDX-License-Identifier: AGPL-3.0-or-later

//! `locus-cli mcp --doc file.locus` (SPEC §16.2, #228): the MCP server over
//! stdio, headless, against one file.
//!
//! The scopes on the command line are the user's grant and approval for
//! this session (there is no one to ask): read only unless more are given
//! (§16.4). `save` writes only to the `--doc` file, atomically; `export`
//! returns bytes and never writes a file.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, anyhow};
use locus_mcp::host::Saver;
use locus_mcp::plugins::RegistryPlugins;
use locus_mcp::{Grants, Headless, LocalBackend, LocusServer, NeverApprove, Options};

use crate::{CliError, plugin, read_document, write_file};

struct Opts {
    doc: PathBuf,
    read: bool,
    write: bool,
    export: bool,
    plugins: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut doc = None;
    let mut scopes = "read".to_owned();
    let mut plugins = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--doc" => doc = Some(PathBuf::from(value()?)),
            "--scopes" => scopes = value()?.clone(),
            "--plugins" => plugins = Some(PathBuf::from(value()?)),
            s => return Err(format!("unknown option `{s}`")),
        }
    }
    let (mut read, mut write, mut export) = (false, false, false);
    for s in scopes.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match s {
            "read" => read = true,
            "write" => write = true,
            "export" => export = true,
            other => return Err(format!("unknown scope `{other}` (read, write, export)")),
        }
    }
    Ok(Opts {
        doc: doc.ok_or("--doc is required")?,
        read,
        write,
        export,
        plugins,
    })
}

/// Saves to the session's own file, atomically (temp file + rename).
#[derive(Debug)]
struct FileSaver(PathBuf);

impl Saver for FileSaver {
    fn save(&mut self, text: &str) -> Result<(), String> {
        write_file(&self.0, text.as_bytes()).map_err(|e| format!("{e:#}"))
    }
}

pub fn run(args: &[String]) -> Result<(), CliError> {
    let o = parse(args).map_err(CliError::Usage)?;
    let doc = if Path::new(&o.doc).exists() {
        read_document(&o.doc)?
    } else {
        eprintln!("locus-cli mcp: the file does not exist yet; starting a new document");
        locus_doc::Document::new()
    };
    let mut ws = Headless::new(doc);
    ws.exporters = plugin::exporters(&[], o.plugins.as_deref())?;
    if let Some(dir) = &o.plugins {
        ws.plugins = Some(Box::new(RegistryPlugins::new(
            locus_plugin_host::registry::Registry::new(dir),
        )));
    }
    let can_save = o.write;
    ws.saver = Some(Box::new(FileSaver(o.doc)));
    let backend = LocalBackend::spawn(ws).context("cannot start the document thread")?;
    let server = LocusServer::new(
        backend,
        Arc::new(NeverApprove),
        Grants::headless(o.read, o.write, o.export),
        Options {
            save_tool: can_save,
        },
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;
    rt.block_on(locus_mcp::stdio::serve(server))
        .map_err(|e| CliError::Failed(anyhow!(e)))
}
