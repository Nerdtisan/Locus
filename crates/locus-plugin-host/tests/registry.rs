// SPDX-License-Identifier: AGPL-3.0-or-later

//! `.locusplugin` archives, manifests and the install registry (#182, §15.4,
//! §15.7): good, malformed and wrong-version archives.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::Fixture;
use locus_plugin_host::archive::{Archive, ArchiveError, sha256_hex};
use locus_plugin_host::manifest::ManifestError;
use locus_plugin_host::registry::{InstallError, Registry};
use locus_plugin_host::{Host, LoadError};

fn manifest(id: &str, version: &str, api: &str, extra: &str) -> String {
    format!(
        "id = \"{id}\"\nname = \"Fixture\"\nversion = \"{version}\"\nauthor = \"Tests\"\n\
         licence = \"MIT\"\nkinds = [\"generator\"]\napi = \"{api}\"\n{extra}"
    )
}

fn good_manifest() -> String {
    manifest("com.example.fixture", "1.0.0", "1.0.0", "")
}

fn wasm() -> Vec<u8> {
    Fixture::well_behaved().build()
}

/// A scratch registry folder, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "locus-registry-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A raw zip with arbitrary entries, for archives `Archive::write` won't make.
fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    for (name, data) in entries {
        if name.ends_with('/') {
            z.add_directory(*name, opts).unwrap();
        } else {
            z.start_file(*name, opts).unwrap();
            z.write_all(data).unwrap();
        }
    }
    z.finish().unwrap().into_inner()
}

// ── Good archives ───────────────────────────────────────────────────────────

#[test]
fn good_archive_installs_lists_loads_and_runs() {
    let host = Host::new().unwrap();
    let dir = TempDir::new();
    let reg = Registry::new(&dir.0);
    let bytes = Archive::write(&good_manifest(), &wasm(), None, Some("# Fixture\n")).unwrap();

    let installed = reg.install(&host, &bytes).unwrap();
    assert_eq!(installed.manifest.id, "com.example.fixture");
    assert_eq!(installed.trust_label(), "Unsigned");

    let list = reg.list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].manifest.name, "Fixture");

    let (_, plugin) = reg.load(&host, "com.example.fixture").unwrap();
    assert_eq!(plugin.generate(&[]).unwrap().points.len(), 4);

    reg.uninstall("com.example.fixture").unwrap();
    assert!(reg.list().is_empty());
    assert!(matches!(
        reg.uninstall("com.example.fixture"),
        Err(InstallError::NotInstalled)
    ));
}

#[test]
fn write_is_deterministic() {
    let a = Archive::write(&good_manifest(), &wasm(), None, None).unwrap();
    let b = Archive::write(&good_manifest(), &wasm(), None, None).unwrap();
    assert_eq!(a, b);
}

#[test]
fn checksum_is_verified() {
    let w = wasm();
    let ok = manifest(
        "com.example.fixture",
        "1.0.0",
        "1.0.0",
        &format!("sha256 = \"{}\"\n", sha256_hex(&w)),
    );
    assert!(Archive::parse(&zip(&[("plugin.toml", ok.as_bytes()), ("plugin.wasm", &w)])).is_ok());

    let bad = manifest(
        "com.example.fixture",
        "1.0.0",
        "1.0.0",
        &format!("sha256 = \"{}\"\n", "0".repeat(64)),
    );
    assert!(matches!(
        Archive::parse(&zip(&[
            ("plugin.toml", bad.as_bytes()),
            ("plugin.wasm", &w)
        ])),
        Err(ArchiveError::Checksum)
    ));
}

#[test]
fn tampered_install_is_caught_on_load() {
    let host = Host::new().unwrap();
    let dir = TempDir::new();
    let reg = Registry::new(&dir.0);
    let w = wasm();
    let m = manifest(
        "com.example.fixture",
        "1.0.0",
        "1.0.0",
        &format!("sha256 = \"{}\"\n", sha256_hex(&w)),
    );
    let installed = reg
        .install(&host, &Archive::write(&m, &w, None, None).unwrap())
        .unwrap();
    // Swap the component on disk for a different (still valid) one.
    let other = Fixture::well_behaved().with_generate("unreachable").build();
    std::fs::write(installed.dir.join("plugin.wasm"), other).unwrap();
    assert!(matches!(
        reg.load(&host, "com.example.fixture"),
        Err(InstallError::Archive(ArchiveError::Checksum))
    ));
}

#[test]
fn reinstall_replaces_and_failed_install_keeps_the_old_one() {
    let host = Host::new().unwrap();
    let dir = TempDir::new();
    let reg = Registry::new(&dir.0);
    reg.install(
        &host,
        &Archive::write(&good_manifest(), &wasm(), None, None).unwrap(),
    )
    .unwrap();

    // A new version whose component disagrees with its manifest fails...
    let v2 = manifest("com.example.fixture", "2.0.0", "1.0.0", "");
    let err = reg
        .install(&host, &Archive::write(&v2, &wasm(), None, None).unwrap())
        .unwrap_err();
    assert!(matches!(err, InstallError::Mismatch(_)), "{err:?}");
    // ...and leaves 1.0.0 working.
    let (installed, plugin) = reg.load(&host, "com.example.fixture").unwrap();
    assert_eq!(installed.manifest.version, "1.0.0");
    assert!(plugin.generate(&[]).is_ok());

    // A matching 2.0.0 replaces it.
    let mut f = Fixture::well_behaved();
    f.version = b"2.0.0".to_vec();
    reg.install(&host, &Archive::write(&v2, &f.build(), None, None).unwrap())
        .unwrap();
    assert_eq!(reg.list()[0].manifest.version, "2.0.0");
    assert_eq!(reg.list().len(), 1);
}

#[test]
fn junk_folders_are_ignored_by_list() {
    let host = Host::new().unwrap();
    let dir = TempDir::new();
    let reg = Registry::new(&dir.0);
    reg.install(
        &host,
        &Archive::write(&good_manifest(), &wasm(), None, None).unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(dir.0.join("com.example.broken")).unwrap();
    std::fs::write(dir.0.join("com.example.broken/plugin.toml"), "garbage").unwrap();
    std::fs::create_dir_all(dir.0.join("not a plugin")).unwrap();
    let ids: Vec<String> = reg.list().into_iter().map(|i| i.manifest.id).collect();
    assert_eq!(ids, vec!["com.example.fixture"]);
}

#[test]
fn ids_outside_the_folder_are_refused() {
    let reg = Registry::new(std::env::temp_dir().join("locus-registry-unused"));
    for id in ["..", "../etc", "a/b", "com.example/../../x"] {
        assert!(
            matches!(reg.get(id), Err(InstallError::NotInstalled)),
            "{id}"
        );
        assert!(
            matches!(reg.uninstall(id), Err(InstallError::NotInstalled)),
            "{id}"
        );
    }
}

// ── Malformed archives ──────────────────────────────────────────────────────

#[test]
fn malformed_archives_are_rejected_with_clear_errors() {
    let m = good_manifest();
    let w = wasm();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("not a zip", b"PK but not really".to_vec()),
        ("empty zip", zip(&[])),
        ("no wasm", zip(&[("plugin.toml", m.as_bytes())])),
        ("no manifest", zip(&[("plugin.wasm", &w)])),
        (
            "extra file",
            zip(&[
                ("plugin.toml", m.as_bytes()),
                ("plugin.wasm", &w),
                ("run.sh", b"#!"),
            ]),
        ),
        (
            "path entry",
            zip(&[("plugin.toml", m.as_bytes()), ("../plugin.wasm", &w)]),
        ),
        (
            "directory",
            zip(&[
                ("plugin.toml", m.as_bytes()),
                ("plugin.wasm", &w),
                ("assets/", b""),
            ]),
        ),
        (
            "bad manifest",
            zip(&[("plugin.toml", b"id = 3"), ("plugin.wasm", &w)]),
        ),
        (
            "non-utf8 manifest",
            zip(&[("plugin.toml", b"\xff\xfe"), ("plugin.wasm", &w)]),
        ),
        (
            "binary readme",
            zip(&[
                ("plugin.toml", m.as_bytes()),
                ("plugin.wasm", &w),
                ("README.md", b"\xff"),
            ]),
        ),
    ];
    for (what, bytes) in cases {
        let err = Archive::parse(&bytes).expect_err(what);
        assert!(!err.to_string().is_empty(), "{what}");
    }
}

#[test]
fn oversized_entry_is_rejected() {
    let big = vec![0u8; 2 * 1024 * 1024];
    let bytes = zip(&[
        ("plugin.toml", good_manifest().as_bytes()),
        ("plugin.wasm", &wasm()),
        ("icon.png", &big),
    ]);
    assert!(matches!(
        Archive::parse(&bytes),
        Err(ArchiveError::EntryTooLarge("icon.png"))
    ));
}

#[test]
fn garbage_component_fails_install() {
    let host = Host::new().unwrap();
    let dir = TempDir::new();
    let bytes = zip(&[
        ("plugin.toml", good_manifest().as_bytes()),
        ("plugin.wasm", b"\0asm garbage"),
    ]);
    assert!(matches!(
        Registry::new(&dir.0).install(&host, &bytes),
        Err(InstallError::Load(LoadError::Invalid(_)))
    ));
    assert!(!dir.0.join("com.example.fixture").exists());
}

#[test]
fn capability_requests_are_refused() {
    let m = manifest(
        "com.example.fixture",
        "1.0.0",
        "1.0.0",
        "capabilities = [\"network\"]\n",
    );
    assert!(matches!(
        Archive::parse(&zip(&[
            ("plugin.toml", m.as_bytes()),
            ("plugin.wasm", &wasm())
        ])),
        Err(ArchiveError::Manifest(ManifestError::Capability(_)))
    ));
}

#[test]
fn manifest_must_match_the_component() {
    let host = Host::new().unwrap();
    let dir = TempDir::new();
    let reg = Registry::new(&dir.0);
    let cases = [
        manifest("com.example.other", "1.0.0", "1.0.0", ""),
        manifest("com.example.fixture", "9.9.9", "1.0.0", ""),
        good_manifest().replace("[\"generator\"]", "[\"exporter\"]"),
        good_manifest().replace("[\"generator\"]", "[\"generator\", \"modifier\"]"),
        // Importers and validators run since M4 (#233, #234), so declaring
        // one the component doesn't provide is a mismatch like any other.
        good_manifest().replace("[\"generator\"]", "[\"generator\", \"importer\"]"),
        good_manifest().replace("[\"generator\"]", "[\"generator\", \"validator\"]"),
    ];
    for m in cases {
        let err = reg
            .install(
                &host,
                &zip(&[("plugin.toml", m.as_bytes()), ("plugin.wasm", &wasm())]),
            )
            .unwrap_err();
        assert!(matches!(err, InstallError::Mismatch(_)), "{m}: {err:?}");
    }
}

// ── Wrong version ───────────────────────────────────────────────────────────

#[test]
fn wrong_api_version_in_manifest_is_rejected() {
    let m = manifest("com.example.fixture", "1.0.0", "2.0.0", "");
    assert!(matches!(
        Archive::parse(&zip(&[
            ("plugin.toml", m.as_bytes()),
            ("plugin.wasm", &wasm())
        ])),
        Err(ArchiveError::Manifest(ManifestError::UnsupportedApi { .. }))
    ));
}

#[test]
fn wrong_api_version_in_component_is_rejected() {
    let host = Host::new().unwrap();
    let dir = TempDir::new();
    let mut f = Fixture::well_behaved();
    f.api_version = "2.0.0".into();
    let bytes = zip(&[
        ("plugin.toml", good_manifest().as_bytes()),
        ("plugin.wasm", &f.build()),
    ]);
    assert!(matches!(
        Registry::new(&dir.0).install(&host, &bytes),
        Err(InstallError::Load(LoadError::UnsupportedApi { .. }))
    ));
}
