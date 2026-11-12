// SPDX-License-Identifier: AGPL-3.0-or-later

//! One golden export per machine-profile preset (SPEC §7, #236): the `layers`
//! fixture exported through each preset's format and colour mapping.
//!
//! Regenerate after an *intended* change with
//! `LOCUS_UPDATE_GOLDEN=1 cargo test -p locus-export --test profile_golden`
//! and review the diff of `tests/fixtures/profiles/` before committing.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use locus_export::{Registry, prepare_for, profile};
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn normalise(b: &[u8]) -> String {
    String::from_utf8_lossy(b).replace("\r\n", "\n")
}

#[test]
fn every_preset_matches_its_golden() {
    let src = std::fs::read_to_string(fixtures().join("layers.locus")).unwrap();
    let doc = locus_doc::file::load(&src).unwrap();
    let registry = Registry::builtin();
    let mut checked = 0;
    for p in profile::presets() {
        // The HPGL preset needs the exporter plugin (§15); locus-cli's plugin
        // tests cover it end to end.
        let Some(exporter) = registry.get(&p.format) else {
            assert_eq!(p.format, "hpgl", "only HPGL comes from a plugin");
            continue;
        };
        let prepared = prepare_for(&doc, false, Some(&p));
        let got = exporter.export(&prepared.model).unwrap();
        let path = fixtures()
            .join("profiles")
            .join(format!("{}.{}", p.id, got.extension));
        if std::env::var_os("LOCUS_UPDATE_GOLDEN").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &got.bytes).unwrap();
        }
        let want = std::fs::read(&path)
            .unwrap_or_else(|_| panic!("missing golden for {}; use LOCUS_UPDATE_GOLDEN=1", p.id));
        assert_eq!(
            normalise(&got.bytes),
            normalise(&want),
            "{} differs from its golden",
            p.id
        );
        checked += 1;
    }
    assert_eq!(checked, 8, "all non-plugin presets are checked");
}

#[test]
fn a_profile_changes_colours_and_generic_ones_do_not() {
    let src = std::fs::read_to_string(fixtures().join("layers.locus")).unwrap();
    let doc = locus_doc::file::load(&src).unwrap();
    let plain = prepare_for(&doc, false, None).model;
    let generic = prepare_for(&doc, false, profile::find("generic-svg", &[]).as_ref()).model;
    assert_eq!(plain, generic, "generic profiles keep the layer colours");
    let lb = prepare_for(&doc, false, profile::find("lightburn", &[]).as_ref()).model;
    assert_ne!(plain, lb);
    assert!(lb.layers.iter().any(|l| l.colour == [255, 0, 0]));
}
