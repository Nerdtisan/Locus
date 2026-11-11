// SPDX-License-Identifier: AGPL-3.0-or-later

//! Headless validate/export tool used by tests and CI (SPEC §3, §15.8, §16.2).
//!
//! Exit codes: 0 success, 1 the input or output could not be processed,
//! 2 bad command line. Error messages never echo file names or contents.

use anyhow::{Context, Result, bail};
use locus_doc::{Document, EntityKind, Sketcher, Status};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod plugin;

const USAGE: &str = "\
usage:
  locus-cli info <file.locus>
  locus-cli validate <file.locus> [--fix]
  locus-cli profiles
  locus-cli export <file.locus> (--format <id> | --profile <id> | --profile-file <profile.json>)
                   --out <file> [--fix] [--force]
                   [--plugin <file.locusplugin>]... [--plugins <dir>]
  locus-cli plugin pack --manifest <plugin.toml> --wasm <file.wasm> --out <file.locusplugin>
                        [--readme <README.md>] [--icon <icon.png>]
  locus-cli plugin run <file.locusplugin | id> [--params <params.ron>] --out <file>
                       [--format svg|pdf|dxf] [--plugins <dir>] [--force]

profiles lists the built-in machine profiles (SPEC §7). export --profile applies one:
its format, colours and bed check; --profile-file loads one from a .json file.
validate prints errors and warnings and exits 1 when errors remain.
--fix applies the safe auto-fixes to the exported geometry (never to the file).
export refuses to write when errors remain unless --force is given. Formats:
svg, pdf, dxf, plus those of exporter plugins given with --plugin (a file) or
--plugins (an installed-plugins folder).
plugin pack wraps a wasm32 build of a plugin into a checked .locusplugin.
plugin run runs a generator headlessly and exports its output; params.ron is
`(key: value, ...)`, and keys left out take the plugin's defaults.";

/// Largest input read, matching the file loader's own cap (SPEC §10).
const MAX_INPUT_BYTES: u64 = locus_doc::file::MAX_FILE_BYTES as u64;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage(msg)) => {
            eprintln!("error: {msg}\n{USAGE}");
            ExitCode::from(2)
        }
        Err(CliError::Failed(e)) => {
            eprintln!("error: {e:#}");
            ExitCode::from(1)
        }
    }
}

enum CliError {
    Usage(String),
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for CliError {
    fn from(e: anyhow::Error) -> Self {
        CliError::Failed(e)
    }
}

fn run(args: &[String]) -> Result<(), CliError> {
    match args.first().map(String::as_str) {
        Some("info") => {
            let [_, file] = args else {
                return Err(CliError::Usage("info takes exactly one file".into()));
            };
            let doc = read_document(Path::new(file))?;
            let mut out = std::io::stdout().lock();
            write_info(&mut out, &doc).context("cannot write to stdout")?;
            Ok(())
        }
        Some("validate") => {
            let opts = ValidateOpts::parse(&args[1..]).map_err(CliError::Usage)?;
            let doc = read_document(&opts.input)?;
            let prepared = locus_export::prepare(&doc, opts.fix);
            let mut out = std::io::stdout().lock();
            write_report(&mut out, &prepared).context("cannot write to stdout")?;
            if prepared.blocked() {
                return Err(CliError::Failed(anyhow::anyhow!("validation found errors")));
            }
            Ok(())
        }
        Some("profiles") => {
            if args.len() != 1 {
                return Err(CliError::Usage("profiles takes no arguments".into()));
            }
            let mut out = std::io::stdout().lock();
            write_profiles(&mut out).context("cannot write to stdout")?;
            Ok(())
        }
        Some("export") => {
            let mut opts = ExportOpts::parse(&args[1..]).map_err(CliError::Usage)?;
            let profile = opts.load_profile()?;
            if let Some(p) = &profile {
                match &opts.format {
                    Some(f) if f != &p.format => {
                        return Err(CliError::Usage(format!(
                            "--format `{f}` conflicts with the profile's format `{}`",
                            p.format
                        )));
                    }
                    _ => opts.format = Some(p.format.clone()),
                }
            }
            let Some(format) = opts.format.clone() else {
                return Err(CliError::Usage(
                    "--format, --profile or --profile-file is required".into(),
                ));
            };
            let registry = plugin::exporters(&opts.plugin_files, opts.plugins_dir.as_deref())?;
            if registry.get(&format).is_none() {
                return Err(CliError::Usage(format!(
                    "unsupported format `{}` (one of: {})",
                    format,
                    registry.ids().join(", ")
                )));
            }
            let doc = read_document(&opts.input)?;
            let prepared = locus_export::prepare_for(&doc, opts.fix, profile.as_ref());
            if prepared.blocked() && !opts.force {
                let mut err = std::io::stderr().lock();
                let _ = write_report(&mut err, &prepared);
                return Err(CliError::Failed(anyhow::anyhow!(
                    "export blocked by validation errors (use --fix, or --force to export anyway)"
                )));
            }
            let exporter = registry.get(&format).context("unknown export format")?;
            let exported = exporter
                .export(&prepared.model)
                .map_err(|e| anyhow::anyhow!("export failed: {e}"))?;
            write_file(&opts.out, &exported.bytes)?;
            Ok(())
        }
        Some("plugin") => plugin::run(&args[1..]),
        Some(other) => Err(CliError::Usage(format!("unknown command `{other}`"))),
        None => Err(CliError::Usage("no command given".into())),
    }
}

struct ValidateOpts {
    input: PathBuf,
    fix: bool,
}

impl ValidateOpts {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut input = None;
        let mut fix = false;
        for a in args {
            match a.as_str() {
                "--fix" => fix = true,
                s if s.starts_with("--") => return Err(format!("unknown option `{s}`")),
                s => {
                    if input.replace(PathBuf::from(s)).is_some() {
                        return Err("validate takes exactly one input file".into());
                    }
                }
            }
        }
        Ok(Self {
            input: input.ok_or("no input file given")?,
            fix,
        })
    }
}

struct ExportOpts {
    input: PathBuf,
    out: PathBuf,
    format: Option<String>,
    profile: Option<String>,
    profile_file: Option<PathBuf>,
    fix: bool,
    force: bool,
    plugin_files: Vec<PathBuf>,
    plugins_dir: Option<PathBuf>,
}

impl ExportOpts {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut input = None;
        let mut out = None;
        let mut format = None;
        let (mut profile, mut profile_file) = (None, None);
        let (mut fix, mut force) = (false, false);
        let (mut plugin_files, mut plugins_dir) = (Vec::new(), None);
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--plugin" => {
                    plugin_files.push(PathBuf::from(it.next().ok_or("--plugin needs a value")?))
                }
                "--plugins" => {
                    plugins_dir = Some(PathBuf::from(it.next().ok_or("--plugins needs a value")?));
                }
                "--format" => format = Some(it.next().ok_or("--format needs a value")?.clone()),
                "--profile" => profile = Some(it.next().ok_or("--profile needs a value")?.clone()),
                "--profile-file" => {
                    profile_file = Some(PathBuf::from(
                        it.next().ok_or("--profile-file needs a value")?,
                    ));
                }
                "--out" => out = Some(PathBuf::from(it.next().ok_or("--out needs a value")?)),
                "--fix" => fix = true,
                "--force" => force = true,
                s if s.starts_with("--") => return Err(format!("unknown option `{s}`")),
                s => {
                    if input.replace(PathBuf::from(s)).is_some() {
                        return Err("export takes exactly one input file".into());
                    }
                }
            }
        }
        if profile.is_some() && profile_file.is_some() {
            return Err("use either --profile or --profile-file, not both".into());
        }
        Ok(Self {
            input: input.ok_or("no input file given")?,
            out: out.ok_or("--out is required")?,
            format,
            profile,
            profile_file,
            fix,
            force,
            plugin_files,
            plugins_dir,
        })
    }
}

impl ExportOpts {
    /// The machine profile asked for, if any. A custom profile file is
    /// untrusted: it is size-bounded and validated by the model.
    fn load_profile(&self) -> Result<Option<locus_export::MachineProfile>, CliError> {
        if let Some(id) = &self.profile {
            return locus_export::profile::find(id, &[])
                .map(Some)
                .ok_or_else(|| {
                    CliError::Usage(format!("unknown profile `{id}` (see `locus-cli profiles`)"))
                });
        }
        let Some(path) = &self.profile_file else {
            return Ok(None);
        };
        let meta = std::fs::metadata(path).context("cannot read profile file")?;
        if meta.len() > locus_export::profile::MAX_PROFILE_JSON_BYTES as u64 {
            return Err(CliError::Failed(anyhow::anyhow!(
                "profile file is too large"
            )));
        }
        let text = std::fs::read_to_string(path).context("cannot read profile file")?;
        locus_export::MachineProfile::from_json(&text)
            .map(Some)
            .map_err(|e| CliError::Failed(anyhow::anyhow!("{e}")))
    }
}

/// One line per built-in profile: id, export format, bed, name.
fn write_profiles(out: &mut impl std::io::Write) -> std::io::Result<()> {
    for p in locus_export::profile::presets() {
        let bed = p.bed.map_or_else(
            || "-".to_owned(),
            |b| format!("{}x{}mm", b.width_mm, b.height_mm),
        );
        writeln!(out, "{}\t{}\t{}\t{}", p.id, p.format, bed, p.name)?;
    }
    Ok(())
}

/// Fixes applied, then each issue, then a one-line summary.
pub(crate) fn write_report(
    out: &mut impl std::io::Write,
    p: &locus_export::Prepared,
) -> std::io::Result<()> {
    for f in &p.fixes {
        writeln!(out, "fixed: {}", f.message)?;
    }
    for i in &p.report.issues {
        writeln!(out, "{i}")?;
    }
    let c = p.report.counts();
    writeln!(out, "{} error(s), {} warning(s)", c.errors, c.warnings)
}

fn read_document(path: &Path) -> Result<Document> {
    let meta = std::fs::metadata(path).context("cannot read input file")?;
    if meta.len() > MAX_INPUT_BYTES {
        bail!("input file is too large");
    }
    let text = std::fs::read_to_string(path).context("cannot read input file (not UTF-8 text?)")?;
    locus_doc::file::load(&text).map_err(|e| anyhow::anyhow!("{e}"))
}

/// Write via temp file + rename so a failure never leaves a truncated output
/// (and never damages an existing file).
fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let result = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.context("cannot write output file")
}

fn write_info(out: &mut impl std::io::Write, doc: &Document) -> std::io::Result<()> {
    let sketcher = Sketcher::new(doc.clone());
    let state = sketcher.state();
    let count = |k: EntityKind| {
        doc.entities()
            .iter()
            .filter(|(_, e)| e.geom.kind() == k)
            .count()
    };
    let status = match state.status {
        Status::UnderConstrained => "under-constrained",
        Status::FullyConstrained => "fully-constrained",
        Status::OverConstrained => "over-constrained",
    };
    writeln!(out, "layers: {}", doc.layers().len())?;
    writeln!(out, "entities: {}", doc.entities().len())?;
    writeln!(out, "  points: {}", count(EntityKind::Point))?;
    writeln!(out, "  lines: {}", count(EntityKind::Line))?;
    writeln!(out, "  circles: {}", count(EntityKind::Circle))?;
    writeln!(out, "  arcs: {}", count(EntityKind::Arc))?;
    writeln!(out, "constraints: {}", doc.constraints().len())?;
    writeln!(out, "dof: {}", state.dof)?;
    writeln!(
        out,
        "solve: {}",
        if state.converged {
            "converged"
        } else {
            "not converged"
        }
    )?;
    writeln!(out, "status: {status}")?;
    writeln!(out, "conflicts: {}", state.conflicts.len())?;
    writeln!(out, "redundant: {}", state.redundant.len())
}
