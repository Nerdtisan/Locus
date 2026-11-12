// SPDX-License-Identifier: AGPL-3.0-or-later

//! `render_preview` golden images within a pixel tolerance (#231). Regenerate
//! with `LOCUS_UPDATE_GOLDEN=1`, and look at the PNGs before committing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use locus_doc::Sketcher;
use locus_mcp::dto::PreviewArgs;
use locus_mcp::preview::render;
use tiny_skia::Pixmap;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn sketch(name: &str) -> Sketcher {
    let text = std::fs::read_to_string(fixture(name)).unwrap();
    Sketcher::new(locus_doc::file::load(&text).unwrap())
}

/// At most 0.5 % of pixels may differ, each by any amount: anti-aliasing
/// and float rounding differ across platforms, a wrong drawing does not.
fn assert_close(name: &str, png: &[u8]) {
    let path = fixture("preview").join(name);
    if std::env::var_os("LOCUS_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png).unwrap();
    }
    let want = Pixmap::decode_png(&std::fs::read(&path).unwrap()).unwrap();
    let got = Pixmap::decode_png(png).unwrap();
    assert_eq!((got.width(), got.height()), (want.width(), want.height()));
    let differing = got
        .pixels()
        .iter()
        .zip(want.pixels())
        .filter(|(a, b)| {
            let d = |x: u8, y: u8| x.abs_diff(y) > 48;
            d(a.red(), b.red()) || d(a.green(), b.green()) || d(a.blue(), b.blue())
        })
        .count();
    let limit = got.pixels().len() / 200;
    assert!(
        differing <= limit,
        "{name}: {differing} pixels differ (limit {limit})"
    );
}

#[test]
fn whole_plate_matches_golden() {
    let (png, info) = render(
        &sketch("plate.locus"),
        &PreviewArgs {
            width: Some(320),
            height: Some(240),
            region: None,
        },
    )
    .unwrap();
    assert_eq!(info["width"], 320);
    assert_close("plate.png", &png);
}

#[test]
fn region_zooms_in() {
    let (png, info) = render(
        &sketch("plate.locus"),
        &PreviewArgs {
            width: Some(200),
            height: Some(200),
            region: Some([-5.0, -5.0, 25.0, 25.0]),
        },
    )
    .unwrap();
    assert_eq!(info["region"][2], 25.0);
    assert_close("plate-corner.png", &png);
}

#[test]
fn empty_document_and_bad_sizes() {
    let s = Sketcher::new(locus_doc::Document::new());
    let (png, _) = render(&s, &PreviewArgs::default()).unwrap();
    let img = Pixmap::decode_png(&png).unwrap();
    assert_eq!((img.width(), img.height()), (800, 600));
    for (w, h) in [(0, 10), (10_000, 100), (100, 15)] {
        let a = PreviewArgs {
            width: Some(w),
            height: Some(h),
            region: None,
        };
        assert!(render(&s, &a).is_err(), "{w}x{h}");
    }
    let a = PreviewArgs {
        region: Some([10.0, 0.0, 0.0, 10.0]),
        ..PreviewArgs::default()
    };
    assert!(render(&s, &a).is_err());
}
