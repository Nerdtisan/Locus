// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bundled UI font (ADR 0001, SPEC §9).
//!
//! egui's default font lacks symbols a CAD app needs (`µ ° Ø ± →`). We embed a
//! Latin + CAD-symbol subset of Noto Sans Regular plus an arrows subset of Noto
//! Sans Symbols (both SIL OFL 1.1, licence texts beside them in `assets/fonts/`). The file is embedded directly rather than
//! pulled in via a crate, so `deny.toml` stays limited to egui's own fonts.
//! Subset with: `pyftsubset` over U+0020-00FF, U+0100-017F, punctuation,
//! arrows U+2190-2194 and maths symbols.
//!
//! egui 0.35's own fonts carry no symbols, so the few UI glyphs beyond those
//! (check and cross marks, warning sign, status dot, menu triangles, return)
//! come from a 2 KB subset of Noto Sans Symbols 2 (OFL 1.1, #385):
//! `pyftsubset NotoSansSymbols2-Regular.ttf --unicodes=U+23CE,U+25B8,U+25BE,
//! U+25CF,U+26A0,U+2713,U+2715,U+2716,U+2717 --no-hinting --layout-features=''`.
//! A test scans the app's string literals so a new symbol can't ship as an
//! empty box.

use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

const NAME: &str = "noto-sans-subset";
const NOTO: &[u8] = include_bytes!("../assets/fonts/NotoSans-Subset.ttf");
// Noto Sans has no arrows; this 2 KB subset (U+2190-2195, U+2300) adds them.
const SYMBOLS_NAME: &str = "noto-sans-symbols-subset";
const SYMBOLS: &[u8] = include_bytes!("../assets/fonts/NotoSansSymbols-Subset.ttf");
const SYMBOLS2_NAME: &str = "noto-sans-symbols2-subset";
const SYMBOLS2: &[u8] = include_bytes!("../assets/fonts/NotoSansSymbols2-Subset.ttf");

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert(NAME.to_owned(), Arc::new(FontData::from_static(NOTO)));
    fonts.font_data.insert(
        SYMBOLS_NAME.to_owned(),
        Arc::new(FontData::from_static(SYMBOLS)),
    );
    fonts.font_data.insert(
        SYMBOLS2_NAME.to_owned(),
        Arc::new(FontData::from_static(SYMBOLS2)),
    );
    // First for proportional text (consistent look), last-resort fallback for
    // monospace so rulers and numeric fields still get the symbols.
    if let Some(f) = fonts.families.get_mut(&FontFamily::Proportional) {
        f.insert(0, NAME.to_owned());
        f.insert(1, SYMBOLS_NAME.to_owned());
        f.insert(2, SYMBOLS2_NAME.to_owned());
    }
    if let Some(f) = fonts.families.get_mut(&FontFamily::Monospace) {
        f.push(NAME.to_owned());
        f.push(SYMBOLS_NAME.to_owned());
        f.push(SYMBOLS2_NAME.to_owned());
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Symbols the UI relies on (ADR 0001).
    const CAD_SYMBOLS: [char; 7] = ['µ', '°', 'Ø', '±', '→', '×', '−'];

    #[test]
    fn embedded_font_covers_cad_symbols() {
        let ctx = egui::Context::default();
        install(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            for c in CAD_SYMBOLS {
                let has = ui.fonts_mut(|f| f.has_glyph(&egui::FontId::proportional(14.0), c));
                assert!(has, "missing glyph {c:?}");
            }
        });
    }

    /// Non-ASCII characters inside string and char literals of `src`
    /// (comments skipped), each with the first file that uses it.
    fn literal_chars() -> std::collections::BTreeMap<char, String> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let mut found = std::collections::BTreeMap::new();
        for f in files {
            let Ok(src) = std::fs::read_to_string(&f) else {
                continue;
            };
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned());
            let cs: Vec<char> = src.chars().collect();
            let mut i = 0;
            let mut in_str = false;
            while i < cs.len() {
                let c = cs[i];
                if in_str {
                    match c {
                        '\\' => i += 1,
                        '"' => in_str = false,
                        c if !c.is_ascii() => {
                            found
                                .entry(c)
                                .or_insert_with(|| name.clone().unwrap_or_default());
                        }
                        _ => {}
                    }
                } else if c == '/' && cs.get(i + 1) == Some(&'/') {
                    while i < cs.len() && cs[i] != '\n' {
                        i += 1;
                    }
                } else if c == '"' {
                    in_str = true;
                } else if c == '\'' && cs.get(i + 2) == Some(&'\'') && !cs[i + 1].is_ascii() {
                    found
                        .entry(cs[i + 1])
                        .or_insert_with(|| name.clone().unwrap_or_default());
                    i += 2;
                }
                i += 1;
            }
        }
        found
    }

    /// Every non-ASCII character the app puts in a string has a glyph in
    /// the installed fonts, so nothing draws as an empty box (#385).
    #[test]
    fn every_ui_string_character_has_a_glyph() {
        let chars = literal_chars();
        assert!(chars.contains_key(&'°'), "scan found nothing");
        let ctx = egui::Context::default();
        install(&ctx);
        let mut missing = Vec::new();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            for (c, file) in &chars {
                let has = ui.fonts_mut(|f| f.has_glyph(&egui::FontId::proportional(14.0), *c));
                if !has {
                    missing.push(format!("U+{:04X} {c} ({file})", *c as u32));
                }
            }
        });
        assert!(missing.is_empty(), "no glyph for:\n{}", missing.join("\n"));
    }
}
