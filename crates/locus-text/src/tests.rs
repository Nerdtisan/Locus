// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

fn style(size: f64) -> TextStyle {
    TextStyle {
        tracking: 0.0,
        size,
        ..TextStyle::default()
    }
}

fn load(f: BundledFont) -> Font {
    f.load().unwrap()
}

#[test]
fn every_bundled_font_loads() {
    for f in BundledFont::ALL {
        let font = f.load().unwrap();
        assert_eq!(font.is_single_line(), f.is_single_line(), "{f:?}");
        assert_eq!(BundledFont::from_id(f.id()), Some(f));
        let g = font.layout("Hello, World 123", &style(10.0)).unwrap();
        assert!(!g.contours.is_empty(), "{f:?}");
        assert!(g.width > 20.0 && g.width < 150.0, "{f:?} {}", g.width);
    }
    assert_eq!(BundledFont::from_id("nope"), None);
}

#[test]
fn outline_glyphs_are_closed_cubic_contours_in_mm() {
    let g = load(BundledFont::Figtree)
        .layout("O", &style(10.0))
        .unwrap();
    // 'O' has an outer and inner contour.
    assert_eq!(g.contours.len(), 2);
    for c in &g.contours {
        assert!(c.closed);
        let last = c.segs.last().unwrap().end();
        assert!((last - c.start).length() < 1e-9, "explicitly closed");
    }
    let b = g.bbox().unwrap();
    // A capital O at 10 mm em is ~7 mm tall, sits on the baseline (± overshoot).
    assert!(b.height() > 6.0 && b.height() < 8.5, "{}", b.height());
    assert!(b.min.y > -0.5 && b.min.y < 0.0 + 0.2);
}

#[test]
fn layout_is_deterministic() {
    let f = load(BundledFont::LibreBaskerville);
    let a = f.layout("Quick brown fox", &style(12.0)).unwrap();
    let b = f.layout("Quick brown fox", &style(12.0)).unwrap();
    assert_eq!(a, b);
}

#[test]
fn kerning_is_applied() {
    for font in [BundledFont::Figtree, BundledFont::LibreBaskerville] {
        let f = load(font);
        let s = style(100.0);
        let av = f.layout("AV", &s).unwrap().width;
        let a = f.layout("A", &s).unwrap().width;
        let v = f.layout("V", &s).unwrap().width;
        assert!(av < a + v - 1.0, "{font:?}: AV {av} vs A+V {}", a + v);
    }
}

#[test]
fn size_scales_linearly() {
    let f = load(BundledFont::Figtree);
    let a = f.layout("Locus", &style(10.0)).unwrap();
    let b = f.layout("Locus", &style(20.0)).unwrap();
    assert!((b.width - 2.0 * a.width).abs() < 1e-6);
    let (ba, bb) = (a.bbox().unwrap(), b.bbox().unwrap());
    assert!((bb.height() - 2.0 * ba.height()).abs() < 1e-6);
}

#[test]
fn alignment_and_lines() {
    let f = load(BundledFont::HersheySans);
    let mut s = style(10.0);
    let left = f.layout("ab\nabcd", &s).unwrap();
    assert_eq!(left.lines, 2);
    assert!(left.bbox().unwrap().min.x >= -1e-9);
    s.align = Align::Right;
    let right = f.layout("ab\nabcd", &s).unwrap();
    assert!(right.bbox().unwrap().max.x <= 1e-9);
    assert!((right.width - left.width).abs() < 1e-9);
    s.align = Align::Center;
    let c = f.layout("abcd", &s).unwrap();
    let b = c.bbox().unwrap();
    assert!((b.min.x + b.max.x).abs() < c.width * 0.2, "roughly centred");
    // The second line sits below the first.
    assert!(left.bbox().unwrap().min.y < -5.0);
}

#[test]
fn stroke_text_is_open_strokes_with_font_metrics() {
    let f = load(BundledFont::HersheySans);
    let g = f.layout("HI", &style(32.0)).unwrap();
    assert!(g.contours.iter().all(|c| !c.closed));
    // size == units_per_em (32), so 1 font unit == 1 mm.
    let font = StrokeFont::from_jhf("t", include_str!("../fonts/hershey/futural.jhf")).unwrap();
    let want = font.glyph('H').unwrap().advance + font.glyph('I').unwrap().advance;
    assert!((g.width - want).abs() < 1e-9);
    let b = g.bbox().unwrap();
    assert!(
        (b.height() - 21.0).abs() < 1e-9,
        "cap height {}",
        b.height()
    );
    assert!(b.min.y.abs() < 1e-9, "on the baseline");
}

#[test]
fn space_advances_and_unknown_chars_fall_back() {
    let f = load(BundledFont::HersheySans);
    let s = style(32.0);
    let tight = f.layout("AB", &s).unwrap();
    let spaced = f.layout("A B", &s).unwrap();
    assert!((spaced.width - tight.width - 16.0).abs() < 1e-9);
    // A character outside the font draws as `?`, never panics.
    let q = f.layout("?", &s).unwrap();
    let odd = f.layout("\u{4e2d}", &s).unwrap();
    assert_eq!(q.contours.len(), odd.contours.len());
}

#[test]
fn user_font_bytes_and_garbage() {
    let ok = OutlineFont::from_bytes(
        "mine",
        include_bytes!("../fonts/figtree/Figtree.ttf").to_vec(),
    );
    let f = Font::Outline(ok.unwrap());
    assert!(!f.layout("abc", &style(5.0)).unwrap().contours.is_empty());
    assert!(OutlineFont::from_bytes("x", Vec::new()).is_err());
    assert!(OutlineFont::from_bytes("x", b"definitely not a font".to_vec()).is_err());
    let mut truncated = include_bytes!("../fonts/figtree/Figtree.ttf").to_vec();
    truncated.truncate(200);
    // A truncated file may load but must never panic when used.
    if let Ok(t) = OutlineFont::from_bytes("x", truncated) {
        let _ = Font::Outline(t).layout("abc", &style(5.0));
    }
}

#[test]
fn limits_and_degenerate_styles() {
    let f = load(BundledFont::HersheySans);
    assert!(matches!(
        f.layout(&"a".repeat(MAX_TEXT_CHARS + 1), &style(5.0)),
        Err(TextError::TextTooLong(_))
    ));
    assert!(matches!(
        f.layout(&"\n".repeat(MAX_TEXT_LINES + 1), &style(5.0)),
        Err(TextError::TextTooLong(_))
    ));
    for bad in [0.0, -3.0, f64::NAN, f64::INFINITY] {
        let g = f.layout("abc", &style(bad)).unwrap();
        assert!(g.contours.is_empty(), "size {bad}");
    }
    assert!(f.layout("", &style(5.0)).unwrap().contours.is_empty());
}

#[test]
fn whitespace_only_outline_text_has_no_contours_but_has_width() {
    let f = load(BundledFont::Figtree);
    let g = f.layout("   ", &style(10.0)).unwrap();
    assert!(g.contours.is_empty());
    assert!(g.width > 0.0);
}

/// Letter spacing (#222): `tracking` mm after every character but the last,
/// for single-line and outline fonts alike; negative tightens.
#[test]
fn tracking_spaces_the_letters() {
    for f in [BundledFont::HersheySans, BundledFont::Figtree] {
        let font = load(f);
        let plain = font.layout("ABCD", &style(10.0)).unwrap();
        let wide = font
            .layout(
                "ABCD",
                &TextStyle {
                    tracking: 2.0,
                    ..style(10.0)
                },
            )
            .unwrap();
        assert!((wide.width - plain.width - 6.0).abs() < 1e-9, "{f:?}");
        let (a, b) = (plain.bbox().unwrap(), wide.bbox().unwrap());
        assert!(
            (b.max.x - a.max.x - 6.0).abs() < 1e-6,
            "{f:?}: the last glyph moved 3 gaps"
        );
        let tight = font
            .layout(
                "ABCD",
                &TextStyle {
                    tracking: -0.5,
                    ..style(10.0)
                },
            )
            .unwrap();
        assert!((plain.width - tight.width - 1.5).abs() < 1e-9, "{f:?}");
    }
}
