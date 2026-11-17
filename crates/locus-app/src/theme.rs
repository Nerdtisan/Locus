// SPDX-License-Identifier: AGPL-3.0-or-later
//! The app's look, from the approved M5 mockup's design tokens (#360, M5
//! design §2): chrome, panels, lines, ink, muted text and the accent, in a
//! light and a dark set. Both are installed, so View ▸ Theme (light, dark,
//! system) switches between them. Widget shapes stay egui's; only colours,
//! corner radii and spacing follow the mockup.

use egui::{Color32, CornerRadius, Stroke, Theme, Visuals};

/// One theme's tokens, named as in the mockup's CSS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    /// Behind the tab strip.
    pub chrome: Color32,
    /// Panes and the ribbon.
    pub panel: Color32,
    /// App bar, pane headers, status bar, hover.
    pub panel_2: Color32,
    pub line: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_ink: Color32,
    /// Hover and selection behind ink.
    pub accent_soft: Color32,
    /// Text fields.
    pub canvas: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const LIGHT: Tokens = Tokens {
    chrome: rgb(0xdfe3e9),
    panel: rgb(0xf6f7f9),
    panel_2: rgb(0xeceff3),
    line: rgb(0xc9cfd8),
    ink: rgb(0x1b2130),
    muted: rgb(0x5f6878),
    accent: rgb(0x1f6fd1),
    accent_ink: rgb(0xffffff),
    accent_soft: rgb(0xd8e6fa),
    canvas: rgb(0xfbfcfd),
};

pub const DARK: Tokens = Tokens {
    chrome: rgb(0x23272f),
    panel: rgb(0x2a2f38),
    panel_2: rgb(0x323844),
    line: rgb(0x3e4552),
    ink: rgb(0xe3e7ee),
    muted: rgb(0x9aa3b3),
    accent: rgb(0x4c93ec),
    accent_ink: rgb(0x0d1420),
    accent_soft: rgb(0x233a5a),
    canvas: rgb(0x1c2027),
};

/// The tokens of the theme `ui` is drawn in.
pub fn tokens(ui: &egui::Ui) -> Tokens {
    if ui.visuals().dark_mode { DARK } else { LIGHT }
}

/// egui visuals from a token set.
pub fn visuals(t: &Tokens, dark: bool) -> Visuals {
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.override_text_color = None;
    v.panel_fill = t.panel;
    v.window_fill = t.panel;
    v.window_stroke = Stroke::new(1.0, t.line);
    v.faint_bg_color = t.panel_2;
    v.extreme_bg_color = t.canvas;
    v.code_bg_color = t.panel_2;
    v.hyperlink_color = t.accent;
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.panel;
    w.noninteractive.weak_bg_fill = t.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.line);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.ink);
    // Buttons sit flat on the panel, as the mockup's `.ib` / `.tb`; hover
    // and active take the soft accent.
    w.inactive.bg_fill = t.panel_2;
    w.inactive.weak_bg_fill = t.panel_2;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, t.ink);
    w.hovered.bg_fill = t.accent_soft;
    w.hovered.weak_bg_fill = t.accent_soft;
    w.hovered.bg_stroke = Stroke::new(1.0, t.line);
    w.hovered.fg_stroke = Stroke::new(1.5, t.ink);
    w.active.bg_fill = t.accent_soft;
    w.active.weak_bg_fill = t.accent_soft;
    w.active.bg_stroke = Stroke::new(1.0, t.accent);
    w.active.fg_stroke = Stroke::new(1.5, t.accent);
    w.open.bg_fill = t.accent_soft;
    w.open.weak_bg_fill = t.accent_soft;
    w.open.bg_stroke = Stroke::new(1.0, t.line);
    w.open.fg_stroke = Stroke::new(1.0, t.ink);
    for s in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        s.corner_radius = CornerRadius::same(5);
    }
    v
}

/// Install both token sets as egui's light and dark visuals.
pub fn install(ctx: &egui::Context) {
    ctx.set_visuals_of(Theme::Light, visuals(&LIGHT, false));
    ctx.set_visuals_of(Theme::Dark, visuals(&DARK, true));
}

/// A panel frame in `fill` with no inner margin change (app bar, status
/// bar, pane headers).
pub fn bar(ui: &egui::Ui, fill: Color32) -> egui::Frame {
    egui::Frame::side_top_panel(ui.style())
        .fill(fill)
        .stroke(Stroke::new(1.0, tokens(ui).line))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The token values are the mockup's.
    #[test]
    fn tokens_are_the_mockups() {
        assert_eq!(LIGHT.accent, Color32::from_rgb(0x1f, 0x6f, 0xd1));
        assert_eq!(DARK.panel, Color32::from_rgb(0x2a, 0x2f, 0x38));
        let v = visuals(&DARK, true);
        assert!(v.dark_mode);
        assert_eq!(v.panel_fill, DARK.panel);
        assert_eq!(v.selection.bg_fill, DARK.accent_soft);
        assert!(!visuals(&LIGHT, false).dark_mode);
    }

    /// Installed for both themes, so View ▸ Theme switches the palette.
    #[test]
    fn both_themes_are_installed() {
        let ctx = egui::Context::default();
        install(&ctx);
        assert_eq!(ctx.style_of(Theme::Dark).visuals.panel_fill, DARK.panel);
        assert_eq!(ctx.style_of(Theme::Light).visuals.panel_fill, LIGHT.panel);
    }
}
