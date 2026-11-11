// SPDX-License-Identifier: AGPL-3.0-or-later

//! `locus-cli plugin …` (SPEC §15.7, §15.8).

use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};

use crate::{CliError, write_file};
use locus_plugin_host::Host;

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
