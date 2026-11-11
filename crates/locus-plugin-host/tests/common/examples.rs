// SPDX-License-Identifier: AGPL-3.0-or-later

//! Builds the plugins in `examples/` for the tests (#185): the template and
//! the first-party finger-joint box and HPGL plugins.
//!
//! They are real Rust guests, so building them needs the
//! `wasm32-unknown-unknown` target. The dev container has it; where it is
//! missing (the native macOS CI job) the tests that need them say so and
//! skip. Each example is its own Cargo project with a committed lockfile and
//! is built `--locked`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn wasm32_installed() -> bool {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let Ok(out) = Command::new(rustc).args(["--print", "sysroot"]).output() else {
        return false;
    };
    let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Path::new(&sysroot)
        .join("lib/rustlib/wasm32-unknown-unknown/lib")
        .is_dir()
}

static BUILD: Mutex<()> = Mutex::new(());

/// The core module built from `examples/<name>`, or `None` (with a message)
/// when this machine can't build wasm32.
pub fn build_example(name: &str) -> Option<Vec<u8>> {
    if !wasm32_installed() {
        eprintln!("skipping: the wasm32-unknown-unknown target isn't installed");
        return None;
    }
    let _guard = BUILD.lock().unwrap_or_else(|e| e.into_inner());
    let target_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("plugin-examples");
    let manifest = repo_root().join("examples").join(name).join("Cargo.toml");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--release", "--locked", "--quiet"])
        .args(["--target", "wasm32-unknown-unknown"])
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&target_dir)
        .status()
        .expect("cargo runs");
    assert!(status.success(), "building examples/{name} failed");

    let pkg = std::fs::read_to_string(&manifest).unwrap();
    let crate_name = pkg
        .lines()
        .find_map(|l| l.strip_prefix("name = \""))
        .and_then(|l| l.strip_suffix('"'))
        .expect("package name")
        .replace('-', "_");
    let wasm = target_dir
        .join("wasm32-unknown-unknown/release")
        .join(format!("{crate_name}.wasm"));
    Some(std::fs::read(wasm).unwrap())
}

pub fn example_manifest(name: &str) -> String {
    std::fs::read_to_string(repo_root().join("examples").join(name).join("plugin.toml")).unwrap()
}

/// Compares `actual` with `tests/golden/<file>`; `LOCUS_BLESS=1` rewrites it.
pub fn golden(dir: &Path, file: &str, actual: &[u8]) {
    let path = dir.join(file);
    if std::env::var_os("LOCUS_BLESS").is_some() {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read(&path)
        .unwrap_or_else(|_| panic!("missing golden {file}; run with LOCUS_BLESS=1"));
    // Golden files may be checked out with CRLF on Windows.
    let norm = |b: &[u8]| String::from_utf8_lossy(b).replace("\r\n", "\n");
    assert_eq!(
        norm(&expected),
        norm(actual),
        "{file} differs from its golden copy"
    );
}
