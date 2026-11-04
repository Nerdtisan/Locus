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
  locus-cli export <file.locus> --format svg --out <out.svg>";

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
        Some("export") => {
            let opts = ExportOpts::parse(&args[1..]).map_err(CliError::Usage)?;
            let doc = read_document(&opts.input)?;
            let bytes = locus_export::to_svg(&doc);
            write_file(&opts.out, &bytes)?;
            Ok(())
        }
        Some(other) => Err(CliError::Usage(format!("unknown command `{other}`"))),
        None => Err(CliError::Usage("no command given".into())),
    }
}

struct ExportOpts {
    input: PathBuf,
    out: PathBuf,
}

impl ExportOpts {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut input = None;
        let mut out = None;
        let mut format = None;
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--format" => format = Some(it.next().ok_or("--format needs a value")?.as_str()),
                "--out" => out = Some(PathBuf::from(it.next().ok_or("--out needs a value")?)),
                s if s.starts_with("--") => return Err(format!("unknown option `{s}`")),
                s => {
                    if input.replace(PathBuf::from(s)).is_some() {
                        return Err("export takes exactly one input file".into());
                    }
                }
            }
        }
        match format {
            Some("svg") => {}
            Some(f) => return Err(format!("unsupported format `{f}` (only svg so far)")),
            None => return Err("--format is required".into()),
        }
        Ok(Self {
            input: input.ok_or("no input file given")?,
            out: out.ok_or("--out is required")?,
        })
    }
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
