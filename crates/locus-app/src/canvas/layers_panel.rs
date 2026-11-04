// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Layers panel (#118; SPEC §4 Layers, §7 Operations).
//!
//! Every action is a `Sketcher` command, so each is one undo step. Slider and
//! colour edits are held in a local copy while the pointer is down and
//! committed once, on release, so a drag is not a hundred undo steps.

use super::Canvas;
use crate::constrain;
use crate::settings::Settings;
use egui::{DragValue, Ui};
use locus_doc::{
    Command, Compensation, ConstraintId, Document, EntityId, LayerDeletion, LayerId, LayerProps,
    LockReport, Operation, OperationKind,
};

/// Colours handed to new layers in turn.
const PALETTE: [[u8; 3]; 6] = [
    [230, 57, 70],
    [29, 128, 190],
    [42, 157, 80],
    [244, 162, 28],
    [138, 79, 190],
    [0, 150, 160],
];

/// A confirmation waiting for the user.
#[derive(Debug)]
pub(super) enum Dialog {
    Lock { layer: LayerId, report: LockReport },
    Delete { layer: LayerId, plan: LayerDeletion },
}

/// Transient panel state (never part of the document, SPEC §11).
#[derive(Debug, Default)]
pub(super) struct LayerUi {
    expanded: Option<LayerId>,
    rename: Option<(LayerId, String)>,
    colour: Option<(LayerId, [u8; 3])>,
    op: Option<(LayerId, Operation)>,
    dialog: Option<Dialog>,
}

/// What the user asked for this frame; applied after the borrows end.
#[derive(Default)]
struct Wants {
    commands: Vec<Command>,
    activate: Option<LayerId>,
    add: bool,
    lock: Option<LayerId>,
    delete: Option<LayerId>,
}

impl Canvas {
    /// The layer new geometry goes on. Falls back to the first layer in
    /// export order when the remembered one is gone.
    pub(super) fn active_layer(&self) -> LayerId {
        let doc = self.sketcher.document();
        if doc.layer(self.active_layer).is_some() {
            self.active_layer
        } else {
            sorted(doc)
                .first()
                .copied()
                .unwrap_or_else(Document::default_layer)
        }
    }

    pub fn layers_ui(&mut self, ui: &mut Ui, settings: &Settings) {
        self.active_layer = self.active_layer();
        let mut want = Wants::default();
        let selected: Vec<EntityId> = self.selection.iter().collect();
        {
            let doc = self.sketcher.document();
            let st = &mut self.layer_ui;
            let active = self.active_layer;
            ui.horizontal(|ui| {
                if ui.button("+ Add layer").clicked() {
                    want.add = true;
                }
                let name = doc.layer(active).map_or("", |l| l.name.as_str());
                let b = ui.add_enabled(
                    !selected.is_empty(),
                    egui::Button::new(format!("Move {} to “{name}”", selected.len())),
                );
                if b.on_hover_text("Move the selection to the active layer")
                    .clicked()
                {
                    want.commands.push(Command::MoveToLayer {
                        entities: selected.clone(),
                        layer: active,
                    });
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                let order = sorted(doc);
                for (pos, id) in order.iter().enumerate() {
                    layer_row(ui, doc, st, &mut want, &order, pos, *id, active);
                    ui.separator();
                }
            });
        }
        self.apply_wants(want, ui, settings);
    }

    fn apply_wants(&mut self, want: Wants, ui: &Ui, settings: &Settings) {
        if let Some(id) = want.activate {
            self.active_layer = id;
            self.draft.set_layer(id);
        }
        if want.add {
            let doc = self.sketcher.document();
            let n = doc.layers().len();
            let cmd = Command::AddLayer {
                name: unique_name(doc),
                colour: PALETTE[n % PALETTE.len()],
                operation: OperationKind::Cut.default_operation(),
            };
            self.apply_reporting(&cmd);
            if self.notice.is_none()
                && let Some(&newest) = self.sketcher.document().layers().keys().next_back()
            {
                self.active_layer = newest;
            }
        }
        for cmd in &want.commands {
            self.apply_reporting(cmd);
        }
        if let Some(id) = want.lock {
            match self.sketcher.check_lock(id) {
                Ok(r) if r.conflicts.is_empty() && r.redundant.is_empty() => {
                    self.apply_reporting(&Command::SetLayerLocked { id, locked: true });
                }
                Ok(report) => {
                    self.layer_ui.dialog = Some(Dialog::Lock { layer: id, report });
                }
                Err(e) => self.notice = Some(e.to_string()),
            }
        }
        if let Some(id) = want.delete {
            match self.sketcher.document().layer_deletion(id) {
                Ok(plan) => self.layer_ui.dialog = Some(Dialog::Delete { layer: id, plan }),
                Err(e) => self.notice = Some(e.to_string()),
            }
        }
        self.layer_dialog(ui.ctx(), settings);
    }

    /// The lock / delete confirmation (SPEC §4 rules 3 and 4).
    fn layer_dialog(&mut self, ctx: &egui::Context, settings: &Settings) {
        let Some(dialog) = self.layer_ui.dialog.take() else {
            return;
        };
        let doc = self.sketcher.document();
        let mut keep = true;
        let mut run: Option<Command> = None;
        let (title, layer) = match &dialog {
            Dialog::Lock { layer, .. } => ("Lock layer", *layer),
            Dialog::Delete { layer, .. } => ("Delete layer", *layer),
        };
        let name = doc.layer(layer).map_or("?", |l| l.name.as_str()).to_owned();
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| match &dialog {
                Dialog::Lock { report, .. } => {
                    ui.label(format!(
                        "Locking “{name}” fixes its geometry in place for the solver."
                    ));
                    if !report.conflicts.is_empty() {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            "These constraints would conflict, so the layer can't be locked:",
                        );
                        for g in &report.conflicts {
                            list_constraints(ui, doc, &g.constraints, settings);
                        }
                    }
                    if !report.redundant.is_empty() {
                        ui.label("These constraints become redundant (over-constrained):");
                        for g in &report.redundant {
                            list_constraints(ui, doc, &g.constraints, settings);
                        }
                    }
                    ui.horizontal(|ui| {
                        let ok = report.conflicts.is_empty();
                        if ui
                            .add_enabled(ok, egui::Button::new("Lock anyway"))
                            .clicked()
                        {
                            run = Some(Command::SetLayerLocked {
                                id: layer,
                                locked: true,
                            });
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Delete { plan, .. } => {
                    ui.label(format!(
                        "Delete “{name}” and its {} entit{}?",
                        plan.entities.len(),
                        if plan.entities.len() == 1 { "y" } else { "ies" }
                    ));
                    if plan.constraints.is_empty() {
                        ui.label("No constraints are affected.");
                    } else {
                        ui.label(format!(
                            "{} constraint(s) will be dropped:",
                            plan.constraints.len()
                        ));
                        list_constraints(ui, doc, &plan.constraints, settings);
                        if !plan.touched_elsewhere.is_empty() {
                            ui.label(format!(
                                "They touch {} entit{} on other layers: {}",
                                plan.touched_elsewhere.len(),
                                if plan.touched_elsewhere.len() == 1 {
                                    "y"
                                } else {
                                    "ies"
                                },
                                ids(&plan.touched_elsewhere)
                            ));
                        }
                    }
                    if !plan.blocked_by.is_empty() {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            format!(
                                "Entities on other layers are built on this layer's points \
                                 ({}). Move them first.",
                                ids(&plan.blocked_by)
                            ),
                        );
                    }
                    ui.weak("Undo restores the layer, its geometry and every dropped constraint.");
                    ui.horizontal(|ui| {
                        let ok = plan.blocked_by.is_empty();
                        if ui.add_enabled(ok, egui::Button::new("Delete")).clicked() {
                            run = Some(Command::DeleteLayer { id: layer });
                            keep = false;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                }
            });
        if keep {
            self.layer_ui.dialog = Some(dialog);
        }
        if let Some(cmd) = run {
            self.apply_reporting(&cmd);
        }
    }
}

/// Layer IDs in export order (ties by ID).
fn sorted(doc: &Document) -> Vec<LayerId> {
    let mut v: Vec<_> = doc.layers().iter().collect();
    v.sort_by_key(|(id, l)| (l.export_order, id.get()));
    v.into_iter().map(|(id, _)| *id).collect()
}

fn unique_name(doc: &Document) -> String {
    (doc.layers().len() + 1..)
        .map(|n| format!("Layer {n}"))
        .find(|n| doc.layers().values().all(|l| &l.name != n))
        .unwrap_or_else(|| "Layer".to_owned())
}

fn ids(v: &[EntityId]) -> String {
    v.iter()
        .map(|i| format!("#{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn list_constraints(ui: &mut Ui, doc: &Document, list: &[ConstraintId], settings: &Settings) {
    for id in list {
        if let Some(c) = doc.constraint(*id) {
            ui.label(format!("• {}", constrain::describe(doc, c, settings.units)));
        }
    }
}

/// Reorder commands that move `order[pos]` one place up (`-1`) or down
/// (`+1`), renumbering `export_order` to the list positions.
fn reorder(doc: &Document, order: &[LayerId], pos: usize, dir: isize) -> Option<Command> {
    let to = pos.checked_add_signed(dir).filter(|t| *t < order.len())?;
    let mut v = order.to_vec();
    v.swap(pos, to);
    let commands: Vec<_> = v
        .iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let want = u32::try_from(i).ok()?;
            (doc.layer(*id)?.export_order != want).then(|| Command::SetLayerProps {
                id: *id,
                props: LayerProps {
                    export_order: Some(want),
                    ..LayerProps::default()
                },
            })
        })
        .collect();
    Some(Command::Compound {
        label: "Reorder layers".to_owned(),
        commands,
    })
}

#[allow(clippy::too_many_arguments)]
fn layer_row(
    ui: &mut Ui,
    doc: &Document,
    st: &mut LayerUi,
    want: &mut Wants,
    order: &[LayerId],
    pos: usize,
    id: LayerId,
    active: LayerId,
) {
    let Some(l) = doc.layer(id) else {
        return;
    };
    let props = |p: LayerProps| Command::SetLayerProps { id, props: p };
    ui.horizontal(|ui| {
        if ui
            .radio(active == id, "")
            .on_hover_text("Draw on this layer")
            .clicked()
        {
            want.activate = Some(id);
        }
        // Colour: held locally until the pointer is released.
        let mut c = match st.colour {
            Some((i, c)) if i == id => c,
            _ => l.colour,
        };
        if ui.color_edit_button_srgb(&mut c).changed() {
            st.colour = Some((id, c));
        }
        if st.colour.is_some_and(|(i, _)| i == id)
            && !ui.input(|i| i.pointer.any_down())
            && let Some((_, c)) = st.colour.take()
            && c != l.colour
        {
            want.commands.push(props(LayerProps {
                colour: Some(c),
                ..LayerProps::default()
            }));
        }
        // Name: double-click to rename.
        if let Some((rid, text)) = st.rename.as_mut().filter(|(r, _)| *r == id) {
            let r = ui.add(egui::TextEdit::singleline(text).desired_width(120.0));
            r.request_focus();
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if esc {
                st.rename = None;
            } else if enter || r.lost_focus() {
                let new = text.trim().to_owned();
                let rid = *rid;
                st.rename = None;
                if !new.is_empty() && new != l.name && rid == id {
                    want.commands.push(props(LayerProps {
                        name: Some(new),
                        ..LayerProps::default()
                    }));
                }
            }
        } else {
            let text = egui::RichText::new(&l.name);
            let text = if active == id { text.strong() } else { text };
            let r = ui
                .add(egui::Label::new(text).sense(egui::Sense::click()))
                .on_hover_text("Double-click to rename");
            if r.double_clicked() {
                st.rename = Some((id, l.name.clone()));
            }
        }
        ui.weak(l.operation.kind().name());
    });
    ui.horizontal(|ui| {
        if ui
            .selectable_label(l.visible, "Show")
            .on_hover_text("Visible on the canvas (no effect on the solver)")
            .clicked()
        {
            want.commands.push(props(LayerProps {
                visible: Some(!l.visible),
                ..LayerProps::default()
            }));
        }
        if ui
            .selectable_label(l.locked, "Lock")
            .on_hover_text("Fix this layer's geometry")
            .clicked()
        {
            if l.locked {
                want.commands
                    .push(Command::SetLayerLocked { id, locked: false });
            } else {
                want.lock = Some(id);
            }
        }
        if ui
            .selectable_label(l.export_enabled, "Export")
            .on_hover_text("Include in exported files")
            .clicked()
        {
            want.commands.push(props(LayerProps {
                export_enabled: Some(!l.export_enabled),
                ..LayerProps::default()
            }));
        }
        if ui
            .add_enabled(pos > 0, egui::Button::new("▲"))
            .on_hover_text("Earlier in export order")
            .clicked()
        {
            want.commands.extend(reorder(doc, order, pos, -1));
        }
        if ui
            .add_enabled(pos + 1 < order.len(), egui::Button::new("▼"))
            .on_hover_text("Later in export order")
            .clicked()
        {
            want.commands.extend(reorder(doc, order, pos, 1));
        }
        let open = st.expanded == Some(id);
        if ui.selectable_label(open, "Operation").clicked() {
            st.expanded = if open { None } else { Some(id) };
        }
        if ui
            .add_enabled(order.len() > 1, egui::Button::new("✕"))
            .on_hover_text("Delete layer…")
            .clicked()
        {
            want.delete = Some(id);
        }
    });
    if st.expanded == Some(id) {
        operation_editor(ui, st, want, id, l.operation);
    }
}

/// Operation kind and parameters (SPEC §7).
fn operation_editor(
    ui: &mut Ui,
    st: &mut LayerUi,
    want: &mut Wants,
    id: LayerId,
    current: Operation,
) {
    let mut op = match st.op {
        Some((i, o)) if i == id => o,
        _ => current,
    };
    let mut changed = false;
    let mut done = false;
    egui::Grid::new(("op", id.get()))
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Operation");
            let mut kind = op.kind();
            egui::ComboBox::from_id_salt(("opkind", id.get()))
                .selected_text(kind.name())
                .show_ui(ui, |ui| {
                    for k in OperationKind::ALL {
                        ui.selectable_value(&mut kind, k, k.name());
                    }
                });
            if kind != op.kind() {
                want.commands.push(Command::SetLayerProps {
                    id,
                    props: LayerProps {
                        operation: Some(kind.default_operation()),
                        ..LayerProps::default()
                    },
                });
                st.op = None;
            }
            ui.end_row();
            let f = |ui: &mut Ui,
                     label: &str,
                     v: &mut f64,
                     (lo, hi): (f64, f64),
                     unit: &str,
                     (changed, done): (&mut bool, &mut bool)| {
                ui.label(label);
                let r = ui.add(
                    DragValue::new(v)
                        .range(lo..=hi)
                        .speed(0.01)
                        .suffix(unit.to_owned()),
                );
                *changed |= r.changed();
                *done |= r.drag_stopped() || r.lost_focus();
                ui.end_row();
            };
            match &mut op {
                Operation::Cut(p) => {
                    f(
                        ui,
                        "Kerf width",
                        &mut p.kerf_width,
                        (0.0, 1000.0),
                        " mm",
                        (&mut changed, &mut done),
                    );
                    ui.label("Compensation");
                    let before = p.compensation;
                    egui::ComboBox::from_id_salt(("comp", id.get()))
                        .selected_text(format!("{:?}", p.compensation))
                        .show_ui(ui, |ui| {
                            for c in [
                                Compensation::None,
                                Compensation::Inside,
                                Compensation::Outside,
                            ] {
                                ui.selectable_value(&mut p.compensation, c, format!("{c:?}"));
                            }
                        });
                    if p.compensation != before {
                        changed = true;
                        done = true;
                    }
                    ui.end_row();
                    ui.label("Passes");
                    let r = ui.add(DragValue::new(&mut p.passes).range(1..=1000));
                    changed |= r.changed();
                    done |= r.drag_stopped() || r.lost_focus();
                    ui.end_row();
                }
                Operation::Engrave(p) => {
                    f(
                        ui,
                        "Hatch spacing",
                        &mut p.hatch_spacing,
                        (0.001, 1000.0),
                        " mm",
                        (&mut changed, &mut done),
                    );
                    f(
                        ui,
                        "Hatch angle",
                        &mut p.hatch_angle,
                        (-360.0, 360.0),
                        "°",
                        (&mut changed, &mut done),
                    );
                }
                Operation::Draw(p) => {
                    ui.label("Pen number");
                    let r = ui.add(DragValue::new(&mut p.pen_number).range(1..=256));
                    changed |= r.changed();
                    done |= r.drag_stopped() || r.lost_focus();
                    ui.end_row();
                    f(
                        ui,
                        "Pen width",
                        &mut p.pen_width,
                        (0.001, 1000.0),
                        " mm",
                        (&mut changed, &mut done),
                    );
                }
                Operation::DragKnife(p) => {
                    f(
                        ui,
                        "Blade offset",
                        &mut p.blade_offset,
                        (0.0, 1000.0),
                        " mm",
                        (&mut changed, &mut done),
                    );
                }
                Operation::Score | Operation::Reference => {
                    ui.label("");
                    ui.weak(if matches!(op, Operation::Reference) {
                        "Never exported"
                    } else {
                        "No parameters"
                    });
                    ui.end_row();
                }
            }
        });
    if changed {
        st.op = Some((id, op));
    }
    if done
        && let Some((_, o)) = st.op.take()
        && o != current
    {
        want.commands.push(Command::SetLayerProps {
            id,
            props: LayerProps {
                operation: Some(o),
                ..LayerProps::default()
            },
        });
    }
}

#[cfg(test)]
impl LayerUi {
    pub(super) fn expanded_for_test(&mut self, id: LayerId) {
        self.expanded = Some(id);
    }

    pub(super) fn dialog_for_test(&mut self, layer: LayerId, plan: LayerDeletion) {
        self.dialog = Some(Dialog::Delete { layer, plan });
    }
}
