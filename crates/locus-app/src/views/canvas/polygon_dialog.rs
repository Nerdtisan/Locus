// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Polygon command dialog (#389, M5 design §2): while a polygon tool
//! runs, the canvas's top-right corner shows its kind (Circumscribed,
//! Inscribed or Edge, Fusion's three polygon tools, also in CREATE ▾) and
//! its side count. The options used to sit only in the Sketch palette,
//! where the owner couldn't find them.

use super::dialog::{Header, close_footer, command_dialog, row, segmented};
use crate::state::AppState;
use crate::tools::Tool;

/// Which of Fusion's polygon tools is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Circumscribed,
    Inscribed,
    Edge,
}

impl Kind {
    fn of(tool: Tool, circumscribed: bool) -> Option<Self> {
        match (tool, circumscribed) {
            (Tool::PolygonEdge, _) => Some(Kind::Edge),
            (Tool::Polygon, true) => Some(Kind::Circumscribed),
            (Tool::Polygon, false) => Some(Kind::Inscribed),
            _ => None,
        }
    }

    /// The registry action (its icon and name head the dialog).
    fn action(self) -> (&'static str, &'static str) {
        match self {
            Kind::Circumscribed => ("sketch.polygon_circumscribed", "Circumscribed Polygon"),
            Kind::Inscribed => ("sketch.polygon", "Inscribed Polygon"),
            Kind::Edge => ("sketch.polygon_edge", "Edge Polygon"),
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Kind::Circumscribed => "Click the centre, then the middle of a side.",
            Kind::Inscribed => "Click the centre, then a vertex.",
            Kind::Edge => "Click a side's two ends, then the side it goes on.",
        }
    }
}

impl AppState {
    pub(crate) fn polygon_dialog(&mut self, ui: &egui::Ui, rect: egui::Rect) {
        let Some((mut sides, outside)) = self.tools.host.tool().polygon() else {
            return;
        };
        let Some(now) = Kind::of(self.tool(), outside) else {
            return;
        };
        let (icon, title) = now.action();
        let mut pick = None;
        let mut resized = false;
        let mut close = false;
        command_dialog(
            ui,
            rect,
            Header::new(icon, title),
            |_| {},
            |ui| {
                row(ui, "Type", |ui| {
                    pick = segmented(
                        ui,
                        now,
                        &[
                            (Kind::Circumscribed, "Circumscribed"),
                            (Kind::Inscribed, "Inscribed"),
                            (Kind::Edge, "Edge"),
                        ],
                    );
                });
                row(ui, "Sides", |ui| {
                    resized = ui
                        .add(egui::DragValue::new(&mut sides).range(3..=64))
                        .on_hover_text("Also typed while drawing: Tab to the Sides field")
                        .changed();
                });
                ui.weak(now.hint());
                close = close_footer(ui);
            },
        );
        if resized {
            self.tools
                .host
                .tool_mut()
                .set_polygon_options(sides, outside);
        }
        match pick {
            Some(Kind::Edge) => self.set_tool(Tool::PolygonEdge),
            Some(Kind::Circumscribed) => self.set_polygon_tool(true),
            Some(Kind::Inscribed) => self.set_polygon_tool(false),
            None if close => self.set_tool(Tool::Select),
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #389: each of Fusion's three polygon tools has its own kind in the
    /// dialog, and nothing else opens it.
    #[test]
    fn the_dialog_names_the_running_polygon_tool() {
        assert_eq!(Kind::of(Tool::Polygon, true), Some(Kind::Circumscribed));
        assert_eq!(Kind::of(Tool::Polygon, false), Some(Kind::Inscribed));
        assert_eq!(Kind::of(Tool::PolygonEdge, true), Some(Kind::Edge));
        assert_eq!(Kind::of(Tool::Line, false), None);
        let reg = crate::actions::Registry::default();
        for k in [Kind::Circumscribed, Kind::Inscribed, Kind::Edge] {
            let (id, name) = k.action();
            assert!(reg.get(id).is_some_and(|a| a.label == name), "{id}");
        }
    }
}
