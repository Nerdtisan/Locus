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
