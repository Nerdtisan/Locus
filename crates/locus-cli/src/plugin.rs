// SPDX-License-Identifier: AGPL-3.0-or-later

//! `locus-cli plugin …` (SPEC §15.7, §15.8): pack, run.

use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};

use crate::{CliError, write_file};
use locus_doc::{Command, Document, Editor};
use locus_plugin_host::Host;
use locus_plugin_host::archive::{Archive, MAX_ARCHIVE_BYTES};
use locus_plugin_host::doc::{plugin_ref, resolve, to_cache};
use locus_plugin_host::export::PluginExporter;
use locus_plugin_host::registry::{Registry, verify};
use locus_plugin_host::types::{Kind, Param, ParamKind, ParamSpec, ParamValue};

/// The built-in export formats plus the exporter plugins named on the
/// command line (#184): `.locusplugin` files, and every exporter installed
/// in `dir`. A plugin can't replace a built-in format.
pub fn exporters(files: &[PathBuf], dir: Option<&Path>) -> Result<locus_export::Registry> {
    let mut reg = locus_export::Registry::builtin();
    for p in load(files, dir, Kind::Exporter)? {
        let e = PluginExporter::new(p).map_err(|e| anyhow!("exporter plugin: {e}"))?;
        let id = locus_export::Exporter::id(&e).to_owned();
        if !reg.add(Box::new(e)) {
            return Err(anyhow!(
                "an exporter plugin uses the format id `{id}`, which is taken"
            ));
        }
    }
    Ok(reg)
}

/// The plugins of `kind` among the `.locusplugin` files given and the
/// plugins installed in `dir`, each checked as at install.
pub fn load(
    files: &[PathBuf],
    dir: Option<&Path>,
    kind: Kind,
) -> Result<Vec<locus_plugin_host::Plugin>> {
    let mut out = Vec::new();
    if files.is_empty() && dir.is_none() {
        return Ok(out);
    }
    let host = Host::new().map_err(|e| anyhow!("{e}"))?;
    for f in files {
        let bytes = read_limited(f, MAX_ARCHIVE_BYTES, "plugin file")?;
        let archive = Archive::parse(&bytes).map_err(|e| anyhow!("plugin file: {e}"))?;
        let p = verify(&host, &archive).map_err(|e| anyhow!("plugin file: {e}"))?;
        if p.kinds().contains(&kind) {
            out.push(p);
        }
    }
    if let Some(dir) = dir {
        let installed = Registry::new(dir);
        for i in installed.list() {
            if i.enabled && i.manifest.kinds.contains(&kind) {
                let (_, p) = installed
                    .load(&host, &i.manifest.id)
                    .map_err(|e| anyhow!("installed plugin: {e}"))?;
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// Validator plugins' findings on the prepared model (#234). The built-in
/// checks have already run; a failing validator only adds a warning.
pub fn validator_findings(
    files: &[PathBuf],
    dir: Option<&Path>,
    model: &locus_export::ExportModel,
) -> Result<Vec<locus_plugin_host::validators::PluginFinding>> {
    let plugins = load(files, dir, Kind::Validator)?;
    let refs: Vec<&locus_plugin_host::Plugin> = plugins.iter().collect();
    Ok(locus_plugin_host::validators::run(&refs, model))
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
        Some("run") => run_generator(&args[1..]),
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

// ── plugin run (#187, SPEC §15.8) ───────────────────────────────────────────

/// Largest `params.ron` read.
const MAX_PARAMS_BYTES: usize = 64 * 1024;

/// A plugin named on the command line: a `.locusplugin` file, or the id of a
/// plugin installed in `--plugins <dir>`. Either way it is checked exactly
/// as an install would check it.
fn load_plugin(host: &Host, spec: &str, dir: Option<&Path>) -> Result<locus_plugin_host::Plugin> {
    let path = Path::new(spec);
    if spec.ends_with(".locusplugin") || path.is_file() {
        let bytes = read_limited(path, MAX_ARCHIVE_BYTES, "plugin file")?;
        let archive = Archive::parse(&bytes).map_err(|e| anyhow!("plugin file: {e}"))?;
        return verify(host, &archive).map_err(|e| anyhow!("plugin file: {e}"));
    }
    let dir = dir.ok_or_else(|| {
        anyhow!("no such plugin file; to run an installed plugin by id, give --plugins <dir>")
    })?;
    Registry::new(dir)
        .load(host, spec)
        .map(|(_, p)| p)
        .map_err(|e| anyhow!("{e}"))
}

/// `params.ron`: `(key: value, …)` or a map. Numbers for number parameters,
/// whole numbers for integers, `true`/`false`, a string for text, and for a
/// choice the option's name or its index. Keys left out take the schema's
/// default; unknown keys and wrong types are errors.
fn parse_params(text: &str, schema: &[ParamSpec]) -> Result<Vec<Param>> {
    let value: ron::Value = ron::from_str(text).map_err(|e| anyhow!("params file: {e}"))?;
    let ron::Value::Map(map) = value else {
        bail!("params file: expected `(key: value, …)`");
    };
    let mut out = Vec::new();
    for (k, v) in map.iter() {
        let ron::Value::String(key) = k else {
            bail!("params file: keys must be names");
        };
        let spec = schema
            .iter()
            .find(|s| &s.key == key)
            .ok_or_else(|| anyhow!("params file: `{key}` isn't a parameter of this plugin"))?;
        let whole = |n: &ron::value::Number| {
            let f = (*n).into_f64();
            (f.fract() == 0.0 && f.abs() < 9.0e15).then_some(f)
        };
        let value = match (&spec.kind, v) {
            (ParamKind::Number(_) | ParamKind::DocParameter, ron::Value::Number(n)) => {
                Some(ParamValue::Number((*n).into_f64()))
            }
            (ParamKind::Integer(_), ron::Value::Number(n)) => {
                whole(n).map(|f| ParamValue::Integer(f as i64))
            }
            (ParamKind::Boolean(_), ron::Value::Bool(b)) => Some(ParamValue::Boolean(*b)),
            (ParamKind::Choice(c), ron::Value::String(s)) => c
                .options
                .iter()
                .position(|o| o == s)
                .map(|i| ParamValue::Choice(i as u32)),
            (ParamKind::Choice(_), ron::Value::Number(n)) => whole(n)
                .filter(|f| (0.0..=f64::from(u32::MAX)).contains(f))
                .map(|f| ParamValue::Choice(f as u32)),
            (ParamKind::Text(_), ron::Value::String(s)) => Some(ParamValue::Text(s.clone())),
            _ => None,
        }
        .ok_or_else(|| anyhow!("params file: `{key}` has the wrong type"))?;
        out.push(Param {
            key: key.clone(),
            value,
        });
    }
    Ok(out)
}

#[derive(Default)]
struct RunOpts {
    plugin: Option<String>,
    params: Option<PathBuf>,
    out: Option<PathBuf>,
    format: Option<String>,
    plugins_dir: Option<PathBuf>,
    force: bool,
}

/// `locus-cli plugin run <plugin> [--params params.ron] --out out.svg`:
/// runs a generator headlessly into a new document at the origin, through
/// the same command a GUI insert uses, and exports it.
fn run_generator(args: &[String]) -> Result<(), CliError> {
    let mut o = RunOpts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| CliError::Usage(format!("{flag} needs a value")))
        };
        match a.as_str() {
            "--params" => o.params = Some(PathBuf::from(value("--params")?)),
            "--out" => o.out = Some(PathBuf::from(value("--out")?)),
            "--format" => o.format = Some(value("--format")?),
            "--plugins" => o.plugins_dir = Some(PathBuf::from(value("--plugins")?)),
            "--force" => o.force = true,
            s if s.starts_with("--") => {
                return Err(CliError::Usage(format!("unknown option `{s}`")));
            }
            s => {
                if o.plugin.replace(s.to_owned()).is_some() {
                    return Err(CliError::Usage("plugin run takes one plugin".into()));
                }
            }
        }
    }
    let spec = o
        .plugin
        .ok_or_else(|| CliError::Usage("plugin run needs a plugin".into()))?;
    let out = o
        .out
        .ok_or_else(|| CliError::Usage("--out is required".into()))?;
    let registry = locus_export::Registry::builtin();
    let format = o.format.unwrap_or_else(|| {
        out.extension()
            .and_then(|e| e.to_str())
            .map_or_else(|| "svg".to_owned(), str::to_ascii_lowercase)
    });
    if registry.get(&format).is_none() {
        return Err(CliError::Usage(format!(
            "unsupported format `{format}` (one of: {})",
            registry.ids().join(", ")
        )));
    }

    let host = Host::new().map_err(|e| anyhow!("{e}"))?;
    let plugin = load_plugin(&host, &spec, o.plugins_dir.as_deref())?;
    if !plugin.kinds().contains(&Kind::Generator) {
        return Err(anyhow!("that plugin isn't a generator").into());
    }
    let schema = plugin
        .generator_schema()
        .map_err(|e| anyhow!("the plugin failed: {e}"))?;
    let given = match &o.params {
        Some(p) => parse_params(&text(p, MAX_PARAMS_BYTES, "params file")?, &schema)?,
        None => Vec::new(),
    };
    let params = resolve(&schema, &given).map_err(|e| anyhow!("{e}"))?;
    let generated = plugin
        .generate(&params)
        .map_err(|e| anyhow!("the plugin failed: {e}"))?;

    let info = plugin.info();
    let mut ed = Editor::new(Document::new());
    ed.apply(&Command::AddGenerated {
        layer: Document::default_layer(),
        origin: locus_geom::Vec2::ZERO,
        plugin: plugin_ref(&info.id, &info.version, &params),
        output: to_cache(&generated),
    })
    .map_err(|e| anyhow!("the plugin's output can't go in a document: {e}"))?;
    let doc = ed.into_document();

    let prepared = locus_export::prepare(&doc, false);
    if prepared.blocked() && !o.force {
        let mut err = std::io::stderr().lock();
        let _ = crate::write_report(&mut err, &prepared, &[]);
        return Err(
            anyhow!("export blocked by validation errors (use --force to export anyway)").into(),
        );
    }
    let exported = registry
        .get(&format)
        .context("unknown export format")?
        .export(&prepared.model)
        .map_err(|e| anyhow!("export failed: {e}"))?;
    write_file(&out, &exported.bytes)?;
    Ok(())
}
