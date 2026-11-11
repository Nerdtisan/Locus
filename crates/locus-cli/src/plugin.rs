// SPDX-License-Identifier: AGPL-3.0-or-later

//! `locus-cli plugin …` (SPEC §15.7, §15.8).

use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};

use crate::{CliError, write_file};
use locus_plugin_host::Host;
use locus_plugin_host::archive::{Archive, MAX_ARCHIVE_BYTES};
use locus_plugin_host::export::PluginExporter;
use locus_plugin_host::registry::{Registry, verify};
use locus_plugin_host::types::Kind;

/// The built-in export formats plus the exporter plugins named on the
/// command line (#184): `.locusplugin` files, and every exporter installed
/// in `dir`. A plugin can't replace a built-in format.
pub fn exporters(files: &[PathBuf], dir: Option<&Path>) -> Result<locus_export::Registry> {
    let mut reg = locus_export::Registry::builtin();
    if files.is_empty() && dir.is_none() {
        return Ok(reg);
    }
    let host = Host::new().map_err(|e| anyhow!("{e}"))?;
    let mut add = |p: locus_plugin_host::Plugin| -> Result<()> {
        if !p.kinds().contains(&Kind::Exporter) {
            return Ok(());
        }
        let e = PluginExporter::new(p).map_err(|e| anyhow!("exporter plugin: {e}"))?;
        let id = locus_export::Exporter::id(&e).to_owned();
        if !reg.add(Box::new(e)) {
            return Err(anyhow!(
                "an exporter plugin uses the format id `{id}`, which is taken"
            ));
        }
        Ok(())
    };
    for f in files {
        let bytes = read_limited(f, MAX_ARCHIVE_BYTES, "plugin file")?;
        let archive = Archive::parse(&bytes).map_err(|e| anyhow!("plugin file: {e}"))?;
        add(verify(&host, &archive).map_err(|e| anyhow!("plugin file: {e}"))?)?;
    }
    if let Some(dir) = dir {
        let installed = Registry::new(dir);
        for i in installed.list() {
            if i.manifest.kinds.contains(&Kind::Exporter) {
                let (_, p) = installed
                    .load(&host, &i.manifest.id)
                    .map_err(|e| anyhow!("installed plugin: {e}"))?;
                add(p)?;
            }
        }
    }
    Ok(reg)
}

/// Reads a file of at most `limit` bytes. Errors never name the file.
pub fn read_limited(path: &Path, limit: usize, what: &str) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut data = Vec::new();
    std::fs::File::open(path)
        .with_context(|| format!("cannot read the {what}"))?
        .take(limit as u64 + 1)
        .read_to_end(&mut data)
        .with_context(|| format!("cannot read the {what}"))?;
    if data.len() > limit {
        return Err(anyhow!("the {what} is too large"));
    }
    Ok(data)
}

fn text(path: &Path, limit: usize, what: &str) -> Result<String> {
    String::from_utf8(read_limited(path, limit, what)?)
        .map_err(|_| anyhow!("the {what} is not UTF-8"))
}

pub fn run(args: &[String]) -> Result<(), CliError> {
    match args.first().map(String::as_str) {
        Some("pack") => pack(&args[1..]),
        Some(other) => Err(CliError::Usage(format!("unknown plugin command `{other}`"))),
        None => Err(CliError::Usage("plugin needs a command".into())),
    }
}

#[derive(Default)]
struct PackOpts {
    manifest: Option<PathBuf>,
    wasm: Option<PathBuf>,
    out: Option<PathBuf>,
    readme: Option<PathBuf>,
    icon: Option<PathBuf>,
}

fn pack(args: &[String]) -> Result<(), CliError> {
    let mut o = PackOpts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let slot = match a.as_str() {
            "--manifest" => &mut o.manifest,
            "--wasm" => &mut o.wasm,
            "--out" => &mut o.out,
            "--readme" => &mut o.readme,
            "--icon" => &mut o.icon,
            s => return Err(CliError::Usage(format!("unknown option `{s}`"))),
        };
        let v = it
            .next()
            .ok_or_else(|| CliError::Usage(format!("{a} needs a value")))?;
        *slot = Some(PathBuf::from(v));
    }
    let need = |p: Option<PathBuf>, flag: &str| {
        p.ok_or_else(|| CliError::Usage(format!("{flag} is required")))
    };
    let manifest = need(o.manifest, "--manifest")?;
    let wasm = need(o.wasm, "--wasm")?;
    let out = need(o.out, "--out")?;

    use locus_plugin_host::{MAX_COMPONENT_BYTES, manifest::MAX_MANIFEST_BYTES};
    let manifest = text(&manifest, MAX_MANIFEST_BYTES, "manifest")?;
    let wasm = read_limited(&wasm, MAX_COMPONENT_BYTES, "WebAssembly file")?;
    let readme = o
        .readme
        .map(|p| text(&p, 256 * 1024, "README"))
        .transpose()?;
    let icon = o
        .icon
        .map(|p| read_limited(&p, 1024 * 1024, "icon"))
        .transpose()?;

    let host = Host::new().map_err(|e| anyhow!("{e}"))?;
    let bytes =
        locus_plugin_host::pack::pack(&host, &manifest, &wasm, readme.as_deref(), icon.as_deref())
            .map_err(|e| anyhow!("cannot pack the plugin: {e}"))?;
    write_file(&out, &bytes)?;
    Ok(())
}
