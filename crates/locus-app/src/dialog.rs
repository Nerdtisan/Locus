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
//!
//! Size (#361): a fixed-size dialog fits its content. A resizable one opens
//! at its default size (a roomy [`RESIZABLE_SIZE`] unless the call site
//! gives one), and on the desktop at the size the user last left it, kept
//! in egui's persisted memory per dialog title. On the web the in-app
//! window keeps its size the same way. Stored sizes are bounded.

use egui::{Align2, Context, Id, Pos2, Ui, Vec2, ViewportBuilder, ViewportCommand, ViewportId};

/// Margin between the content and the OS window edge, in points.
const MARGIN: f32 = 8.0;
/// A fixed-size dialog's starting size; it then fits its content.
const DEFAULT_SIZE: Vec2 = Vec2::new(360.0, 160.0);
/// A resizable dialog's size when the call site gives none: room for a
/// list or a form without the user resizing first (#361).
const RESIZABLE_SIZE: Vec2 = Vec2::new(560.0, 420.0);
const MIN_SIZE: Vec2 = Vec2::new(200.0, 80.0);
/// Largest remembered size (stored data is untrusted).
const MAX_SIZE: Vec2 = Vec2::new(4000.0, 3000.0);

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

    /// The size with nothing given: roomy if resizable, else small (it
    /// fits its content).
    fn fallback_size(&self) -> Vec2 {
        if self.resizable {
            RESIZABLE_SIZE
        } else {
            DEFAULT_SIZE
        }
    }

    /// The size to open at.
    fn initial_size(&self) -> Vec2 {
        self.default_size.unwrap_or_else(|| self.fallback_size())
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
        let size = self.initial_size();
        // A fresh id (#361): windows sized before default sizes existed had
        // their cramped size stored under the bare title, which egui would
        // keep reusing. From here the user's own resize is what is kept.
        let id = Id::new(("locus-dialog", self.title.as_str()));
        let mut w = egui::Window::new(self.title.as_str())
            .id(id)
            .resizable(self.resizable)
            .collapsible(self.collapsible);
        if let Some((a, o)) = self.anchor {
            w = w.anchor(a, o);
        }
        if self.resizable {
            // egui keeps a resized window's size (persisted with memory).
            w = w.default_size(size);
        } else {
            // A fixed dialog fits its content's height; its width is set,
            // or its text would wrap into a narrow column.
            w = w.default_width(size.x);
        }
        if let Some(open) = self.open {
            w = w.open(open);
        }
        let fill = self.resizable;
        w.show(ctx, |ui| {
            let r = add(ui);
            // A resizable window would shrink to short content; fill it, so
            // it opens at its size and then follows the user's (#361).
            if fill {
                ui.allocate_space(ui.available_size());
            }
            r
        })
        .and_then(|r| r.inner)
    }

    fn show_os<R>(self, ctx: &Context, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
        let id = ViewportId::from_hash_of(("locus-dialog", &self.title));
        // The size the user last left a resizable dialog at (#361).
        let size_key = Id::new(id).with("size");
        let remembered = if self.resizable {
            ctx.data_mut(|d| d.get_persisted::<Vec2>(size_key))
        } else {
            None
        };
        // Fixed for as long as the window is open: a builder that changes
        // makes egui resize (or move) the window every frame.
        let open_key = Id::new(id).with("open-size");
        let default = self.initial_size();
        let size = ctx.data_mut(|d| {
            *d.get_temp_mut_or_insert_with(open_key, || opening_size(default, remembered))
        });
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
        let remember = self.resizable;
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
            if remember && let Some(now) = ui.input(|i| i.viewport().inner_rect.map(|r| r.size())) {
                ui.ctx().data_mut(|d| d.insert_persisted(size_key, now));
            }
            (r, ui.input(|i| i.viewport().close_requested()))
        });
        if closed && let Some(open) = self.open {
            *open = false;
            // Re-centre next time it opens, at the size it was left.
            ctx.data_mut(|d| {
                d.remove::<Pos2>(Id::new(id).with("pos"));
                d.remove::<Vec2>(open_key);
            });
        }
        out
    }
}

/// The size a dialog opens at: the one the user left it at, if usable,
/// else its default. A stored size is bounded (untrusted memory).
fn opening_size(default: Vec2, remembered: Option<Vec2>) -> Vec2 {
    remembered
        .filter(|s| s.x.is_finite() && s.y.is_finite())
        .map_or(default, |s| s.clamp(MIN_SIZE, MAX_SIZE))
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

    /// #361: a resizable dialog without a size opens roomy, not 360 × 160;
    /// a given size wins.
    #[test]
    fn resizable_dialogs_open_at_a_usable_size() {
        let d = Dialog::new("Plugin manager").resizable(true);
        assert_eq!(d.initial_size(), RESIZABLE_SIZE);
        let d = Dialog::new("MCP server")
            .resizable(true)
            .default_size([480.0, 520.0]);
        assert_eq!(d.initial_size(), Vec2::new(480.0, 520.0));
        let d = Dialog::new("Error").resizable(false);
        assert_eq!(d.initial_size(), DEFAULT_SIZE);
    }

    /// #361: the user's size wins over the default, bounded.
    #[test]
    fn a_remembered_size_is_kept_and_bounded() {
        let def = Vec2::new(500.0, 400.0);
        assert_eq!(opening_size(def, None), def);
        assert_eq!(
            opening_size(def, Some(Vec2::new(900.0, 700.0))),
            Vec2::new(900.0, 700.0)
        );
        assert_eq!(
            opening_size(def, Some(Vec2::new(1.0, 1e9))),
            Vec2::new(MIN_SIZE.x, MAX_SIZE.y)
        );
        assert_eq!(opening_size(def, Some(Vec2::new(f32::NAN, 10.0))), def);
    }

    /// #361 on the web: an in-app resizable dialog opens at its default
    /// size, not shrunk to its content.
    /// A fixed dialog keeps a readable width.
    #[test]
    #[allow(clippy::expect_used)]
    fn a_fixed_in_app_dialog_is_not_a_narrow_column() {
        let ctx = Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(1600.0, 1000.0),
            )),
            ..Default::default()
        };
        for _ in 0..3 {
            let _ = ctx.run_ui(input(), |ui| {
                Dialog::new("Restored")
                    .resizable(false)
                    .show(ui.ctx(), |ui| {
                        ui.label("Restored your autosaved sketch from this browser.")
                    });
            });
        }
        let rect = ctx
            .memory(|m| m.area_rect(Id::new(("locus-dialog", "Restored"))))
            .expect("the window was drawn");
        assert!(rect.width() >= DEFAULT_SIZE.x - 20.0, "{rect:?}");
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn an_in_app_dialog_opens_at_its_default_size() {
        let ctx = Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(1600.0, 1000.0),
            )),
            ..Default::default()
        };
        for _ in 0..3 {
            let _ = ctx.run_ui(input(), |ui| {
                Dialog::new("Plugin manager")
                    .resizable(true)
                    .default_size([680.0, 480.0])
                    .show(ui.ctx(), |ui| ui.label("No plugins installed."));
            });
        }
        let rect = ctx
            .memory(|m| m.area_rect(Id::new(("locus-dialog", "Plugin manager"))))
            .expect("the window was drawn");
        assert!(rect.width() >= 660.0, "{rect:?}");
        assert!(rect.height() >= 440.0, "{rect:?}");
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
