// SPDX-License-Identifier: AGPL-3.0-or-later
//! Dialogs as real OS windows (owner request, M5; design §2.2).
//!
//! The owner wants every window movable outside the app: on the desktop a
//! dialog is an egui immediate viewport, i.e. its own OS window that can go
//! to another monitor. The web has one window, so there (and wherever
//! eframe embeds viewports) it falls back to an in-app [`egui::Window`].
//!
//! [`Dialog`] mirrors the `egui::Window` builder calls the app uses, so a
//! call site changes only its constructor.

use egui::{Align2, Context, Id, Pos2, Ui, Vec2, ViewportBuilder, ViewportCommand, ViewportId};

/// Margin between the content and the OS window edge, in points.
const MARGIN: f32 = 8.0;
const DEFAULT_SIZE: Vec2 = Vec2::new(360.0, 160.0);
const MIN_SIZE: Vec2 = Vec2::new(200.0, 80.0);

pub struct Dialog<'open> {
    title: String,
    open: Option<&'open mut bool>,
    resizable: bool,
    collapsible: bool,
    anchor: Option<(Align2, Vec2)>,
    default_size: Option<Vec2>,
}

impl<'open> Dialog<'open> {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            open: None,
            resizable: true,
            collapsible: true,
            anchor: None,
            default_size: None,
        }
    }

    /// A close button that clears `open`; without it the dialog closes only
    /// through its own buttons.
    pub fn open(mut self, open: &'open mut bool) -> Self {
        self.open = Some(open);
        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    pub fn collapsible(mut self, collapsible: bool) -> Self {
        self.collapsible = collapsible;
        self
    }

    /// Where the in-app window sits. An OS window opens centred on the
    /// main window instead.
    pub fn anchor(mut self, align: Align2, offset: impl Into<Vec2>) -> Self {
        self.anchor = Some((align, offset.into()));
        self
    }

    // Used by the desktop-only dialogs (MCP, plugins).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn default_width(mut self, w: f32) -> Self {
        let h = self.default_size.map_or(DEFAULT_SIZE.y, |s| s.y);
        self.default_size = Some(Vec2::new(w, h));
        self
    }

    pub fn default_size(mut self, size: impl Into<Vec2>) -> Self {
        self.default_size = Some(size.into());
        self
    }

    /// Draw the dialog. `None` when it is closed or not drawn.
    pub fn show<R>(self, ctx: &Context, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
        if self.open.as_deref() == Some(&false) {
            return None;
        }
        if ctx.embed_viewports() {
            self.show_in_app(ctx, add)
        } else {
            self.show_os(ctx, add)
        }
    }

    fn show_in_app<R>(self, ctx: &Context, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
        let mut w = egui::Window::new(self.title)
            .resizable(self.resizable)
            .collapsible(self.collapsible);
        if let Some((a, o)) = self.anchor {
            w = w.anchor(a, o);
        }
        if let Some(s) = self.default_size {
            w = w.default_size(s);
        }
        if let Some(open) = self.open {
            w = w.open(open);
        }
        w.show(ctx, add).and_then(|r| r.inner)
    }

    fn show_os<R>(self, ctx: &Context, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
        let id = ViewportId::from_hash_of(("locus-dialog", &self.title));
        let size = self.default_size.unwrap_or(DEFAULT_SIZE);
        // Centred on the main window when it first opens; remembered so
        // the builder doesn't change (which would move the window) while
        // the main window moves.
        let main = ctx.input(|i| i.viewport().inner_rect);
        let pos = ctx.data_mut(|d| {
            *d.get_temp_mut_or_insert_with(Id::new(id).with("pos"), || {
                main.map_or(Pos2::new(200.0, 200.0), |r| centred(r, size))
            })
        });
        let builder = ViewportBuilder::default()
            .with_title(self.title.clone())
            .with_position(pos)
            .with_inner_size(size)
            .with_min_inner_size(MIN_SIZE)
            .with_resizable(self.resizable)
            .with_close_button(self.open.is_some())
            .with_minimize_button(false);
        let fit = !self.resizable;
        let mut add = Some(add);
        let (out, closed) = ctx.show_viewport_immediate(id, builder, |ui, _class| {
            let inner = egui::CentralPanel::default_margins().show(ui, |ui| {
                let r = add.take().map(|f| f(ui));
                (r, ui.min_rect())
            });
            let (r, used) = inner.inner;
            // A fixed-size dialog fits its content, as an egui::Window would.
            if fit {
                let want = (used.max.to_vec2() + Vec2::splat(MARGIN)).max(MIN_SIZE);
                let have = ui.input(|i| i.viewport().inner_rect.map(|r| r.size()));
                if have.is_some_and(|h| (h - want).length() > 1.0) {
                    ui.ctx().send_viewport_cmd_to(
                        ui.ctx().viewport_id(),
                        ViewportCommand::InnerSize(want),
                    );
                }
            }
            (r, ui.input(|i| i.viewport().close_requested()))
        });
        if closed && let Some(open) = self.open {
            *open = false;
            // Re-centre next time it opens.
            ctx.data_mut(|d| d.remove::<Pos2>(Id::new(id).with("pos")));
        }
        out
    }
}

/// The top-left that centres a `size` window on `parent`.
fn centred(parent: egui::Rect, size: Vec2) -> Pos2 {
    parent.center() - size / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialogs_open_centred_on_the_main_window() {
        let main = egui::Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(1000.0, 800.0));
        assert_eq!(
            centred(main, Vec2::new(200.0, 100.0)),
            Pos2::new(500.0, 400.0)
        );
    }

    /// Where viewports are embedded (the web, tests) a dialog is an in-app
    /// window: it draws, and a closed one doesn't.
    #[test]
    fn embedded_dialogs_draw_in_app_and_respect_open() {
        let ctx = Context::default();
        assert!(ctx.embed_viewports());
        let mut drawn = false;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let r = Dialog::new("Test").show(ui.ctx(), |_| {
                drawn = true;
                7
            });
            assert_eq!(r, Some(7));
        });
        assert!(drawn);
        let mut open = false;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let r = Dialog::new("Closed").open(&mut open).show(ui.ctx(), |_| 1);
            assert_eq!(r, None);
        });
    }
}
