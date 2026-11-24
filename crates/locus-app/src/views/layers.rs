// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Browser's layer rows (#118, #326; SPEC §4 Layers, §7 Operations).
//!
//! One compact row per layer, in export order: colour, name, then icons for
//! the operation, export, lock and visibility (M5 design §2). Clicking the
//! name makes the layer active and the Inspector's subject; the row's menu
//! (right-click) holds rename, operation settings, order and delete.
//!
//! Rows reorder by dragging their grip (#462): a 2 px insertion line shows
//! where the layer lands, Esc cancels, and Alt+↑ / Alt+↓ move the focused
//! row. Each move is one `ReorderLayer` step (ADR 0019 decision 9).
//!
//! Every action is a `Sketcher` command, so each is one undo step. Slider and
//! colour edits are held in a local copy while the pointer is down and
//! committed once, on release, so a drag is not a hundred undo steps.

use crate::constrain;
use crate::settings::Settings;
use crate::state::AppState;
use crate::views::{icons, op_chip};
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

/// The drag-and-drop payload of a layer row's grip (#462).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LayerDrag(LayerId);

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
        let machine = crate::views::machine_profile::current(app.document(), settings);
        {
            let doc = app.doc.sketcher.document();
            let st = &mut *self;
            let row = Row {
                active: app.doc.active_layer,
                picked: app.view.browser_layer,
                selected: &selected,
                machine: machine.as_ref(),
            };
            let order = doc.layer_order();
            let rects: Vec<egui::Rect> = order
                .iter()
                .enumerate()
                .map(|(pos, id)| {
                    ui.scope(|ui| layer_row(ui, doc, st, &mut want, &order, pos, *id, &row))
                        .response
                        .rect
                })
                .collect();
            want.commands.extend(drop_layer(ui, &order, &rects));
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
            let machine = crate::views::machine_profile::current(app.document(), settings);
            let cmd = new_layer(app.doc.sketcher.document(), machine.as_ref());
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
            doc.layer_order()
                .first()
                .copied()
                .unwrap_or_else(Document::default_layer)
        }
    }
}

/// The command behind "+ Add layer": the machine profile's operation (a
/// laser Cut, a plotter Draw, a vinyl cutter Drag knife; Draw without a
/// profile, #288, #419), next palette colour.
fn new_layer(doc: &Document, machine: Option<&locus_export::MachineProfile>) -> Command {
    Command::AddLayer {
        name: unique_name(doc),
        colour: PALETTE[doc.layers().len() % PALETTE.len()],
        operation: crate::views::machine_profile::new_layer_operation(machine),
    }
}

/// A fresh operation of `kind`, with the machine profile's default kerf or
/// blade offset (SPEC §7).
pub(crate) fn operation_for(
    kind: OperationKind,
    machine: Option<&locus_export::MachineProfile>,
) -> Operation {
    machine.map_or_else(|| kind.default_operation(), |p| p.defaults.operation(kind))
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

/// The command that moves `order[pos]` one place up (`-1`) or down (`+1`).
fn reorder(order: &[LayerId], pos: usize, dir: isize) -> Option<Command> {
    let to = pos.checked_add_signed(dir).filter(|t| *t < order.len())?;
    Some(Command::ReorderLayer {
        layer: *order.get(pos)?,
        to: u32::try_from(to).ok()?,
    })
}

/// Where a layer dropped at height `y` lands: the slot before the first
/// row whose middle is below `y`, as a position in the list without it.
fn drop_slot(rects: &[egui::Rect], y: f32) -> usize {
    rects
        .iter()
        .position(|r| y < r.center().y)
        .unwrap_or(rects.len())
}

/// While a row's grip is dragged (#462): the 2 px insertion line, Esc to
/// cancel, and on release over the list the `ReorderLayer` step (none when
/// the layer would land where it is).
fn drop_layer(ui: &Ui, order: &[LayerId], rects: &[egui::Rect]) -> Option<Command> {
    let ctx = ui.ctx();
    let LayerDrag(layer) = *egui::DragAndDrop::payload::<LayerDrag>(ctx)?;
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        egui::DragAndDrop::clear_payload(ctx);
        return None;
    }
    let list = rects.iter().copied().reduce(|a, b| a.union(b))?;
    let p = ctx
        .pointer_interact_pos()
        .filter(|p| list.expand(8.0).contains(*p))?;
    let slot = drop_slot(rects, p.y);
    let y = rects.get(slot).map_or(list.bottom(), |r| r.top());
    let accent = crate::theme::tokens(ui).accent;
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, ui.id()))
        .hline(list.x_range(), y, egui::Stroke::new(2.0, accent));
    if !ui.input(|i| i.pointer.any_released()) {
        return None;
    }
    egui::DragAndDrop::clear_payload(ctx);
    let from = order.iter().position(|id| *id == layer)?;
    let to = if slot > from { slot - 1 } else { slot };
    (to != from).then(|| Command::ReorderLayer {
        layer,
        to: u32::try_from(to).unwrap_or(u32::MAX),
    })
}

/// The row's ⋮⋮ grip: drag it to reorder the layer (#462).
fn grip(ui: &mut Ui, id: LayerId) {
    let r = ui.dnd_drag_source(egui::Id::new(("layer-grip", id)), LayerDrag(id), |ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 18.0), Sense::hover());
        let c = ui.visuals().weak_text_color();
        for dx in [-1.5, 1.5] {
            for dy in [-4.0, 0.0, 4.0] {
                ui.painter()
                    .circle_filled(rect.center() + egui::vec2(dx, dy), 1.0, c);
            }
        }
    });
    if r.response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    r.response
        .on_hover_text("Drag to reorder (export and draw order). Alt+↑ / Alt+↓ move the row.");
}

/// What every row needs to know about the app.
struct Row<'a> {
    active: LayerId,
    /// The layer picked in the Browser, if any.
    picked: Option<LayerId>,
    selected: &'a [EntityId],
    /// The document's machine profile: the operations offered (#419).
    machine: Option<&'a locus_export::MachineProfile>,
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
        grip(ui, id);
        // Colour: held locally until the pointer is released.
        let mut c = match st.colour {
            Some((i, c)) if i == id => c,
            _ => l.colour,
        };
        // The mockup's small row swatch (`.sw`).
        if crate::theme::swatch_sized(ui, &mut c, 12.0)
            .on_hover_text("Layer colour")
            .changed()
        {
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
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            // Right to left: lock, then the eye, as the mockup's row ends.
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
            operation_chip(ui, l, st, want, id, row.machine);
            // The name gets the space the icons leave, truncated, so a
            // narrow Browser never draws the icons over it (#358).
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                // Name: click to make active, double-click to rename.
                if let Some((rid, text)) = st.rename.as_mut().filter(|(r, _)| *r == id) {
                    let r = ui
                        .add(egui::TextEdit::singleline(text).desired_width(ui.available_width()));
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
                        .add(egui::Button::selectable(row.picked == Some(id), text).truncate())
                        .on_hover_text(
                            "Click: draw on this layer and show it in the Inspector. \
                             Double-click to rename; right-click for more.",
                        );
                    if r.clicked() {
                        want.activate = Some(id);
                    }
                    // Alt+↑ / Alt+↓ move the focused (or picked) row (#462).
                    if (r.has_focus() || row.picked == Some(id))
                        && !ui.ctx().egui_wants_keyboard_input()
                    {
                        let alt = egui::Modifiers::ALT;
                        let up = ui.input_mut(|i| i.consume_key(alt, egui::Key::ArrowUp));
                        let down = ui.input_mut(|i| i.consume_key(alt, egui::Key::ArrowDown));
                        let dir = if up {
                            -1
                        } else if down {
                            1
                        } else {
                            0
                        };
                        if dir != 0 {
                            want.commands.extend(reorder(order, pos, dir));
                        }
                    }
                    if r.double_clicked() {
                        st.rename = Some((id, l.name.clone()));
                    }
                    r.context_menu(|ui| row_menu(ui, l, st, want, order, pos, row.selected));
                }
            });
        });
    });
    if st.expanded == Some(id) {
        ui.indent(("layer-op", id), |ui| {
            operation_editor(ui, st, want, id, l.operation, row.machine);
        });
    }
}

/// The row's operation chip (#420): the operation's colour and short name,
/// outlined when the machine can't run it. One click opens the operations
/// the machine offers; picking one changes the layer (one undo step), as in
/// LightBurn's layer list. Settings… opens the parameters under the row.
fn operation_chip(
    ui: &mut Ui,
    l: &locus_doc::Layer,
    st: &mut LayersView,
    want: &mut Wants,
    id: LayerId,
    machine: Option<&locus_export::MachineProfile>,
) {
    let kind = l.operation.kind();
    let foreign = machine.filter(|p| !p.machine.offers(kind));
    let tip = match foreign {
        Some(p) => format!(
            "{}: {} can't run this operation. Click to change it",
            kind.name(),
            p.name
        ),
        None => format!(
            "{}: click to change the operation or its settings",
            kind.name()
        ),
    };
    let r = op_chip::chip(ui, kind, foreign.is_some()).on_hover_text(tip);
    egui::Popup::menu(&r)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(150.0);
            ui.weak("OPERATION");
            for k in crate::views::machine_profile::offered(machine, kind) {
                let picked = ui
                    .horizontal(|ui| {
                        let (dot, _) =
                            ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
                        ui.painter().circle_filled(
                            dot.center(),
                            4.0,
                            op_chip::colour(k, ui.visuals().dark_mode),
                        );
                        ui.add(egui::Button::selectable(k == kind, k.name()))
                            .clicked()
                    })
                    .inner;
                if picked {
                    if k != kind {
                        want.commands.push(Command::SetLayerProps {
                            id,
                            props: LayerProps {
                                operation: Some(operation_for(k, machine)),
                                ..LayerProps::default()
                            },
                        });
                        st.op = None;
                    }
                    ui.close();
                }
            }
            ui.separator();
            let open = st.expanded == Some(id);
            if ui
                .button(if open { "Hide settings" } else { "Settings…" })
                .clicked()
            {
                st.expanded = if open { None } else { Some(id) };
                ui.close();
            }
        });
}

/// A layer row's menu: rename, settings, order and delete (design §2).
#[allow(clippy::too_many_arguments)]
fn row_menu(
    ui: &mut Ui,
    l: &locus_doc::Layer,
    st: &mut LayersView,
    want: &mut Wants,
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
        want.commands.extend(reorder(order, pos, -1));
        ui.close();
    }
    if ui
        .add_enabled(pos + 1 < order.len(), egui::Button::new("Move down"))
        .on_hover_text("Later in export order")
        .clicked()
    {
        want.commands.extend(reorder(order, pos, 1));
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
    machine: Option<&locus_export::MachineProfile>,
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
                    for k in crate::views::machine_profile::offered(machine, op.kind()) {
                        ui.selectable_value(&mut kind, k, k.name());
                    }
                });
            if kind != op.kind() {
                want.commands.push(Command::SetLayerProps {
                    id,
                    props: LayerProps {
                        operation: Some(operation_for(kind, machine)),
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
                    // Mode and overlap rule (#455), as LightBurn has them.
                    ui.label("Mode");
                    let before = (p.mode, p.overlap);
                    egui::ComboBox::from_id_salt(("engrave-mode", id.get()))
                        .selected_text(p.mode.name())
                        .show_ui(ui, |ui| {
                            for m in locus_doc::EngraveMode::ALL {
                                let tip = match m {
                                    locus_doc::EngraveMode::Fill => "Hatch the inside, no outline",
                                    locus_doc::EngraveMode::Line => {
                                        "Trace the outline only: mark an edge"
                                    }
                                    locus_doc::EngraveMode::LineFill => "The outline and the hatch",
                                    locus_doc::EngraveMode::OffsetFill => {
                                        "Concentric rings, the spacing apart"
                                    }
                                };
                                ui.selectable_value(&mut p.mode, m, m.name())
                                    .on_hover_text(tip);
                            }
                        });
                    ui.end_row();
                    ui.label("Overlaps");
                    egui::ComboBox::from_id_salt(("engrave-overlap", id.get()))
                        .selected_text(p.overlap.name())
                        .show_ui(ui, |ui| {
                            for o in locus_doc::Overlap::ALL {
                                let tip = match o {
                                    locus_doc::Overlap::EvenOdd => {
                                        "Overlaps and shapes in shapes are left empty"
                                    }
                                    locus_doc::Overlap::Union => {
                                        "Overlapping shapes are engraved as one area"
                                    }
                                };
                                ui.selectable_value(&mut p.overlap, o, o.name())
                                    .on_hover_text(tip);
                            }
                        });
                    ui.end_row();
                    if (p.mode, p.overlap) != before {
                        changed = true;
                        done = true;
                    }
                    ui.label("");
                    ui.weak("To mark just an edge: set Mode to Line, or use a Score layer.");
                    ui.end_row();
                    if p.mode == locus_doc::EngraveMode::Line {
                        return;
                    }
                    f(
                        ui,
                        if p.mode == locus_doc::EngraveMode::OffsetFill {
                            "Ring spacing"
                        } else {
                            "Hatch spacing"
                        },
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
                    if p.mode != locus_doc::EngraveMode::OffsetFill {
                        ui.label("Crosshatch");
                        let r = crate::theme::checkbox(ui, &mut p.crosshatch, "");
                        changed |= r.changed();
                        done |= r.changed();
                        ui.end_row();
                    }
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
    fn a_dropped_row_lands_before_the_first_row_below_the_pointer() {
        // #462: three 20 pt rows; the slot follows the row middles.
        let rows: Vec<egui::Rect> = (0..3)
            .map(|k| {
                let top = 20.0 * k as f32;
                egui::Rect::from_min_max(egui::pos2(0.0, top), egui::pos2(100.0, top + 20.0))
            })
            .collect();
        assert_eq!(drop_slot(&rows, -5.0), 0);
        assert_eq!(drop_slot(&rows, 9.0), 0);
        assert_eq!(drop_slot(&rows, 11.0), 1);
        assert_eq!(drop_slot(&rows, 45.0), 2);
        assert_eq!(drop_slot(&rows, 55.0), 3);
    }

    #[test]
    fn move_up_and_down_are_reorder_steps_that_stop_at_the_ends() {
        let order = [LayerId::new(1), LayerId::new(2), LayerId::new(3)];
        assert_eq!(
            reorder(&order, 1, -1),
            Some(Command::ReorderLayer {
                layer: LayerId::new(2),
                to: 0
            })
        );
        assert_eq!(
            reorder(&order, 1, 1),
            Some(Command::ReorderLayer {
                layer: LayerId::new(2),
                to: 2
            })
        );
        assert_eq!(reorder(&order, 0, -1), None);
        assert_eq!(reorder(&order, 2, 1), None);
    }

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

    /// #358: hiding a layer takes its geometry off the canvas; showing it
    /// brings it back. A view setting, not an export exclusion.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn hiding_a_layer_hides_its_geometry() {
        let mut app = AppState::default();
        app.load(crate::demo::sketch());
        let layer = app.active_layer();
        let shown = app.doc.scene.items.len();
        assert!(shown > 0);
        let set = |visible| Command::SetLayerProps {
            id: layer,
            props: LayerProps {
                visible: Some(visible),
                ..LayerProps::default()
            },
        };
        let annots = app.doc.scene.annots.len();
        assert!(annots > 0, "the demo has dimensions and constraint symbols");
        assert!(app.apply_reporting(&set(false)), "{:?}", app.doc.notice);
        assert!(!app.document().layer(layer).unwrap().visible);
        assert!(app.doc.scene.items.is_empty());
        // Its dimensions and constraint symbols go too, or the layer
        // doesn't look hidden (the bug in #358).
        assert!(app.doc.scene.annots.is_empty());
        // Hiding is a view setting: export is untouched.
        assert!(app.document().layer(layer).unwrap().export_enabled);
        assert!(app.apply_reporting(&set(true)));
        assert_eq!(app.doc.scene.items.len(), shown);
        assert_eq!(app.doc.scene.annots.len(), annots);
    }

    #[test]
    fn add_layer_defaults_to_draw() {
        let doc = Document::new();
        assert!(matches!(
            new_layer(&doc, None),
            Command::AddLayer {
                operation: Operation::Draw,
                ..
            }
        ));
    }

    /// #419: a new layer takes the machine profile's operation and defaults.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn add_layer_follows_the_machine_profile() {
        let doc = Document::new();
        let mut cricut = locus_export::profile::find("cricut", &[]).unwrap();
        cricut.defaults.blade_offset_mm = Some(0.3);
        match new_layer(&doc, Some(&cricut)) {
            Command::AddLayer {
                operation: Operation::DragKnife(p),
                ..
            } => assert!((p.blade_offset - 0.3).abs() < 1e-12),
            other => panic!("{other:?}"),
        }
        let laser = locus_export::profile::find("lightburn", &[]).unwrap();
        assert!(matches!(
            new_layer(&doc, Some(&laser)),
            Command::AddLayer {
                operation: Operation::Cut(_),
                ..
            }
        ));
    }
}
