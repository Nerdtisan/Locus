// SPDX-License-Identifier: AGPL-3.0-or-later

//! Golden-file test: `.locus` fixture → SVG compared with the committed output (SPEC §14).
//! Regenerate with `LOCUS_UPDATE_GOLDEN=1` after an intended change.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn plate_matches_golden() {
    check("plate");
}

/// True arcs (small and large), a circle, escaping, and excluded construction geometry.
#[test]
fn arcs_match_golden() {
    check("arcs");
}

fn check(name: &str) {
    let text = std::fs::read_to_string(fixture(&format!("{name}.locus"))).unwrap();
    let doc = locus_doc::file::load(&text).unwrap();
    let svg = String::from_utf8(locus_export::to_svg(&doc)).unwrap();
    let path = fixture(&format!("{name}.svg"));
    if std::env::var_os("LOCUS_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &svg).unwrap();
    }
    let golden = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(svg, golden);
}
