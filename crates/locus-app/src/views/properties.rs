// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Inspector pane (M5 design §2.1, #331): document and current style
//! when nothing is selected; header, layer, construction, geometry, style
//! and constraints for curves; the editor, layer and style for text.

use crate::settings::{Settings, Units};
use crate::state::AppState;
use crate::views::text_ui::{self, TextSettings};
use locus_doc::{Command, Document, EntityId, Geom};
use locus_geom::Vec2;

/// A section heading in the mockup's style (`.sec h4`): small capitals,
/// muted, with an optional control at the right end.
pub(crate) fn section(ui: &mut egui::Ui, title: &str, right: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title.to_uppercase())
                .size(10.5)
                .strong()
                .color(ui.visuals().weak_text_color()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
}

/// A small coloured badge (`.badge`): FREE, CONSTRUCTION, RE-EDITABLE…
fn badge(ui: &mut egui::Ui, text: &str, colour: egui::Color32) {
    egui::Frame::new()
        .fill(colour.gamma_multiply(0.18))
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(10.0).strong().color(colour));
        });
}

/// A layer's colour swatch (`.sw`).
fn swatch(ui: &mut egui::Ui, rgb: [u8; 3]) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(r, 2.0, egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
}

/// The sketch's solve state as a pill (`.pill`), in the status colour:
/// shared by the Inspector and the status bar (SPEC §5 Solver UX).
pub(crate) fn solve_pill(ui: &mut egui::Ui, state: &AppState) {
    let st = state.doc.sketcher.state();
    let v = ui.visuals();
    let (text, colour) = match st.status {
        locus_doc::Status::FullyConstrained => (
            "Fully constrained".to_owned(),
            egui::Color32::from_rgb(47, 138, 76),
        ),
        locus_doc::Status::UnderConstrained => (
            format!("Under-constrained · {} DOF", st.dof),
            v.hyperlink_color,
        ),
        _ => (state.solve_summary(), v.error_fg_color),
    };
    egui::Frame::new()
        .fill(colour.gamma_multiply(0.16))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(8, 1))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(11.0).strong().color(colour));
        });
}

impl AppState {
    /// The Inspector (M5 design §2.1, #331): changes with the selection.
    /// Nothing selected: Document and the current Style. Curves: a header
    /// with type, count and state, Layer, Construction, Geometry, Style and
    /// the constraints on them. Text: its editor, Layer and Style.
    pub fn properties_ui(&mut self, ui: &mut egui::Ui, settings: &mut Settings) {
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
        let regions = !self.selection.regions.selected.is_empty();
        if self.selection.items.is_empty() {
            if !regions {
                self.document_section(ui, settings);
                ui.separator();
                self.style_ui(ui);
            }
            self.tools.text_edit = None;
            return;
        }
        if let Some((id, anchor, dir, text)) = selected_text {
            let shown: String = text.content.chars().take(24).collect();
            section(ui, &format!("Text · \u{201c}{shown}\u{201d}"), |ui| {
                badge(ui, "RE-EDITABLE", ui.visuals().hyperlink_color);
            });
            if self.tools.text_dialog == Some(id) {
                ui.weak("Editing in the Text dialog on the canvas.");
            } else {
                self.edit_text(ui, id, anchor, dir, &text, false);
                if ui
                    .button("Edit in dialog")
                    .on_hover_text("Or double-click the text on the canvas")
                    .clicked()
                {
                    self.open_text_dialog(id, true);
                }
            }
            self.layer_row(ui);
            ui.separator();
            self.style_ui(ui);
            return;
        }
        self.tools.text_edit = None;
        self.selection_header(ui);
        self.layer_row(ui);
        if let Some(cmd) = self.construction_toggle() {
            // Normal/Construction (design §2.1, §6.10; #289): one command per click.
            let doc = self.doc.sketcher.document();
            let flags: Vec<bool> = self
                .selection
                .items
                .iter()
                .filter(|id| doc.driven_by(*id).is_none())
                .filter_map(|id| doc.entity(id).map(|e| e.construction))
                .collect();
            let all = flags.iter().all(|c| *c);
            let mixed = !all && flags.iter().any(|c| *c);
            let mut on = all;
            let toggled = crate::views::canvas::dialog::row(ui, "Construction", |ui| {
                ui.add(egui::Checkbox::new(&mut on, "X").indeterminate(mixed))
                    .on_hover_text("Drawn dashed, takes constraints, never exported (X)")
                    .changed()
            });
            if toggled {
                self.apply_reporting(&cmd);
            }
        }
        ui.separator();
        section(ui, "Geometry", |_| {});
        self.geometry_rows(ui, settings.units);
        if !regions {
            ui.separator();
            self.style_ui(ui);
        }
        ui.separator();
        self.constraints_section(ui, settings.units);
    }

    /// Nothing selected (design §2.1): sketch status, the document's
    /// machine profile, the active layer and the display units.
    fn document_section(&mut self, ui: &mut egui::Ui, settings: &mut Settings) {
        use crate::views::canvas::dialog::{row, segmented};
        section(ui, "Document", |_| {});
        row(ui, "Sketch", |ui| solve_pill(ui, self));
        let doc = self.doc.sketcher.document();
        let profile = doc
            .profile()
            .and_then(|id| locus_export::profile::find(id, &settings.profiles))
            .map_or_else(|| "Chosen at export".to_owned(), |p| p.name);
        row(ui, "Profile", |ui| {
            ui.label(profile)
                .on_hover_text("The machine profile this document exports with (File ▸ Export…)")
        });
        let layer = doc
            .layer(self.doc.active_layer)
            .map(|l| (l.colour, l.name.clone()));
        row(ui, "Active layer", |ui| match layer {
            Some((c, name)) => {
                swatch(ui, c);
                ui.label(name);
            }
            None => {
                ui.weak("—");
            }
        });
        row(ui, "Units", |ui| {
            if let Some(u) = segmented(
                ui,
                settings.units,
                &[(Units::Mm, "mm"), (Units::Inch, "in")],
            ) {
                settings.units = u;
            }
        });
    }

    /// The header for a selection of curves: what and how many, and a
    /// badge for its state (CONSTRUCTION, FULLY CONSTRAINED or FREE).
    fn selection_header(&self, ui: &mut egui::Ui) {
        let doc = self.doc.sketcher.document();
        let ids: Vec<EntityId> = self.selection.items.iter().collect();
        let kinds: std::collections::BTreeSet<&str> = ids
            .iter()
            .filter_map(|id| doc.entity(*id))
            .map(|e| kind_name(&e.geom))
            .collect();
        let what = match (kinds.len(), kinds.first()) {
            (1, Some(k)) => (*k).to_owned(),
            _ => "Selection".to_owned(),
        };
        let n = ids.len();
        let title = if n == 1 {
            what
        } else {
            format!("{what} · {n} items")
        };
        let items: Vec<&crate::scene::Item> = self
            .doc
            .scene
            .items
            .iter()
            .filter(|i| self.selection.items.contains(i.id))
            .collect();
        let v = ui.visuals().clone();
        section(ui, &title, |ui| {
            if !items.is_empty() && items.iter().all(|i| i.construction) {
                badge(ui, "CONSTRUCTION", egui::Color32::from_rgb(210, 122, 28));
            } else if !items.is_empty() && items.iter().all(|i| i.constrained) {
                badge(ui, "FULLY CONSTRAINED", v.weak_text_color());
            } else {
                badge(ui, "FREE", v.hyperlink_color);
            }
        });
    }

    /// The selection's layer, or "—" when it spans several.
    fn layer_row(&self, ui: &mut egui::Ui) {
        let doc = self.doc.sketcher.document();
        let layers: std::collections::BTreeSet<_> = self
            .selection
            .items
            .iter()
            .filter_map(|id| doc.entity(id).map(|e| e.layer))
            .collect();
        let one = (layers.len() == 1)
            .then(|| layers.first().and_then(|l| doc.layer(*l)))
            .flatten();
        crate::views::canvas::dialog::row(ui, "Layer", |ui| match one {
            Some(l) => {
                swatch(ui, l.colour);
                ui.label(&l.name);
            }
            None => {
                ui.weak("—")
                    .on_hover_text("The selection spans several layers");
            }
        });
    }

    /// Geometry rows: one item's measurements, or the bounds of many.
    fn geometry_rows(&self, ui: &mut egui::Ui, units: Units) {
        use crate::views::canvas::dialog::row;
        let doc = self.doc.sketcher.document();
        let ids: Vec<EntityId> = self.selection.items.iter().collect();
        if let [id] = ids[..] {
            for (k, v) in describe(doc, id, units)
                .into_iter()
                .filter(|(k, _)| !matches!(k.as_str(), "ID" | "Type" | "Layer" | "Construction"))
            {
                row(ui, &k, |ui| ui.monospace(v));
            }
            ui.weak("Add a driving value with Dimension (D).");
            return;
        }
        let bbox = self
            .doc
            .scene
            .items
            .iter()
            .filter(|i| self.selection.items.contains(i.id))
            .map(|i| i.bbox)
            .reduce(locus_geom::Aabb::union);
        if let Some(b) = bbox {
            row(ui, "Bounds", |ui| {
                ui.monospace(format!(
                    "{} × {}",
                    units.format_len(b.width()),
                    units.format_len(b.height())
                ))
            });
        }
        let mut counts = [0usize; 6];
        for id in &ids {
            if let Some(e) = doc.entity(*id) {
                counts[e.geom.kind() as usize] += 1;
            }
        }
        let parts: Vec<String> = ["points", "lines", "circles", "arcs", "Béziers", "texts"]
            .iter()
            .zip(counts)
            .filter(|(_, c)| *c > 0)
            .map(|(name, c)| format!("{c} {name}"))
            .collect();
        row(ui, "Contains", |ui| ui.label(parts.join(", ")));
    }

    /// The constraints touching the selection (design §2.1), by name.
    fn constraints_section(&self, ui: &mut egui::Ui, units: Units) {
        let doc = self.doc.sketcher.document();
        let names: Vec<String> = doc
            .constraints()
            .values()
            .filter(|c| {
                c.references()
                    .iter()
                    .any(|r| self.selection.items.contains(*r))
            })
            .filter_map(|c| match (crate::scene::glyph_symbol(c), c.dimension()) {
                (Some(s), _) => Some(s.name.to_owned()),
                (None, Some(d)) => Some(format!("Dimension {}", units.format_len(d.value))),
                (None, None) => None,
            })
            .collect();
        let n = names.len();
        section(ui, "Constraints", |ui| {
            ui.weak(n.to_string());
        });
        if names.is_empty() {
            ui.weak("None. Add them from CONSTRAIN or the marking menu.");
        } else {
            ui.label(egui::RichText::new(names.join(" · ")).size(11.5));
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
        focus: bool,
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
        let mut edited = text_ui::controls(ui, &mut s, &mut self.tools.fonts, !missing, 3, focus);
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

/// An entity kind's name, as the Inspector header shows it.
fn kind_name(g: &Geom) -> &'static str {
    match g {
        Geom::Point { .. } => "Point",
        Geom::Line { .. } => "Line",
        Geom::Circle { .. } => "Circle",
        Geom::Arc { .. } => "Arc",
        Geom::Bezier { .. } => "Bézier",
        Geom::Text { .. } => "Text",
    }
}

/// Label/value rows for the properties panel.
pub(crate) fn describe(doc: &Document, id: EntityId, units: Units) -> Vec<(String, String)> {
    let Some(e) = doc.entity(id) else {
        return Vec::new();
    };
    let pt = |p: Vec2| format!("{}, {}", units.format_len(p.x), units.format_len(p.y));
    let kind = kind_name(&e.geom);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The Inspector draws each context of design §2.1 without panicking:
    /// nothing selected (Document + current style), one curve, many
    /// curves; and the Document section's units switch is the setting.
    #[test]
    fn inspector_contexts_render() {
        let ctx = egui::Context::default();
        let mut state = AppState::default();
        let mut settings = Settings::default();
        let frame = |state: &mut AppState, settings: &mut Settings| {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                state.properties_ui(ui, settings);
            });
        };
        frame(&mut state, &mut settings);
        let ids: Vec<EntityId> = state.doc.scene.items.iter().map(|i| i.id).collect();
        assert!(ids.len() > 2, "the demo has geometry");
        state.selection.items.set_all(ids.iter().copied().take(1));
        frame(&mut state, &mut settings);
        state.selection.items.set_all(ids.iter().copied());
        frame(&mut state, &mut settings);
        assert_eq!(settings.units, Units::Mm);
    }

    #[test]
    fn kind_names_read_as_in_the_header() {
        let doc = crate::demo::sketch();
        let names: std::collections::BTreeSet<&str> = doc
            .entities()
            .iter()
            .map(|(_, e)| kind_name(&e.geom))
            .collect();
        assert!(names.contains("Line") && names.contains("Circle"));
    }
}
