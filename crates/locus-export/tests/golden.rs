// SPDX-License-Identifier: AGPL-3.0-or-later

//! Golden-file tests for every writer: `.locus` fixture → SVG / DXF / PDF compared
//! with the committed output (SPEC §14, #132).
//!
//! Writers are deterministic (no timestamps or IDs, fixed 4-decimal numbers), so
//! the comparison is exact apart from line endings, which are normalised.
//! After an *intended* output change, regenerate with
//! `LOCUS_UPDATE_GOLDEN=1 cargo test -p locus-export --test golden`
//! and review the diff of `tests/fixtures/*.{svg,dxf,pdf}` before committing.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Bytes → text one-to-one (PDF carries binary marker bytes), CRLF normalised,
/// so a mismatch prints as a readable line diff.
fn text(b: &[u8]) -> String {
    b.iter()
        .map(|&c| if c.is_ascii() { char::from(c) } else { '?' })
        .collect::<String>()
        .replace("\r\n", "\n")
}

fn check(name: &str, ext: &str, write: fn(&locus_doc::Document) -> Vec<u8>) {
    let src = std::fs::read_to_string(fixture(&format!("{name}.locus"))).unwrap();
    let doc = locus_doc::file::load(&src).unwrap();
    let got = write(&doc);
    let path = fixture(&format!("{name}.{ext}"));
    if std::env::var_os("LOCUS_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &got).unwrap();
    }
    let want = std::fs::read(&path).unwrap_or_else(|_| {
        panic!("missing golden {name}.{ext}; create it with LOCUS_UPDATE_GOLDEN=1")
    });
    assert_eq!(
        text(&got),
        text(&want),
        "{name}.{ext} differs from the golden"
    );
    if ext == "pdf" {
        // Byte offsets in the xref table make PDF sensitive to any newline rewrite.
        assert_eq!(got, want, "{name}.pdf differs byte-for-byte");
    }
}

macro_rules! goldens {
    ($($test:ident: $name:literal,)*) => {$(
        #[test]
        fn $test() {
            check($name, "svg", locus_export::to_svg);
            check($name, "dxf", locus_export::to_dxf);
            check($name, "pdf", locus_export::to_pdf);
        }
    )*};
}

goldens! {
    plate_matches_golden: "plate",
    // True arcs (small and large), a circle, escaping, excluded construction geometry.
    arcs_match_golden: "arcs",
    // Two layers: a cut part with a hole nested inside, and an open drawn line.
    mixed_matches_golden: "mixed",
    // Cubic Béziers (#159): a closed arch + line, and an open S-curve on a Draw layer.
    beziers_match_golden: "beziers",
}
