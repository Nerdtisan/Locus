// SPDX-License-Identifier: AGPL-3.0-or-later

//! Headless validate/export tool used by tests and CI (SPEC §3, §15.8, §16.2).
//!
//! Exit codes: 0 success, 1 the input or output could not be processed,
//! 2 bad command line. Error messages never echo file names or contents.

use anyhow::{Context, Result, bail};
use locus_doc::{Document, EntityKind, Sketcher, Status};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage:
  locus-cli info <file.locus>
  locus-cli validate <file.locus> [--fix]
  locus-cli export <file.locus> --format svg|pdf|dxf --out <file> [--fix] [--force]

validate prints errors and warnings and exits 1 when errors remain.
--fix applies the safe auto-fixes to the exported geometry (never to the file).
export refuses to write when errors remain unless --force is given.";

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
        Some("export") => {
            let opts = ExportOpts::parse(&args[1..]).map_err(CliError::Usage)?;
            let doc = read_document(&opts.input)?;
            let prepared = locus_export::prepare(&doc, opts.fix);
            if prepared.blocked() && !opts.force {
                let mut err = std::io::stderr().lock();
                let _ = write_report(&mut err, &prepared);
                return Err(CliError::Failed(anyhow::anyhow!(
                    "export blocked by validation errors (use --fix, or --force to export anyway)"
                )));
            }
            let registry = locus_export::Registry::builtin();
            let exporter = registry
                .get(&opts.format)
                .context("unknown export format")?;
            let exported = exporter
                .export(&prepared.model)
                .map_err(|e| anyhow::anyhow!("export failed: {e}"))?;
            write_file(&opts.out, &exported.bytes)?;
            Ok(())
        }
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
    format: String,
    fix: bool,
    force: bool,
}

impl ExportOpts {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut input = None;
        let mut out = None;
        let mut format = None;
        let (mut fix, mut force) = (false, false);
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--format" => format = Some(it.next().ok_or("--format needs a value")?.as_str()),
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
        let format = format.ok_or("--format is required")?.to_owned();
        let registry = locus_export::Registry::builtin();
        if registry.get(&format).is_none() {
            return Err(format!(
                "unsupported format `{format}` (one of: {})",
                registry.ids().join(", ")
            ));
        }
        Ok(Self {
            input: input.ok_or("no input file given")?,
            out: out.ok_or("--out is required")?,
            format,
            fix,
            force,
        })
    }
}

/// Fixes applied, then each issue, then a one-line summary.
fn write_report(out: &mut impl std::io::Write, p: &locus_export::Prepared) -> std::io::Result<()> {
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
