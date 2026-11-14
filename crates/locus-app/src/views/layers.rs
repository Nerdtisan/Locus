// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Browser's layer rows (#118, #326; SPEC §4 Layers, §7 Operations).
//!
//! One compact row per layer, in export order: colour, name, then icons for
//! the operation, export, lock and visibility (M5 design §2). Clicking the
//! name makes the layer active and the Inspector's subject; the row's menu
//! (right-click) holds rename, operation settings, order and delete.
//!
//! Every action is a `Sketcher` command, so each is one undo step. Slider and
//! colour edits are held in a local copy while the pointer is down and
//! committed once, on release, so a drag is not a hundred undo steps.

use crate::constrain;
use crate::settings::Settings;
use crate::state::AppState;
use crate::views::icons;
use egui::{DragValue, Sense, Ui};
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
pub(crate) enum Dialog {
    Lock {
        layer: LayerId,
        report: LockReport,
    },
    Delete {
        layer: LayerId,
        plan: LayerDeletion,
        /// Why the last attempt was refused, shown in the dialog (#292).
        error: Option<String>,
    },
}

/// Transient panel state (never part of the document, SPEC §11).
#[derive(Debug, Default)]
pub struct LayersView {
    expanded: Option<LayerId>,
    rename: Option<(LayerId, String)>,
    colour: Option<(LayerId, [u8; 3])>,
    op: Option<(LayerId, Operation)>,
    dialog: Option<Dialog>,
    /// The Browser's + asked for a new layer.
    add: bool,
    /// The document generation this state belongs to.
    seen: u64,
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

impl LayersView {
    /// Confirm the open delete dialog, as its Delete button does.
    #[cfg(test)]
    pub(crate) fn confirm_delete_for_test(&mut self, app: &mut AppState) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        if let Dialog::Delete { layer, .. } = &dialog {
            let cmd = Command::DeleteLayer { id: *layer };
            self.settle_dialog(app, dialog, Some(cmd), false);
        }
    }

    /// The Layers header's +: a new Draw layer, made active, next frame.
    pub fn request_add(&mut self) {
        self.add = true;
    }

    /// Draw the layer rows. Transient state is dropped when another
    /// document loads.
    pub fn ui(&mut self, ui: &mut Ui, app: &mut AppState, settings: &Settings) {
        if self.seen != app.doc.generation {
            // A + clicked this frame still counts.
            *self = Self {
                seen: app.doc.generation,
                add: self.add,
                ..Self::default()
            };
        }
        app.doc.active_layer = app.active_layer();
        // A canvas selection is the Inspector's subject instead (§2.1).
        if !app.selection.items.is_empty() {
            app.view.browser_layer = None;
        }
        let mut want = Wants {
            add: std::mem::take(&mut self.add),
            ..Wants::default()
        };
        let selected: Vec<EntityId> = app.selection.items.iter().collect();
        {
            let doc = app.doc.sketcher.document();
            let st = &mut *self;
            let row = Row {
                active: app.doc.active_layer,
                picked: app.view.browser_layer,
                selected: &selected,
            };
            let order = sorted(doc);
            for (pos, id) in order.iter().enumerate() {
                layer_row(ui, doc, st, &mut want, &order, pos, *id, &row);
            }
        }
        self.apply_wants(app, want, ui, settings);
    }

    fn apply_wants(&mut self, app: &mut AppState, want: Wants, ui: &Ui, settings: &Settings) {
        if let Some(id) = want.activate {
            app.doc.active_layer = id;
            // The layer is now what the Inspector shows (design §2.1).
            app.clear_selection();
            app.view.browser_layer = Some(id);
        }
        if want.add {
            let cmd = new_layer(app.doc.sketcher.document());
            app.apply_reporting(&cmd);
            if app.doc.notice.is_none()
                && let Some(&newest) = app.doc.sketcher.document().layers().keys().next_back()
            {
                app.doc.active_layer = newest;
            }
        }
        for cmd in &want.commands {
            app.apply_reporting(cmd);
        }
        if let Some(id) = want.lock {
            match app.doc.sketcher.check_lock(id) {
                Ok(r) if r.conflicts.is_empty() && r.redundant.is_empty() => {
                    app.apply_reporting(&Command::SetLayerLocked { id, locked: true });
                }
                Ok(report) => {
                    self.dialog = Some(Dialog::Lock { layer: id, report });
                }
                Err(e) => app.doc.notice = Some(e.to_string()),
            }
        }
        if let Some(id) = want.delete {
            match app.doc.sketcher.document().layer_deletion(id) {
                Ok(plan) => {
                    self.dialog = Some(Dialog::Delete {
                        layer: id,
                        plan,
                        error: None,
                    });
                }
                Err(e) => app.doc.notice = Some(e.to_string()),
            }
        }
        self.layer_dialog(app, ui.ctx(), settings);
    }

    /// Runs the confirmed command and decides whether the dialog stays. A
    /// refusal stays on screen with its reason, never only in the status bar
    /// (#292).
    fn settle_dialog(
        &mut self,
        app: &mut AppState,
        mut dialog: Dialog,
        run: Option<Command>,
        mut keep: bool,
    ) {
        if let Some(cmd) = run
            && !app.apply_reporting(&cmd)
            && let Dialog::Delete { error, .. } = &mut dialog
        {
            *error = app.doc.notice.clone();
            keep = true;
        }
        if keep {
            self.dialog = Some(dialog);
        }
    }

    /// The lock / delete confirmation (SPEC §4 rules 3 and 4).
    fn layer_dialog(&mut self, app: &mut AppState, ctx: &egui::Context, settings: &Settings) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let doc = app.doc.sketcher.document();
        let mut keep = true;
        let mut run: Option<Command> = None;
        let (title, layer) = match &dialog {
            Dialog::Lock { layer, .. } => ("Lock layer", *layer),
            Dialog::Delete { layer, .. } => ("Delete layer", *layer),
        };
        let name = doc.layer(layer).map_or("?", |l| l.name.as_str()).to_owned();
        crate::dialog::Dialog::new(title)
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
                Dialog::Delete { plan, error, .. } => {
                    ui.label(format!(
                        "Delete “{name}” and its {} entit{}?",
                        plan.entities.len(),
                        if plan.entities.len() == 1 { "y" } else { "ies" }
                    ));
                    if !plan.recipes.is_empty() {
                        ui.label(format!(
                            "{} pattern(s) / plugin object(s) built on it will be deleted too.",
                            plan.recipes.len()
                        ));
                    }
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
                    if let Some(why) = error {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            format!("Could not delete the layer: {why}"),
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
        self.settle_dialog(app, dialog, run, keep);
    }
}

impl AppState {
    /// The layer new geometry goes on. Falls back to the first layer in
    /// export order when the remembered one is gone.
    pub(crate) fn active_layer(&self) -> LayerId {
        let doc = self.doc.sketcher.document();
        if doc.layer(self.doc.active_layer).is_some() {
            self.doc.active_layer
        } else {
            sorted(doc)
                .first()
                .copied()
                .unwrap_or_else(Document::default_layer)
        }
    }
}

/// The command behind "+ Add layer": a Draw layer (#288), next palette colour.
fn new_layer(doc: &Document) -> Command {
    Command::AddLayer {
        name: unique_name(doc),
        colour: PALETTE[doc.layers().len() % PALETTE.len()],
        operation: OperationKind::Draw.default_operation(),
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

/// What every row needs to know about the app.
struct Row<'a> {
    active: LayerId,
    /// The layer picked in the Browser, if any.
    picked: Option<LayerId>,
    selected: &'a [EntityId],
}

/// A small icon button for a layer row; dim when off or disabled.
fn icon_toggle(ui: &mut Ui, icon: &str, on: bool, enabled: bool, tip: &str) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, r) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), sense);
    if enabled && r.hovered() {
        ui.painter()
            .rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let color = if on && enabled {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    icons::paint(ui.painter(), rect.shrink(2.0), icon, "", color);
    r.on_hover_text(tip)
}

#[allow(clippy::too_many_arguments)]
fn layer_row(
    ui: &mut Ui,
    doc: &Document,
    st: &mut LayersView,
    want: &mut Wants,
    order: &[LayerId],
    pos: usize,
    id: LayerId,
    row: &Row<'_>,
) {
    let Some(l) = doc.layer(id) else {
        return;
    };
    let props = |p: LayerProps| Command::SetLayerProps { id, props: p };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        // Colour: held locally until the pointer is released.
        let mut c = match st.colour {
            Some((i, c)) if i == id => c,
            _ => l.colour,
        };
        ui.scope(|ui| {
            ui.spacing_mut().interact_size = egui::vec2(14.0, 14.0);
            if ui
                .color_edit_button_srgb(&mut c)
                .on_hover_text("Layer colour")
                .changed()
            {
                st.colour = Some((id, c));
            }
        });
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
        // Name: click to make active, double-click to rename.
        if let Some((rid, text)) = st.rename.as_mut().filter(|(r, _)| *r == id) {
            let r = ui.add(egui::TextEdit::singleline(text).desired_width(110.0));
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
            let text = if row.active == id {
                text.strong()
            } else {
                text
            };
            let r = ui
                .selectable_label(row.picked == Some(id), text)
                .on_hover_text(
                    "Click: draw on this layer and show it in the Inspector. \
                     Double-click to rename; right-click for more.",
                );
            if r.clicked() {
                want.activate = Some(id);
            }
            if r.double_clicked() {
                st.rename = Some((id, l.name.clone()));
            }
            r.context_menu(|ui| row_menu(ui, l, st, want, doc, order, pos, row.selected));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            let (icon, tip) = if l.visible {
                (
                    "ui.eye",
                    "Visible on the canvas: click to hide (no effect on the solver)",
                )
            } else {
                ("ui.eye_off", "Hidden: click to show")
            };
            if icon_toggle(ui, icon, l.visible, true, tip).clicked() {
                want.commands.push(props(LayerProps {
                    visible: Some(!l.visible),
                    ..LayerProps::default()
                }));
            }
            let (icon, tip) = if l.locked {
                ("ui.lock", "Locked: geometry fixed. Click to unlock")
            } else {
                ("ui.unlock", "Click to lock this layer's geometry")
            };
            if icon_toggle(ui, icon, l.locked, true, tip).clicked() {
                if l.locked {
                    want.commands
                        .push(Command::SetLayerLocked { id, locked: false });
                } else {
                    want.lock = Some(id);
                }
            }
            // Reference geometry is never exported (SPEC §7), whatever the flag says.
            let reference = matches!(l.operation, Operation::Reference);
            let exported = l.export_enabled && !reference;
            let (icon, tip) = match (reference, exported) {
                (true, _) => ("ui.export_off", "Reference layers are never exported"),
                (false, true) => (
                    "ui.export",
                    "Exported: click to leave out of exported files",
                ),
                (false, false) => ("ui.export_off", "Not exported: click to include"),
            };
            if icon_toggle(ui, icon, exported, !reference, tip).clicked() {
                want.commands.push(props(LayerProps {
                    export_enabled: Some(!l.export_enabled),
                    ..LayerProps::default()
                }));
            }
            let kind = l.operation.kind();
            let open = st.expanded == Some(id);
            let tip = format!("{}: click for its settings", kind.name());
            if icon_toggle(ui, icons::operation(kind), true, true, &tip).clicked() {
                st.expanded = if open { None } else { Some(id) };
            }
        });
    });
    if st.expanded == Some(id) {
        ui.indent(("layer-op", id), |ui| {
            operation_editor(ui, st, want, id, l.operation);
        });
    }
}

/// A layer row's menu: rename, settings, order and delete (design §2).
#[allow(clippy::too_many_arguments)]
fn row_menu(
    ui: &mut Ui,
    l: &locus_doc::Layer,
    st: &mut LayersView,
    want: &mut Wants,
    doc: &Document,
    order: &[LayerId],
    pos: usize,
    selected: &[EntityId],
) {
    let id = order[pos];
    if ui.button("Rename").clicked() {
        st.rename = Some((id, l.name.clone()));
        ui.close();
    }
    if ui.button("Operation settings…").clicked() {
        st.expanded = Some(id);
        ui.close();
    }
    if ui
        .add_enabled(
            !selected.is_empty(),
            egui::Button::new(format!("Move selection here ({})", selected.len())),
        )
        .clicked()
    {
        want.commands.push(Command::MoveToLayer {
            entities: selected.to_vec(),
            layer: id,
        });
        ui.close();
    }
    ui.separator();
    if ui
        .add_enabled(pos > 0, egui::Button::new("Move up"))
        .on_hover_text("Earlier in export order")
        .clicked()
    {
        want.commands.extend(reorder(doc, order, pos, -1));
        ui.close();
    }
    if ui
        .add_enabled(pos + 1 < order.len(), egui::Button::new("Move down"))
        .on_hover_text("Later in export order")
        .clicked()
    {
        want.commands.extend(reorder(doc, order, pos, 1));
        ui.close();
    }
    ui.separator();
    if ui
        .add_enabled(order.len() > 1, egui::Button::new("Delete layer…"))
        .clicked()
    {
        want.delete = Some(id);
        ui.close();
    }
}

/// Operation kind and parameters (SPEC §7).
fn operation_editor(
    ui: &mut Ui,
    st: &mut LayersView,
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
                    ui.label("Crosshatch");
                    let r = ui.checkbox(&mut p.crosshatch, "");
                    changed |= r.changed();
                    done |= r.changed();
                    ui.end_row();
                }
                Operation::Draw => {
                    // Draw layers have no pen settings since M5: the pen is
                    // each object's style (design §2.1, #287).
                    ui.label("");
                    ui.weak("Pen, width: per object")
                        .on_hover_text("Pen and width are each object's Style");
                    ui.end_row();
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
impl LayersView {
    pub(crate) fn expanded_for_test(&mut self, id: LayerId) {
        self.expanded = Some(id);
    }

    pub(crate) fn dialog_error_for_test(&self) -> Option<&str> {
        match &self.dialog {
            Some(Dialog::Delete { error, .. }) => error.as_deref(),
            _ => None,
        }
    }

    pub(crate) fn has_dialog_for_test(&self) -> bool {
        self.dialog.is_some()
    }

    pub(crate) fn dialog_for_test(&mut self, layer: LayerId, plan: LayerDeletion) {
        self.dialog = Some(Dialog::Delete {
            layer,
            plan,
            error: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(lv: &mut LayersView, app: &mut AppState) {
        let ctx = egui::Context::default();
        let settings = Settings::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| lv.ui(ui, app, &settings));
    }

    /// Picking a layer row makes it active and the Inspector's subject; a
    /// canvas selection takes over again (#326, design §2.1).
    #[test]
    fn a_picked_layer_is_active_until_the_canvas_selects() {
        let mut app = AppState::default();
        let mut lv = LayersView::default();
        lv.request_add();
        frame(&mut lv, &mut app);
        let ids: Vec<LayerId> = app.document().layers().keys().copied().collect();
        assert_eq!(ids.len(), 2, "the Browser's + added a layer");
        let first = ids[0];
        app.select_all();
        let mut want = Some(Wants {
            activate: Some(first),
            ..Wants::default()
        });
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(w) = want.take() {
                lv.apply_wants(&mut app, w, ui, &Settings::default());
            }
        });
        assert_eq!(app.doc.active_layer, first);
        assert_eq!(app.view.browser_layer, Some(first));
        assert!(app.selection.items.is_empty());
        app.apply_reporting(&Command::AddEntity {
            layer: first,
            construction: false,
            style: Default::default(),
            geom: locus_doc::Geom::Point {
                pos: locus_geom::Vec2::ZERO,
            },
        });
        app.select_all();
        frame(&mut lv, &mut app);
        assert_eq!(app.view.browser_layer, None);
    }

    #[test]
    fn add_layer_defaults_to_draw() {
        let doc = Document::new();
        assert!(matches!(
            new_layer(&doc),
            Command::AddLayer {
                operation: Operation::Draw,
                ..
            }
        ));
    }
}
