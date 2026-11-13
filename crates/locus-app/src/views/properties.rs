// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Properties pane: what is selected, and the selected text's editor.

use crate::settings::{Settings, Units};
use crate::state::AppState;
use crate::views::text_ui::{self, TextSettings};
use locus_doc::{Command, Document, EntityId, Geom};
use locus_geom::Vec2;

impl AppState {
    pub fn properties_ui(&mut self, ui: &mut egui::Ui, settings: &Settings) {
        if let Some(s) = self.region_summary() {
            ui.label(s);
        }
        let selected_text = self
            .selection
            .items
            .iter()
            .next()
            .filter(|_| self.selection.items.len() == 1)
            .and_then(|id| {
                let (t, anchor, dir) = self.doc.sketcher.document().text(id)?;
                Some((id, anchor, dir, t.clone()))
            });
        let doc = self.doc.sketcher.document();
        match self.selection.items.len() {
            0 if !self.selection.regions.selected.is_empty() => {}
            0 => {
                ui.weak("Nothing selected.");
            }
            1 => {
                if let Some(id) = self.selection.items.iter().next() {
                    let rows = describe(doc, id, settings.units);
                    egui::Grid::new("props").num_columns(2).show(ui, |ui| {
                        for (k, v) in rows {
                            ui.label(k);
                            ui.monospace(v);
                            ui.end_row();
                        }
                    });
                }
            }
            n => {
                ui.label(format!("{n} entities selected"));
                let mut counts = [0usize; 6];
                for id in self.selection.items.iter() {
                    if let Some(e) = doc.entity(id) {
                        counts[e.geom.kind() as usize] += 1;
                    }
                }
                for (name, c) in ["Points", "Lines", "Circles", "Arcs", "Béziers", "Texts"]
                    .iter()
                    .zip(counts)
                {
                    if c > 0 {
                        ui.label(format!("{name}: {c}"));
                    }
                }
            }
        }
        if let Some((id, anchor, dir, text)) = selected_text {
            ui.separator();
            self.edit_text(ui, id, anchor, dir, &text);
        } else {
            self.tools.text_edit = None;
        }
    }

    /// Edit the selected text's content, font and style. Style changes apply
    /// at once; the content applies when its field loses focus, so one edit
    /// is one undo step (SPEC §11). With the font missing only the font can
    /// change (the content can't be re-laid out), and picking one re-lays
    /// the saved text out in it.
    pub(crate) fn edit_text(
        &mut self,
        ui: &mut egui::Ui,
        id: EntityId,
        anchor: EntityId,
        dir: EntityId,
        stored: &locus_doc::TextData,
    ) {
        let mut s = match self.tools.text_edit.take() {
            Some((i, s)) if i == id => s,
            _ => TextSettings::from_data(stored),
        };
        // An undo/redo or solver change under the buffer: show the stored text
        // unless the user is mid-edit of the content.
        let live = TextSettings::from_data(stored);
        if s.content != live.content && !ui.memory(|m| m.focused().is_some()) {
            s = live;
        }
        let missing = self.tools.fonts.missing(&stored.font);
        let mut edited = text_ui::controls(ui, &mut s, &mut self.tools.fonts, !missing, 3);
        if s.content != stored.content
            && !missing
            && ui
                .button("Apply text")
                .on_hover_text("Ctrl+Enter, or click away from the field")
                .clicked()
        {
            edited.content_done = true;
        }
        if edited.commit() && s != TextSettings::from_data(stored) {
            match s.to_data(&self.tools.fonts.library) {
                Ok(text) => {
                    let cmd = Command::ModifyEntity {
                        id,
                        geom: Geom::Text {
                            anchor,
                            dir,
                            text: Box::new(text),
                        },
                    };
                    match self.apply(&cmd) {
                        Ok(()) => self.doc.notice = None,
                        Err(e) => self.doc.notice = Some(e.to_string()),
                    }
                }
                Err(e) => self.doc.notice = Some(e.to_string()),
            }
        }
        self.tools.text_edit = Some((id, s));
    }
}

/// Label/value rows for the properties panel.
pub(crate) fn describe(doc: &Document, id: EntityId, units: Units) -> Vec<(String, String)> {
    let Some(e) = doc.entity(id) else {
        return Vec::new();
    };
    let pt = |p: Vec2| format!("{}, {}", units.format_len(p.x), units.format_len(p.y));
    let kind = match e.geom {
        Geom::Point { .. } => "Point",
        Geom::Line { .. } => "Line",
        Geom::Circle { .. } => "Circle",
        Geom::Arc { .. } => "Arc",
        Geom::Bezier { .. } => "Bézier",
        Geom::Text { .. } => "Text",
    };
    let mut rows = vec![
        ("ID".to_owned(), id.to_string()),
        ("Type".to_owned(), kind.to_owned()),
    ];
    if let Some(l) = doc.layer(e.layer) {
        rows.push(("Layer".to_owned(), l.name.clone()));
    }
    rows.push((
        "Construction".to_owned(),
        if e.construction { "yes" } else { "no" }.to_owned(),
    ));
    match e.geom {
        Geom::Point { pos } => rows.push(("Position".to_owned(), pt(pos))),
        Geom::Line { .. } => {
            if let Some(l) = doc.line(id) {
                rows.push(("Start".to_owned(), pt(l.a)));
                rows.push(("End".to_owned(), pt(l.b)));
                rows.push(("Length".to_owned(), units.format_len(l.length())));
                let ang = (l.b - l.a).angle().to_degrees();
                rows.push(("Angle".to_owned(), format!("{ang:.2}°")));
            }
        }
        Geom::Circle { .. } => {
            if let Some(c) = doc.circle(id) {
                rows.push(("Centre".to_owned(), pt(c.center)));
                rows.push(("Radius".to_owned(), units.format_len(c.radius)));
                rows.push(("Diameter".to_owned(), units.format_len(c.radius * 2.0)));
            }
        }
        Geom::Arc { .. } => {
            if let Some(a) = doc.arc(id) {
                rows.push(("Centre".to_owned(), pt(a.center)));
                rows.push(("Radius".to_owned(), units.format_len(a.radius())));
                rows.push((
                    "Sweep".to_owned(),
                    format!("{:.2}°", a.sweep().to_degrees()),
                ));
            }
        }
        Geom::Bezier { .. } => {
            if let Some(c) = doc.cubic(id) {
                rows.push(("Start".to_owned(), pt(c.p0)));
                rows.push(("End".to_owned(), pt(c.p1)));
                rows.push(("Length".to_owned(), units.format_len(c.length())));
            }
        }
        Geom::Text { .. } => {
            if let Some((t, anchor, _)) = doc.text(id) {
                rows.push(("Text".to_owned(), t.content.clone()));
                rows.push(("Font".to_owned(), t.font.display_name()));
                rows.push(("Size".to_owned(), units.format_len(t.size)));
                if let Some(p) = doc.point_pos(anchor) {
                    rows.push(("Anchor".to_owned(), pt(p)));
                }
            }
        }
    }
    rows
}
