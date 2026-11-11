// SPDX-License-Identifier: AGPL-3.0-or-later

//! `locus-cli plugin …` integration tests (SPEC §15.7, §15.8, #185).
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../locus-plugin-host/tests/common/examples.rs"]
mod examples;

use std::path::PathBuf;
use std::process::{Command, Output};

use examples::{build_example, repo_root};

fn cli(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_locus-cli"))
        .args(args)
        .output()
        .unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("locus-cli-plugin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Builds `examples/<name>` and packs it with the CLI; `None` without wasm32.
pub fn packed(name: &str) -> Option<PathBuf> {
    let core = build_example(name)?;
    let wasm = scratch(&format!("{name}.wasm"));
    std::fs::write(&wasm, core).unwrap();
    let out = scratch(&format!("{name}.locusplugin"));
    let manifest = repo_root().join("examples").join(name).join("plugin.toml");
    let readme = repo_root().join("examples").join(name).join("README.md");
    let res = cli(&[
        "plugin".as_ref(),
        "pack".as_ref(),
        "--manifest".as_ref(),
        manifest.as_os_str(),
        "--wasm".as_ref(),
        wasm.as_os_str(),
        "--readme".as_ref(),
        readme.as_os_str(),
        "--out".as_ref(),
        out.as_os_str(),
    ]);
    assert!(
        res.status.success(),
        "pack failed: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    Some(out)
}

#[test]
fn pack_builds_the_template() {
    let Some(file) = packed("plugin-template") else {
        return;
    };
    let bytes = std::fs::read(file).unwrap();
    let archive = locus_plugin_host::archive::Archive::parse(&bytes).unwrap();
    assert_eq!(archive.manifest.id, "com.example.my-plugin");
    assert!(archive.readme.is_some());
}

#[test]
fn pack_usage_errors() {
    let out = cli(&["plugin".as_ref(), "pack".as_ref()]);
    assert_eq!(out.status.code(), Some(2));
    let out = cli(&["plugin".as_ref(), "frobnicate".as_ref()]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn pack_refuses_garbage() {
    let wasm = scratch("garbage.wasm");
    std::fs::write(&wasm, b"not wasm").unwrap();
    let manifest = repo_root().join("examples/plugin-template/plugin.toml");
    let out_file = scratch("garbage.locusplugin");
    let out = cli(&[
        "plugin".as_ref(),
        "pack".as_ref(),
        "--manifest".as_ref(),
        manifest.as_os_str(),
        "--wasm".as_ref(),
        wasm.as_os_str(),
        "--out".as_ref(),
        out_file.as_os_str(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!out_file.exists());
}

// ── Exporter plugins (#184) ─────────────────────────────────────────────────

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn hpgl_plugin_exports_the_plate_golden() {
    let Some(hpgl) = packed("hpgl") else {
        return;
    };
    let out_file = scratch("plate.plt");
    let input = fixture("plate.locus");
    let out = cli(&[
        "export".as_ref(),
        input.as_os_str(),
        "--format".as_ref(),
        "hpgl".as_ref(),
        "--plugin".as_ref(),
        hpgl.as_os_str(),
        "--out".as_ref(),
        out_file.as_os_str(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = std::fs::read(&out_file).unwrap();
    examples::golden(&fixture(""), "plate.plt", &bytes);
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with("IN;\n") && text.ends_with("SP0;\n"));
}

#[test]
fn plugin_formats_need_their_plugin() {
    let input = fixture("plate.locus");
    let out_file = scratch("nope.plt");
    let out = cli(&[
        "export".as_ref(),
        input.as_os_str(),
        "--format".as_ref(),
        "hpgl".as_ref(),
        "--out".as_ref(),
        out_file.as_os_str(),
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("svg, pdf, dxf"));
}
