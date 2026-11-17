// SPDX-License-Identifier: AGPL-3.0-or-later
//! The app's look, from the approved M5 mockup's design tokens (#360, M5
//! design §2): chrome, panels, lines, ink, muted text and the accent, in a
//! light and a dark set. Both are installed, so View ▸ Theme (light, dark,
//! system) switches between them.
//!
//! Styling pass 2 (#384) makes the controls the mockup's, from this one
//! place: text and number fields (`.fld`: canvas fill, 1 px line border,
//! radius 4, accent border when focused), buttons (`.btn`), and three
//! shared widgets egui's style can't express: [`checkbox`] (`.ck`, filled
//! with the accent when ticked), [`segmented`] (`.seg`, a rounded, bordered
//! group, the chosen option filled) and [`swatch`] (`.swbtn`, a 22 px
//! rounded colour button). `clippy.toml` disallows egui's own check box
//! and colour button, so no panel styles its own.

use egui::{
    Color32, CornerRadius, Response, RichText, Sense, Stroke, StrokeKind, Theme, Ui, Visuals,
    Widget, WidgetInfo, WidgetText, WidgetType, pos2, vec2,
};

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
    // Fields and buttons as the mockup's `.fld` / `.btn` (#384): the canvas
    // colour inside a 1 px line border. Icon buttons (`.ib`, `.tb`) stay
    // frameless until hovered, as egui's selectable buttons are; hover and
    // active take the soft accent.
    w.inactive.bg_fill = t.canvas;
    w.inactive.weak_bg_fill = t.canvas;
    w.inactive.bg_stroke = Stroke::new(1.0, t.line);
    w.inactive.fg_stroke = Stroke::new(1.0, t.ink);
    w.hovered.bg_fill = t.canvas;
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
        s.corner_radius = CornerRadius::same(FIELD_RADIUS);
        // No grow-on-hover: fields keep their size, as in the mockup.
        s.expansion = 0.0;
    }
    v.text_edit_bg_color = Some(t.canvas);
    v
}

/// `.fld` / `.btn` corner radius.
const FIELD_RADIUS: u8 = 4;
/// `.seg` corner radius.
const SEG_RADIUS: u8 = 5;
/// `.ck` box: 13 px, radius 3.
const CHECK_SIZE: f32 = 13.0;
const CHECK_RADIUS: u8 = 3;
/// `.swbtn`: 22 px, radius 4.
const SWATCH: f32 = 22.0;

/// The mockup's control sizes (#384): 22 px fields and rows (`.fld`
/// height), 6 px horizontal padding, 13 px check boxes.
pub fn spacing(s: &mut egui::style::Spacing) {
    s.interact_size.y = 22.0;
    s.button_padding = vec2(6.0, 3.0);
    s.icon_width = CHECK_SIZE;
    s.icon_width_inner = 7.0;
    s.icon_spacing = 6.0;
}

/// Install both token sets as egui's light and dark visuals, with the
/// mockup's control sizes.
pub fn install(ctx: &egui::Context) {
    for (theme, t, dark) in [(Theme::Light, &LIGHT, false), (Theme::Dark, &DARK, true)] {
        ctx.style_mut_of(theme, |s| {
            s.visuals = visuals(t, dark);
            spacing(&mut s.spacing);
        });
    }
}

/// The mockup's check box (`.ck`, #384): a 13 px rounded box in the canvas
/// colour with a line border; ticked, it fills with the accent and the tick
/// is drawn in the accent's ink. `mixed` draws a dash (a mixed selection).
pub struct Checkbox<'a> {
    checked: &'a mut bool,
    text: WidgetText,
    mixed: bool,
}

impl<'a> Checkbox<'a> {
    pub fn new(checked: &'a mut bool, text: impl Into<WidgetText>) -> Self {
        Self {
            checked,
            text: text.into(),
            mixed: false,
        }
    }

    pub fn mixed(mut self, mixed: bool) -> Self {
        self.mixed = mixed;
        self
    }
}

impl Widget for Checkbox<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let Checkbox {
            checked,
            text,
            mixed,
        } = self;
        let gap = ui.spacing().icon_spacing;
        let wrap = ui.available_width() - CHECK_SIZE - gap;
        let galley =
            (!text.is_empty()).then(|| text.into_galley(ui, None, wrap, egui::TextStyle::Button));
        let text_w = galley.as_ref().map_or(0.0, |g| gap + g.size().x);
        let h = galley
            .as_ref()
            .map_or(0.0, |g| g.size().y)
            .max(ui.spacing().interact_size.y);
        let (rect, mut resp) = ui.allocate_exact_size(vec2(CHECK_SIZE + text_w, h), Sense::click());
        if resp.clicked() {
            *checked = !*checked;
            resp.mark_changed();
        }
        let label = galley
            .as_ref()
            .map_or_else(String::new, |g| g.text().to_owned());
        resp.widget_info(|| {
            WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *checked, &label)
        });
        if ui.is_rect_visible(rect) {
            let t = tokens(ui);
            let on = *checked || mixed;
            let b = egui::Rect::from_center_size(
                pos2(rect.left() + CHECK_SIZE / 2.0, rect.center().y),
                vec2(CHECK_SIZE, CHECK_SIZE),
            );
            let (fill, edge) = if on {
                (t.accent, t.accent)
            } else if resp.hovered() {
                (t.canvas, t.accent)
            } else {
                (t.canvas, t.line)
            };
            let p = ui.painter();
            p.rect(
                b,
                CHECK_RADIUS,
                fill,
                Stroke::new(1.0, edge),
                StrokeKind::Inside,
            );
            let ink = Stroke::new(1.6, t.accent_ink);
            let at = |x: f32, y: f32| pos2(b.left() + x * b.width(), b.top() + y * b.height());
            if mixed {
                p.line_segment([at(0.27, 0.5), at(0.73, 0.5)], ink);
            } else if *checked {
                p.add(egui::Shape::line(
                    vec![at(0.24, 0.52), at(0.43, 0.7), at(0.77, 0.32)],
                    ink,
                ));
            }
            if let Some(g) = galley {
                let color = ui.style().interact(&resp).text_color();
                let top = rect.center().y - g.size().y / 2.0;
                p.galley(pos2(b.right() + gap, top), g, color);
            }
        }
        resp
    }
}

/// `ui.add(Checkbox::new(checked, text))`: the app's only check box.
pub fn checkbox(ui: &mut Ui, checked: &mut bool, text: impl Into<WidgetText>) -> Response {
    ui.add(Checkbox::new(checked, text))
}

/// The mockup's segmented switch (`.seg`, #384): one button per option in
/// a rounded group with a line border and dividers, the chosen one filled
/// with the accent. Returns the option clicked, if it differs from
/// `current`.
pub fn segmented<T: Copy + PartialEq>(ui: &mut Ui, current: T, options: &[(T, &str)]) -> Option<T> {
    let t = tokens(ui);
    let mut picked = None;
    let n = options.len();
    let group = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let mut rects = Vec::with_capacity(n);
        for (i, &(v, label)) in options.iter().enumerate() {
            let on = v == current;
            let text = RichText::new(label).size(11.0);
            let text = if on { text.color(t.accent_ink) } else { text };
            let r = SEG_RADIUS - 1;
            let corner = CornerRadius {
                nw: if i == 0 { r } else { 0 },
                sw: if i == 0 { r } else { 0 },
                ne: if i + 1 == n { r } else { 0 },
                se: if i + 1 == n { r } else { 0 },
            };
            let b = egui::Button::new(text)
                .fill(if on { t.accent } else { t.canvas })
                .stroke(Stroke::NONE)
                .corner_radius(corner)
                .min_size(vec2(0.0, 20.0));
            let resp = ui.add(b);
            if resp.clicked() && !on {
                picked = Some(v);
            }
            rects.push(resp.rect);
        }
        rects
    });
    let p = ui.painter();
    let line = Stroke::new(1.0, t.line);
    for r in group.inner.iter().skip(1) {
        p.vline(r.left(), r.y_range(), line);
    }
    if let (Some(first), Some(last)) = (group.inner.first(), group.inner.last()) {
        p.rect_stroke(first.union(*last), SEG_RADIUS, line, StrokeKind::Inside);
    }
    picked
}

/// The mockup's colour button (`.swbtn`, #384): a 22 px swatch, radius 4,
/// with a line border; a click opens egui's colour picker.
pub fn swatch(ui: &mut Ui, srgb: &mut [u8; 3]) -> Response {
    swatch_sized(ui, srgb, SWATCH)
}

/// A swatch `size` px square (the Browser's layer rows use the mockup's
/// small `.sw`).
pub fn swatch_sized(ui: &mut Ui, srgb: &mut [u8; 3], size: f32) -> Response {
    let radius = if size < SWATCH { 2 } else { FIELD_RADIUS };
    let resp = ui
        .scope(|ui| {
            ui.spacing_mut().interact_size = vec2(size, size);
            #[allow(clippy::disallowed_methods)] // wrapped here, once
            ui.color_edit_button_srgb(srgb)
        })
        .inner;
    if ui.is_rect_visible(resp.rect) {
        // Repaint egui's square-cornered button as the mockup's swatch.
        let t = tokens(ui);
        let p = ui.painter();
        let r = resp.rect;
        p.rect_filled(r, 0.0, ui.visuals().panel_fill);
        p.rect_filled(
            r.shrink(1.0),
            radius - 1,
            Color32::from_rgb(srgb[0], srgb[1], srgb[2]),
        );
        let edge = if resp.hovered() { t.accent } else { t.line };
        p.rect_stroke(r, radius, Stroke::new(1.0, edge), StrokeKind::Inside);
    }
    resp
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

    /// #384: fields and buttons are the mockup's `.fld` / `.btn` (canvas
    /// fill, 1 px line border, radius 4, accent border when focused) and the
    /// controls its sizes, in both themes, from this one style.
    #[test]
    fn controls_are_the_mockups() {
        let ctx = egui::Context::default();
        install(&ctx);
        for (theme, t) in [(Theme::Light, LIGHT), (Theme::Dark, DARK)] {
            let s = ctx.style_of(theme);
            let w = &s.visuals.widgets.inactive;
            assert_eq!(w.weak_bg_fill, t.canvas);
            assert_eq!(w.bg_stroke, Stroke::new(1.0, t.line));
            assert_eq!(w.corner_radius, CornerRadius::same(4));
            assert_eq!(s.visuals.text_edit_bg_color(), t.canvas);
            assert_eq!(s.visuals.selection.stroke.color, t.accent);
            assert_eq!(s.spacing.interact_size.y, 22.0);
            assert_eq!(s.spacing.icon_width, 13.0);
        }
    }

    /// The shared check box toggles, reports itself as a check box, and is
    /// filled with the accent when ticked.
    #[test]
    #[allow(clippy::expect_used)]
    fn the_check_box_toggles_and_is_a_check_box() {
        let ctx = egui::Context::default();
        install(&ctx);
        ctx.enable_accesskit();
        let mut on = false;
        let run = |on: &mut bool, click: bool| {
            let mut input = egui::RawInput::default();
            if click {
                let at = pos2(5.0, 11.0);
                input.events = vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ];
            }
            ctx.run_ui(input, |ui| {
                checkbox(ui, on, "Snap");
            })
        };
        let _ = run(&mut on, false);
        let out = run(&mut on, true);
        assert!(on, "a click on the box ticks it");
        let tree = out.platform_output.accesskit_update.expect("accesskit");
        assert!(tree.nodes.iter().any(|(_, n)| {
            n.role() == egui::accesskit::Role::CheckBox && n.label() == Some("Snap")
        }));
    }

    /// The shared segmented switch reports a click on another option only.
    #[test]
    fn segmented_reports_a_new_choice() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut got = None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            got = segmented(ui, 1, &[(1, "mm"), (2, "in")]);
        });
        assert_eq!(got, None, "nothing clicked");
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
