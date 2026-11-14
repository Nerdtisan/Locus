// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Text dialog (#222, design §6.5, SPEC §9).
//!
//! - **With the Text tool,** the dialog holds the settings for the next text:
//!   content, the font list, size, alignment, line spacing and letter
//!   spacing. Clicking the canvas places the text.
//! - **After placing,** the dialog switches to the new text with its content
//!   focused, so you type straight in.
//! - **Double-clicking placed text** reopens the dialog. Edit text in the
//!   inspector shows the same editor.
//!
//! Each change is one command (SPEC §11).

use crate::state::AppState;
use crate::tools::Tool;
use crate::views::text_ui;
use egui::Rect;

impl AppState {
    /// Open the dialog on placed text `id` (double-click, or just placed).
    pub(crate) fn open_text_dialog(&mut self, id: locus_doc::EntityId, focus: bool) {
        if self.doc.sketcher.document().text(id).is_some() {
            self.selection.items.set_all([id]);
            self.tools.text_dialog = Some(id);
            self.tools.text_focus = focus;
        }
    }

    /// Close the dialog; returns whether it was open.
    pub(crate) fn close_text_dialog(&mut self) -> bool {
        self.tools.text_focus = false;
        self.tools.text_dialog.take().is_some()
    }

    pub(crate) fn text_dialog(&mut self, ui: &egui::Ui, rect: Rect) {
        if let Some(id) = self.tools.text_dialog {
            let Some((text, anchor, dir)) = self
                .doc
                .sketcher
                .document()
                .text(id)
                .map(|(t, a, d)| (t.clone(), a, d))
            else {
                // Deleted or undone away.
                self.close_text_dialog();
                return;
            };
            let focus = std::mem::take(&mut self.tools.text_focus);
            let mut close = false;
            super::dialog::command_dialog(ui, rect, "Edit Text", |ui| {
                self.edit_text(ui, id, anchor, dir, &text, focus);
                ui.separator();
                close = ui.button("Close").clicked();
            });
            if close {
                self.close_text_dialog();
            }
            return;
        }
        if self.tool() == Tool::Text {
            let mut done = false;
            super::dialog::command_dialog(ui, rect, "Text", |ui| {
                ui.weak("Click on the canvas to place the text.");
                text_ui::controls(
                    ui,
                    &mut self.tools.text_settings,
                    &mut self.tools.fonts,
                    true,
                    3,
                    false,
                );
                ui.separator();
                done = ui.button("Close").clicked();
            });
            if done {
                self.set_tool(Tool::Select);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)] // a missing fixture should fail loudly
mod tests {
    use super::*;
    use crate::settings::Units;
    use crate::tool::ToolEvent;
    use crate::tools::Pick;
    use locus_doc::Document;
    use locus_geom::Vec2;

    fn placed() -> (AppState, locus_doc::EntityId) {
        let mut c = AppState::default();
        c.load(Document::new());
        c.set_tool(Tool::Text);
        c.tools.text_settings.content = "Hi".to_owned();
        c.tools.text_settings.tracking = 1.0;
        c.tool_event(ToolEvent::Click(Pick::free(Vec2::ZERO)), None, Units::Mm);
        let id = c
            .tools
            .text_dialog
            .expect("the dialog opens on the new text");
        (c, id)
    }

    /// Placing text opens it for typing (design §6.5), and its letter
    /// spacing is kept on the document (format v14).
    #[test]
    fn placing_text_opens_the_dialog_on_it() {
        let (c, id) = placed();
        assert!(c.tools.text_focus, "the content takes focus");
        assert_eq!(c.tool(), Tool::Select);
        let (t, ..) = c.doc.sketcher.document().text(id).expect("a text");
        assert_eq!(t.content, "Hi");
        assert_eq!(t.tracking, 1.0);
    }

    #[test]
    fn the_dialog_reopens_and_closes() {
        let (mut c, id) = placed();
        assert!(c.close_text_dialog());
        assert!(!c.close_text_dialog());
        c.open_text_dialog(id, false);
        assert_eq!(c.tools.text_dialog, Some(id));
        // A deleted text closes its dialog.
        c.delete_selection();
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            c.text_dialog(ui, ui.max_rect());
        });
        assert!(c.tools.text_dialog.is_none());
    }
}
