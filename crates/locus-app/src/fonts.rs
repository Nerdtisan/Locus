// SPDX-License-Identifier: AGPL-3.0-or-later
//! Bundled UI font (ADR 0001, SPEC §9).
//!
//! egui's default font lacks symbols a CAD app needs (`µ ° Ø ± →`). We embed a
//! Latin + CAD-symbol subset of Noto Sans Regular plus an arrows subset of Noto
//! Sans Symbols (both SIL OFL 1.1, licence texts beside them in `assets/fonts/`). The file is embedded directly rather than
//! pulled in via a crate, so `deny.toml` stays limited to egui's own fonts.
//! Subset with: `pyftsubset` over U+0020-00FF, U+0100-017F, punctuation,
//! arrows U+2190-2194 and maths symbols.

use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

const NAME: &str = "noto-sans-subset";
const NOTO: &[u8] = include_bytes!("../assets/fonts/NotoSans-Subset.ttf");
// Noto Sans has no arrows; this 2 KB subset (U+2190-2195, U+2300) adds them.
const SYMBOLS_NAME: &str = "noto-sans-symbols-subset";
const SYMBOLS: &[u8] = include_bytes!("../assets/fonts/NotoSansSymbols-Subset.ttf");

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert(NAME.to_owned(), Arc::new(FontData::from_static(NOTO)));
    fonts.font_data.insert(
        SYMBOLS_NAME.to_owned(),
        Arc::new(FontData::from_static(SYMBOLS)),
    );
    // First for proportional text (consistent look), last-resort fallback for
    // monospace so rulers and numeric fields still get the symbols.
    if let Some(f) = fonts.families.get_mut(&FontFamily::Proportional) {
        f.insert(0, NAME.to_owned());
        f.insert(1, SYMBOLS_NAME.to_owned());
    }
    if let Some(f) = fonts.families.get_mut(&FontFamily::Monospace) {
        f.push(NAME.to_owned());
        f.push(SYMBOLS_NAME.to_owned());
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
}
