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
            if let Some(layer) = self.view.browser_layer.filter(|_| !regions) {
                // A layer picked in the Browser (design §2.1, #326).
                self.layer_section(ui, layer);
            } else if !regions {
                self.document_section(ui, settings);
                ui.separator();
                self.style_ui(ui);
            } else {
                // Regions alone (#370): their fill.
                ui.separator();
                self.region_style_ui(ui, settings.display.fills);
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
            self.text_path_rows(ui, id, &text);
            self.layer_row(ui);
            // Text switches between normal and construction too (#363).
            self.construction_row(ui);
            ui.separator();
            self.style_ui(ui);
            return;
        }
        self.tools.text_edit = None;
        self.selection_header(ui);
        self.layer_row(ui);
        self.construction_row(ui);
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

    /// The Construction checkbox (X): one command per click (design §2.1,
    /// §6.10; #289).
    fn construction_row(&mut self, ui: &mut egui::Ui) {
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
                ui.add(crate::theme::Checkbox::new(&mut on, "X").mixed(mixed))
                    .on_hover_text("Drawn dashed, takes constraints, never exported (X)")
                    .changed()
            });
            if toggled {
                self.apply_reporting(&cmd);
            }
        }
    }

    /// Text on a path (#392): its side, direction and offset, and Detach.
    /// Each change is one command.
    fn text_path_rows(&mut self, ui: &mut egui::Ui, id: EntityId, text: &locus_doc::TextData) {
        use crate::views::canvas::dialog::{row, segmented};
        use locus_doc::path_text::PathSide;
        let Some(p) = text.path.as_deref() else {
            return;
        };
        let mut next = (p.side, p.flip, p.offset);
        row(ui, "On path", |ui| {
            ui.label(crate::views::canvas::dialog::count(
                p.curves.len(),
                "curve",
                "curves",
            ))
        });
        row(ui, "Side", |ui| {
            if let Some(s) = segmented(
                ui,
                p.side,
                &[(PathSide::Above, "Above"), (PathSide::Below, "Below")],
            ) {
                next.0 = s;
            }
        });
        row(ui, "", |ui| {
            crate::theme::checkbox(ui, &mut next.1, "Flip direction")
        });
        let mut offset = p.offset;
        let r = row(ui, "Offset", |ui| {
            ui.add(egui::DragValue::new(&mut offset).speed(0.1).suffix(" mm"))
                .on_hover_text("How far along the path; or drag the text on the canvas")
        });
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            next.2 = offset;
        }
        if next != (p.side, p.flip, p.offset) {
            let (side, flip, offset) = next;
            match self.text_path_command(id, "Change text on path", |t| {
                if let Some(p) = t.path.as_mut() {
                    p.side = side;
                    p.flip = flip;
                    p.offset = offset;
                }
            }) {
                Ok(cmd) => {
                    self.apply_reporting(&cmd);
                }
                Err(why) => self.doc.notice = Some(why),
            }
        }
        if ui
            .button("Detach from path")
            .on_hover_text("Back to straight text at its anchor")
            .clicked()
        {
            self.detach_text(id);
        }
    }

    /// A layer picked in the Browser (design §2.1): name, operation,
    /// export and order. Export is one undoable command (SPEC §11); the
    /// operation's parameters are edited from the layer's row menu.
    fn layer_section(&mut self, ui: &mut egui::Ui, id: locus_doc::LayerId) {
        use crate::views::canvas::dialog::row;
        let Some(l) = self.doc.sketcher.document().layer(id).cloned() else {
            // Deleted or undone away.
            self.view.browser_layer = None;
            return;
        };
        section(ui, "Layer", |_| {});
        row(ui, "Name", |ui| {
            swatch(ui, l.colour);
            ui.label(&l.name);
        });
        row(ui, "Operation", |ui| ui.label(l.operation.kind().name()));
        let mut export = l.export_enabled;
        let toggled = row(ui, "Export", |ui| {
            crate::theme::checkbox(ui, &mut export, "Include in export").changed()
        });
        if toggled {
            self.apply_reporting(&Command::SetLayerProps {
                id,
                props: locus_doc::LayerProps {
                    export_enabled: Some(export),
                    ..locus_doc::LayerProps::default()
                },
            });
        }
        row(ui, "Order", |ui| {
            ui.monospace((l.export_order + 1).to_string())
        });
        let state = match (l.visible, l.locked) {
            (true, false) => "Visible",
            (true, true) => "Visible, locked",
            (false, false) => "Hidden",
            (false, true) => "Hidden, locked",
        };
        row(ui, "State", |ui| ui.label(state));
        // A reference's scale from a known distance (#391), high in the
        // section so a short Inspector doesn't clip it.
        if matches!(l.operation, locus_doc::Operation::Reference)
            && ui
                .button("Calibrate…")
                .on_hover_text(
                    "Click two points on this reference and type the real distance between them",
                )
                .clicked()
        {
            self.set_modify(crate::modify_tool::Modify::Calibrate);
            self.view.browser_layer = Some(id);
            self.tools.calibrate.begin(Some(id));
        }
        ui.weak(
            "Operation settings: right-click the layer in the Browser, then Operation settings…",
        );
        self.image_section(ui, id);
    }

    /// A reference image's opacity and size (#390). An edit is held while
    /// the slider is dragged or the field typed in, then applied as one
    /// command (SPEC §11), as the style fields are.
    fn image_section(&mut self, ui: &mut egui::Ui, id: locus_doc::LayerId) {
        use crate::views::canvas::dialog::row;
        use locus_doc::ImagePlacement;
        let Some(img) = self.doc.sketcher.document().image(id).cloned() else {
            return;
        };
        let key = egui::Id::new(("image-placement", id.get()));
        let pending: Option<ImagePlacement> = ui.data(|d| d.get_temp(key));
        let mut p = pending.unwrap_or(img.placement);
        section(ui, "Image", |_| {});
        row(ui, "Pixels", |ui| {
            ui.label(format!("{} × {} px", img.width_px, img.height_px))
        });
        let mut opacity = p.opacity * 100.0;
        let moved = row(ui, "Opacity", |ui| {
            ui.add(egui::Slider::new(&mut opacity, 0.0..=100.0).suffix(" %"))
                .changed()
        });
        if moved {
            p.opacity = (opacity / 100.0).clamp(0.0, 1.0);
        }
        let px_w = f64::from(img.width_px);
        let mut width = px_w * p.mm_per_px;
        let sized = row(ui, "Width", |ui| {
            let r = ui
                .add(
                    egui::DragValue::new(&mut width)
                        .range(0.001..=1.0e6)
                        .speed(0.1)
                        .suffix(" mm"),
                )
                .on_hover_text("The image's width; its height follows");
            ui.weak(format!(
                "× {:.3} mm",
                f64::from(img.height_px) * width / px_w
            ));
            r.changed()
        });
        if sized && width.is_finite() && width > 0.0 {
            let s = (width / px_w).clamp(
                locus_doc::image::MIN_MM_PER_PX,
                locus_doc::image::MAX_MM_PER_PX,
            );
            p.mm_per_px = s;
        }
        ui.weak("Type the width, or Calibrate from a known distance.");
        let settled = !ui.input(|i| i.pointer.any_down()) && ui.memory(|m| m.focused().is_none());
        if p == img.placement {
            ui.data_mut(|d| d.remove::<ImagePlacement>(key));
        } else if settled {
            ui.data_mut(|d| d.remove::<ImagePlacement>(key));
            self.apply_reporting(&Command::SetImagePlacement {
                layer: id,
                placement: p,
            });
        } else {
            ui.data_mut(|d| d.insert_temp(key, p));
        }
    }

    /// Nothing selected (design §2.1): sketch status, the document's
    /// machine profile, the active layer and the display units.
    fn document_section(&mut self, ui: &mut egui::Ui, settings: &mut Settings) {
        use crate::views::canvas::dialog::{row, segmented};
        section(ui, "Document", |_| {});
        row(ui, "Sketch", |ui| solve_pill(ui, self));
        let profile = crate::views::machine_profile::current(self.document(), settings);
        let text = profile
            .as_ref()
            .map_or_else(|| "None".to_owned(), |p| p.name.clone());
        let tip = profile.as_ref().map_or_else(
            || "Choose the machine this document is for (saved with it)".to_owned(),
            crate::views::machine_profile::summary,
        );
        let manage = row(ui, "Machine", |ui| {
            crate::views::machine_profile::picker_button(
                ui,
                self,
                settings,
                egui::RichText::new(text),
                &tip,
            )
        });
        if manage {
            self.view.shell.push(crate::actions::Shell::Profiles);
        }
        let doc = self.doc.sketcher.document();
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
            self.doc.notice = self.commit_text_edit(id, anchor, dir, stored, &s).err();
        }
        self.tools.text_edit = Some((id, s));
    }

    /// Apply the Text dialog's / Inspector's settings `s` to the stored text
    /// as one undo step. The stored text is edited, not rebuilt, so text on
    /// a path keeps its binding (#415).
    pub(crate) fn commit_text_edit(
        &mut self,
        id: EntityId,
        anchor: EntityId,
        dir: EntityId,
        stored: &locus_doc::TextData,
        s: &TextSettings,
    ) -> Result<(), String> {
        let text = s
            .apply_to(stored, &self.tools.fonts.library)
            .map_err(|e| e.to_string())?;
        let cmd = self.text_edit_command(id, anchor, dir, stored, text);
        self.apply(&cmd).map_err(|e| e.to_string())
    }
}

impl AppState {
    /// The command for an edited text. A new alignment keeps the text
    /// block where it is and lines up its lines inside it (#362, Fusion):
    /// the anchor is moved by how far the block's left edge would jump, in
    /// one undo step, instead of the text swinging round the anchor.
    pub(crate) fn text_edit_command(
        &self,
        id: EntityId,
        anchor: EntityId,
        dir: EntityId,
        stored: &locus_doc::TextData,
        text: locus_doc::TextData,
    ) -> Command {
        use locus_text::Align;
        let left = |a: Align, w: f64| match a {
            Align::Left => 0.0,
            Align::Center => -w / 2.0,
            Align::Right => -w,
        };
        let shift = left(stored.align, stored.layout.width) - left(text.align, text.layout.width);
        // On a path the alignment places the text along the path, not
        // about its anchor (#392), so the anchor stays put.
        let shift = if text.path.is_some() { 0.0 } else { shift };
        let modify = Command::ModifyEntity {
            id,
            geom: Geom::Text {
                anchor,
                dir,
                text: Box::new(text),
            },
        };
        let doc = self.doc.sketcher.document();
        let along = doc
            .point_pos(anchor)
            .zip(doc.point_pos(dir))
            .and_then(|(a, d)| (d - a).normalized(1e-12));
        let Some(u) = along.filter(|_| shift.abs() > 1e-9) else {
            return modify;
        };
        match locus_doc::transform::transform(
            doc,
            &[id],
            locus_doc::transform::Transform::Move(u * shift),
            false,
        ) {
            Ok(plan) => Command::Compound {
                label: "Align text".to_owned(),
                commands: vec![modify, plan.command],
            },
            // The anchor can't move (it is held by constraints): align
            // about it instead.
            Err(_) => modify,
        }
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

    /// #391: a Reference layer's Calibrate… button in the Inspector starts
    /// the tool on that layer, clicked in real egui frames.
    #[test]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    fn the_calibrate_button_starts_the_tool_on_the_layer() {
        use egui::{Event, PointerButton, RawInput, accesskit::Role, pos2, vec2};
        let mut state = AppState::default();
        state.load(locus_doc::Document::new());
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 10 10"><line x1="0" y1="5" x2="10" y2="5" stroke="black"/></svg>"#;
        let (cmd, _) = locus_import::reference_command(state.document(), "plan.svg", svg).unwrap();
        state.apply(&cmd).unwrap();
        let layer = *state
            .document()
            .layers()
            .iter()
            .find(|(_, l)| matches!(l.operation, locus_doc::Operation::Reference))
            .unwrap()
            .0;
        state.view.browser_layer = Some(layer);
        let mut settings = Settings::default();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut t = 0.0;
        let mut frame = |state: &mut AppState, events: Vec<Event>| {
            let input = RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    pos2(0.0, 0.0),
                    vec2(400.0, 900.0),
                )),
                time: Some(t),
                events,
                ..RawInput::default()
            };
            t += 0.05;
            ctx.run_ui(input, |ui| state.properties_ui(ui, &mut settings))
        };
        let _ = frame(&mut state, Vec::new());
        let out = frame(&mut state, Vec::new());
        let tree = out
            .platform_output
            .accesskit_update
            .expect("accesskit tree");
        let at = tree
            .nodes
            .iter()
            .find(|(_, n)| {
                n.role() == Role::Button && n.label().is_some_and(|l| l.contains("Calibrate"))
            })
            .and_then(|(_, n)| n.bounds())
            .map(|b| pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32))
            .expect("a Calibrate button");
        let btn = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = frame(&mut state, vec![Event::PointerMoved(at)]);
        let _ = frame(&mut state, vec![btn(true)]);
        let _ = frame(&mut state, vec![btn(false)]);
        assert_eq!(state.modify(), Some(crate::modify_tool::Modify::Calibrate));
        assert_eq!(state.tools.calibrate.layer, Some(layer));
    }

    /// #370, #451: selecting a region shows its own Fill; setting it fills
    /// the region and no object.
    #[test]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    fn a_selected_region_shows_and_takes_its_fill() {
        use egui::accesskit::Role;
        use locus_doc::{Command, Fill};
        let mut state = AppState::default();
        let mut settings = Settings::default();
        let key = state
            .regions()
            .first()
            .expect("the demo has regions")
            .key
            .clone();
        state.selection.regions.selected.insert(key.clone());
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut frame = || {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                state.properties_ui(ui, &mut settings);
            })
        };
        let _ = frame();
        let out = frame();
        let tree = out
            .platform_output
            .accesskit_update
            .expect("accesskit tree");
        let has = |role: Role, text: &str| {
            tree.nodes.iter().any(|(_, n)| {
                let said = |t: Option<&str>| t.is_some_and(|t| t.to_lowercase().contains(text));
                n.role() == role && (text.is_empty() || said(n.label()) || said(n.value()))
            })
        };
        assert!(has(Role::Label, "fill"), "a Fill section");
        assert!(has(Role::CheckBox, ""), "the fill on/off box");
        // The region has its own fill (#451): set it, and it is filled.
        let region = state.region(&key).unwrap().clone();
        assert!(state.region_fill(&region).is_none());
        assert!(state.apply_reporting(&Command::SetRegionFill {
            regions: vec![key.clone()],
            fill: Some(locus_doc::region_fill::RegionFill {
                fill: Fill::Colour([200, 40, 40]),
                opacity: 0.6,
            }),
        }));
        let region = state.region(&key).unwrap().clone();
        let c = state.region_fill(&region).expect("filled");
        assert_eq!(c, egui::Color32::from_rgba_unmultiplied(200, 40, 40, 153));
        assert_eq!(state.undo_label(), Some("Fill region"));
        // The panel still draws with the fill set, and no object got one.
        frame_again(&mut state, &mut settings, &ctx);
        assert!(
            state
                .document()
                .entities()
                .iter()
                .all(|(_, e)| e.style.fill == Fill::None)
        );
    }

    fn frame_again(state: &mut AppState, settings: &mut Settings, ctx: &egui::Context) {
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            state.properties_ui(ui, settings);
        });
    }

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
        // A layer picked in the Browser is the subject when nothing is selected.
        state.clear_selection();
        state.view.browser_layer = Some(state.doc.active_layer);
        frame(&mut state, &mut settings);
        assert!(state.view.browser_layer.is_some());
        assert_eq!(settings.units, Units::Mm);
    }

    /// #362: changing alignment keeps the text block in place (its left
    /// edge doesn't jump), and Right ends the lines at the block's right.
    #[test]
    #[allow(clippy::expect_used)]
    fn changing_alignment_keeps_the_text_in_place() {
        use crate::tool::ToolEvent;
        use crate::tools::{Pick, Tool};
        use locus_text::Align;
        let mut c = AppState::default();
        c.load(locus_doc::Document::new());
        c.set_tool(Tool::Text);
        c.tools.text_settings.content = "ab
abcd"
            .to_owned();
        c.tool_event(
            ToolEvent::Click(Pick::free(Vec2::new(10.0, 5.0))),
            None,
            Units::Mm,
        );
        c.close_text_dialog();
        let doc = c.doc.sketcher.document();
        let id = doc
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Text { .. }))
            .map(|(id, _)| id)
            .expect("a text");
        let before = doc.text_geometry(id).and_then(|g| g.bbox()).expect("bbox");
        let (stored, anchor, dir) = doc
            .text(id)
            .map(|(t, a, d)| (t.clone(), a, d))
            .expect("text");
        let right = stored
            .edited(&locus_text::BundledProvider, |t| t.align = Align::Right)
            .expect("relaid out");
        let cmd = c.text_edit_command(id, anchor, dir, &stored, right);
        assert!(c.apply_reporting(&cmd));
        let after = c
            .doc
            .sketcher
            .document()
            .text_geometry(id)
            .and_then(|g| g.bbox())
            .expect("bbox");
        assert!(
            (after.min.x - before.min.x).abs() < 0.05,
            "{before:?} -> {after:?}"
        );
        // The right edge moves only by the last glyph's side bearing.
        assert!(
            (after.max.x - before.max.x).abs() < 0.5,
            "{before:?} -> {after:?}"
        );
        assert_eq!(c.undo_label(), Some("Align text"));
    }

    /// #363: text switches between normal and construction both ways,
    /// through the Normal/Construction action (X) and the Inspector row.
    #[test]
    #[allow(clippy::expect_used)]
    fn text_toggles_between_normal_and_construction() {
        use crate::tool::ToolEvent;
        use crate::tools::{Pick, Tool};
        for start_construction in [false, true] {
            let mut c = AppState::default();
            c.load(locus_doc::Document::new());
            c.doc.construction_linetype = start_construction;
            c.set_tool(Tool::Text);
            c.tools.text_settings.content = "Hi".to_owned();
            c.tool_event(ToolEvent::Click(Pick::free(Vec2::ZERO)), None, Units::Mm);
            c.close_text_dialog();
            let id = c
                .doc
                .sketcher
                .document()
                .entities()
                .iter()
                .find(|(_, e)| matches!(e.geom, Geom::Text { .. }))
                .map(|(id, _)| id)
                .expect("a text");
            let is_cons =
                |c: &AppState| c.doc.sketcher.document().entity(id).map(|e| e.construction);
            assert_eq!(is_cons(&c), Some(start_construction));
            c.selection.items.set_all([id]);
            let reg = crate::actions::Registry::default();
            let mut settings = Settings::default();
            let mut cx = crate::actions::Cx {
                state: &mut c,
                settings: &mut settings,
                plugins: None,
            };
            reg.run(reg.get("modify.construction").expect("X"), &mut cx);
            assert_eq!(is_cons(&c), Some(!start_construction));
            // The Inspector shows the row for a selected text.
            let ctx = egui::Context::default();
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                c.properties_ui(ui, &mut settings);
            });
            assert!(c.construction_toggle().is_some());
        }
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
