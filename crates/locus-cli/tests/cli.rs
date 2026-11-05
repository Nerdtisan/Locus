// SPDX-License-Identifier: AGPL-3.0-or-later

//! Integration tests: drive the `locus-cli` binary against fixtures (SPEC §14).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn cli(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_locus-cli"))
        .args(args)
        .output()
        .unwrap()
}

fn s(x: &str) -> &std::ffi::OsStr {
    x.as_ref()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("locus-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn info_reports_counts_dof_and_status() {
    let f = fixture("plate.locus");
    let out = cli(&[s("info"), f.as_os_str()]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    for line in [
        "entities: 11",
        "  lines: 4",
        "  circles: 1",
        "  arcs: 0",
        "constraints: 8",
        "solve: converged",
    ] {
        assert!(
            text.lines().any(|l| l == line),
            "missing `{line}` in:\n{text}"
        );
    }
    assert!(text.contains("dof: "));
    assert!(text.contains("status: "));
}

#[test]
fn export_svg_matches_golden() {
    let out_path = scratch("plate.svg");
    let f = fixture("plate.locus");
    let out = cli(&[
        s("export"),
        f.as_os_str(),
        s("--format"),
        s("svg"),
        s("--out"),
        out_path.as_os_str(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got = std::fs::read_to_string(&out_path).unwrap();
    let golden = std::fs::read_to_string(fixture("plate.svg"))
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(got, golden);
}

#[test]
fn missing_file_fails_without_echoing_the_path() {
    let missing = scratch("does-not-exist.locus");
    let out = cli(&[s("info"), missing.as_os_str()]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.starts_with("error: "));
    assert!(!err.contains("does-not-exist"));
}

#[test]
fn malformed_file_fails_cleanly_and_writes_nothing() {
    let bad = scratch("bad.locus");
    std::fs::write(&bad, "this is not a locus file").unwrap();
    let out_path = scratch("bad.svg");
    let out = cli(&[
        s("export"),
        bad.as_os_str(),
        s("--format"),
        s("svg"),
        s("--out"),
        out_path.as_os_str(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("not a valid .locus file")
    );
    assert!(!out_path.exists());
}

#[test]
fn usage_errors_exit_2() {
    assert_eq!(cli(&[]).status.code(), Some(2));
    assert_eq!(cli(&[s("frobnicate")]).status.code(), Some(2));
    let f = fixture("plate.locus");
    let out = cli(&[
        s("export"),
        f.as_os_str(),
        s("--format"),
        s("hpgl"),
        s("--out"),
        s("x"),
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(cli(&[s("validate")]).status.code(), Some(2));
    let out = cli(&[s("export"), f.as_os_str(), s("--out"), s("x")]);
    assert_eq!(out.status.code(), Some(2));
}

fn run_export(input: &str, format: &str, out: &std::path::Path, extra: &[&str]) -> Output {
    let f = fixture(input);
    let mut args: Vec<&std::ffi::OsStr> = vec![
        s("export"),
        f.as_os_str(),
        s("--format"),
        s(format),
        s("--out"),
        out.as_os_str(),
    ];
    args.extend(extra.iter().map(|a| s(a)));
    cli(&args)
}

fn validate(input: &str, extra: &[&str]) -> (Option<i32>, String) {
    let f = fixture(input);
    let mut args: Vec<&std::ffi::OsStr> = vec![s("validate"), f.as_os_str()];
    args.extend(extra.iter().map(|a| s(a)));
    let out = cli(&args);
    (out.status.code(), String::from_utf8(out.stdout).unwrap())
}

#[test]
fn validate_clean_file_exits_0() {
    let (code, text) = validate("plate.locus", &[]);
    assert_eq!(code, Some(0));
    assert!(text.contains("0 error(s), 0 warning(s)"), "{text}");
}

#[test]
fn validate_reports_open_path_with_entity_ids_and_exits_1() {
    let (code, text) = validate("open.locus", &[]);
    assert_eq!(code, Some(1));
    assert!(text.contains("error: layer \"Cut\": open path"), "{text}");
    assert!(
        text.contains("[entities 7, 8, 9]") || text.contains("[entities 9, 8, 7]"),
        "{text}"
    );
}

#[test]
fn validate_fix_resolves_duplicates_but_not_open_paths() {
    let (code, text) = validate("dup.locus", &[]);
    assert_eq!(code, Some(1));
    assert!(text.contains("double cut"), "{text}");
    let (code, text) = validate("dup.locus", &["--fix"]);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("fixed: "), "{text}");
    let (code, _) = validate("open.locus", &["--fix"]);
    assert_eq!(code, Some(1));
}

#[test]
fn export_is_blocked_by_errors_unless_forced_or_fixed() {
    let blocked = scratch("blocked.svg");
    let out = run_export("open.locus", "svg", &blocked, &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("blocked by validation"));
    assert!(!blocked.exists());

    let forced = scratch("forced.svg");
    assert!(
        run_export("open.locus", "svg", &forced, &["--force"])
            .status
            .success()
    );
    assert!(forced.exists());

    let fixed = scratch("fixed.dxf");
    assert!(
        run_export("dup.locus", "dxf", &fixed, &["--fix"])
            .status
            .success()
    );
    // The duplicate edge is gone: exactly four LINEs remain.
    let dxf = std::fs::read_to_string(&fixed).unwrap();
    assert_eq!(
        dxf.matches(
            "
LINE
"
        )
        .count(),
        4
    );
}

#[test]
fn export_pdf_and_dxf_from_the_cli() {
    let pdf = scratch("plate.pdf");
    assert!(run_export("plate.locus", "pdf", &pdf, &[]).status.success());
    let bytes = std::fs::read(&pdf).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.5"));
    assert!(bytes.ends_with(
        b"%%EOF
"
    ));

    let dxf = scratch("plate.dxf");
    assert!(run_export("plate.locus", "dxf", &dxf, &[]).status.success());
    let text = std::fs::read_to_string(&dxf).unwrap();
    assert!(text.contains("AC1009") && text.contains("CIRCLE"));
}
