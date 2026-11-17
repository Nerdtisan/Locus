// SPDX-License-Identifier: AGPL-3.0-or-later
//! The document's machine profile (SPEC §7, #419): the picker shown in the
//! app bar, File menu and Inspector, and the offer to update layers whose
//! operation the new machine can't run.
//!
//! Choosing a profile is one undoable `SetProfile` (§11). It never rewrites
//! layers by itself: when some layers use operations the machine doesn't
//! offer, the user is asked, and accepting is one more undoable step.

use crate::settings::Settings;
use crate::state::AppState;
use locus_doc::{Command, Document, LayerId, LayerProps, OperationKind};
use locus_export::MachineProfile;
use locus_export::profile::MachineKind;

/// Layers the user may want moved onto the new machine's operations.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerOffer {
    /// The profile's display name.
    pub profile: String,
    /// Each layer, its current operation and the one it would get.
    pub layers: Vec<(LayerId, String, OperationKind, OperationKind)>,
    /// The command that updates them all, one undo step.
    pub command: Command,
}

/// The document's profile, looked up among the presets and user profiles.
pub(crate) fn current(doc: &Document, settings: &Settings) -> Option<MachineProfile> {
    doc.profile()
        .and_then(|id| locus_export::profile::find(id, &settings.profiles))
}

/// The operation a layer of `kind` gets on `machine`: the same one when the
/// machine runs it, its counterpart for a through-cut (Cut ↔ Drag knife),
/// otherwise the machine's default.
pub(crate) fn counterpart(kind: OperationKind, machine: MachineKind) -> OperationKind {
    use OperationKind::{Cut, DragKnife};
    if machine.offers(kind) {
        return kind;
    }
    match kind {
        Cut if machine.offers(DragKnife) => DragKnife,
        DragKnife if machine.offers(Cut) => Cut,
        _ => machine.default_operation(),
    }
}

/// The offer for `profile`, or `None` when every layer already fits.
pub(crate) fn offer(doc: &Document, profile: &MachineProfile) -> Option<LayerOffer> {
    let mut layers: Vec<_> = doc
        .layers()
        .iter()
        .filter_map(|(id, l)| {
            let from = l.operation.kind();
            let to = counterpart(from, profile.machine);
            (to != from).then(|| (*id, l.name.clone(), from, to))
        })
        .collect();
    if layers.is_empty() {
        return None;
    }
    layers.sort_by_key(|(id, ..)| doc.layer(*id).map(|l| (l.export_order, id.get())));
    let commands = layers
        .iter()
        .map(|(id, _, _, to)| Command::SetLayerProps {
            id: *id,
            props: LayerProps {
                operation: Some(profile.defaults.operation(*to)),
                ..LayerProps::default()
            },
        })
        .collect();
    Some(LayerOffer {
        profile: profile.name.clone(),
        layers,
        command: Command::Compound {
            label: format!("Update layers for {}", profile.name),
            commands,
        },
    })
}

/// The operations a layer picker offers: the machine's, plus the layer's
/// own when the machine doesn't run it (so it still shows, marked).
pub(crate) fn offered(
    profile: Option<&MachineProfile>,
    current: OperationKind,
) -> Vec<OperationKind> {
    let machine = profile.map_or(MachineKind::Generic, |p| p.machine);
    OperationKind::ALL
        .into_iter()
        .filter(|k| machine.offers(*k) || *k == current)
        .collect()
}

/// The operation "+ Add layer" gives a new layer (SPEC §7, #419).
pub(crate) fn new_layer_operation(profile: Option<&MachineProfile>) -> locus_doc::Operation {
    profile.map_or_else(
        || MachineKind::Generic.default_operation().default_operation(),
        MachineProfile::new_layer_operation,
    )
}

impl AppState {
    /// Choose the document's machine profile (`None` clears it): one undoable
    /// command. Layers are left alone; when some don't fit the machine, the
    /// offer to update them is queued for [`profile_offer_window`].
    pub fn choose_profile(&mut self, id: Option<String>, settings: &Settings) -> bool {
        if self.document().profile() == id.as_deref() {
            return true;
        }
        if !self.apply_reporting(&Command::SetProfile { id: id.clone() }) {
            return false;
        }
        let profile = id
            .as_deref()
            .and_then(|id| locus_export::profile::find(id, &settings.profiles));
        // A profile's pen width seeds the style new geometry gets (#287).
        if let Some(w) = profile.as_ref().and_then(|p| p.defaults.pen_width_mm) {
            self.doc.current_style.width = w;
        }
        self.view.profile_offer = profile.and_then(|p| offer(self.document(), &p));
        true
    }

    /// Accept the queued offer: one undoable step.
    pub(crate) fn accept_profile_offer(&mut self) -> bool {
        match self.view.profile_offer.take() {
            Some(o) => self.apply_reporting(&o.command),
            None => false,
        }
    }
}

/// The profiles as a menu: None, then every preset and user profile. Returns
/// `true` when "Manage profiles…" was picked.
pub fn picker_menu(ui: &mut egui::Ui, app: &mut AppState, settings: &Settings) -> bool {
    let now = app.document().profile().map(str::to_owned);
    let mut chosen: Option<Option<String>> = None;
    if ui
        .selectable_label(now.is_none(), "None (choose at export)")
        .clicked()
    {
        chosen = Some(None);
    }
    ui.separator();
    let mut all = locus_export::profile::presets();
    all.extend(settings.profiles.iter().cloned());
    for p in &all {
        let on = now.as_deref() == Some(p.id.as_str());
        if ui
            .selectable_label(on, &p.name)
            .on_hover_text(summary(p))
            .clicked()
        {
            chosen = Some(Some(p.id.clone()));
        }
    }
    ui.separator();
    let manage = ui.button("Manage profiles…").clicked();
    let picked = chosen.is_some();
    if let Some(id) = chosen {
        app.choose_profile(id, settings);
    }
    if picked || manage {
        ui.close();
    }
    manage
}

/// The document's profile as a button with a painted ▾ (the fonts have no
/// ▾) that opens [`picker_menu`]. Returns `true` when "Manage profiles…" was
/// picked. A plain popup, not `menu_button`: inside the app bar's `MenuBar`
/// a nested menu button never opens.
pub fn picker_button(
    ui: &mut egui::Ui,
    app: &mut AppState,
    settings: &Settings,
    text: egui::RichText,
    tip: &str,
) -> bool {
    let r = ui
        .add(
            egui::Button::new(text)
                .frame_when_inactive(false)
                .right_text("    "),
        )
        .on_hover_text(tip);
    let c = ui.style().interact(&r).fg_stroke.color;
    let at = egui::pos2(r.rect.right() - 10.0, r.rect.center().y);
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            at + egui::vec2(-4.0, -2.0),
            at + egui::vec2(4.0, -2.0),
            at + egui::vec2(0.0, 3.0),
        ],
        c,
        egui::Stroke::NONE,
    ));
    let mut manage = false;
    egui::Popup::menu(&r)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(260.0);
            manage = picker_menu(ui, app, settings);
        });
    manage
}

/// "Laser cutter · SVG · bed 400 × 400 mm, origin top left".
pub(crate) fn summary(p: &MachineProfile) -> String {
    let bed = p.bed.map_or_else(
        || "no bed".to_owned(),
        |b| {
            format!(
                "bed {} × {} mm, origin {}",
                trim(b.width_mm),
                trim(b.height_mm),
                b.origin.name().to_lowercase()
            )
        },
    );
    format!("{} · {} · {bed}", p.machine.name(), p.format.to_uppercase())
}

fn trim(v: f64) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map_or(s.clone(), str::to_owned)
}

/// The "update layers?" window after a profile change.
pub fn profile_offer_window(ctx: &egui::Context, app: &mut AppState) {
    let Some(o) = app.view.profile_offer.clone() else {
        return;
    };
    let mut accept = false;
    let mut dismiss = false;
    egui::Window::new("Update layers for the machine?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(format!(
                "{} can't run every layer's operation. Layers are never changed without asking:",
                o.profile
            ));
            ui.add_space(4.0);
            egui::Grid::new("profile-offer")
                .striped(true)
                .show(ui, |ui| {
                    for (_, name, from, to) in &o.layers {
                        ui.label(name);
                        ui.weak(from.name());
                        ui.label("→");
                        ui.strong(to.name());
                        ui.end_row();
                    }
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let n = o.layers.len();
                let text = if n == 1 {
                    "Update 1 layer".to_owned()
                } else {
                    format!("Update {n} layers")
                };
                if ui.button(text).on_hover_text("One undo step").clicked() {
                    accept = true;
                }
                if ui.button("Keep as they are").clicked() {
                    dismiss = true;
                }
            });
        });
    if accept {
        app.accept_profile_offer();
    } else if dismiss || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.view.profile_offer = None;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use locus_doc::Operation;

    fn find(id: &str) -> MachineProfile {
        locus_export::profile::find(id, &[]).unwrap()
    }

    #[test]
    fn presets_say_what_machine_they_drive() {
        assert_eq!(find("lightburn").machine, MachineKind::Laser);
        assert_eq!(find("cricut").machine, MachineKind::Cutter);
        assert_eq!(find("hpgl").machine, MachineKind::Plotter);
        assert_eq!(find("generic-svg").machine, MachineKind::Generic);
        // New layers: laser Cut, plotter Draw, vinyl cutter Drag knife.
        assert!(matches!(
            new_layer_operation(Some(&find("lightburn"))),
            Operation::Cut(_)
        ));
        assert!(matches!(
            new_layer_operation(Some(&find("hpgl"))),
            Operation::Draw
        ));
        assert!(matches!(
            new_layer_operation(Some(&find("cricut"))),
            Operation::DragKnife(_)
        ));
        assert!(matches!(new_layer_operation(None), Operation::Draw));
    }

    #[test]
    fn pickers_offer_the_machines_operations_and_keep_the_current_one() {
        let laser = find("lightburn");
        let ops = offered(Some(&laser), OperationKind::Cut);
        assert!(!ops.contains(&OperationKind::Draw));
        assert!(!ops.contains(&OperationKind::DragKnife));
        // A Draw layer on a laser still shows its own operation.
        assert!(offered(Some(&laser), OperationKind::Draw).contains(&OperationKind::Draw));
        assert_eq!(
            offered(None, OperationKind::Draw),
            OperationKind::ALL.to_vec()
        );
    }

    #[test]
    fn choosing_a_profile_is_one_step_and_only_offers_to_update_layers() {
        let settings = Settings::default();
        let mut app = AppState::untitled();
        let layer = app.active_layer();
        assert!(matches!(
            app.document().layer(layer).unwrap().operation,
            Operation::Draw
        ));
        assert!(app.choose_profile(Some("lightburn".into()), &settings));
        assert_eq!(app.document().profile(), Some("lightburn"));
        // Never silently rewritten (#419).
        assert!(matches!(
            app.document().layer(layer).unwrap().operation,
            Operation::Draw
        ));
        let o = app.view.profile_offer.clone().unwrap();
        assert_eq!(o.layers.len(), 1);
        assert_eq!(o.layers[0].3, OperationKind::Cut);
        assert!(app.accept_profile_offer());
        assert!(matches!(
            app.document().layer(layer).unwrap().operation,
            Operation::Cut(_)
        ));
        assert!(app.view.profile_offer.is_none());
        // Two steps: the update, then the profile.
        assert_eq!(
            app.undo().as_deref(),
            Some("Update layers for LightBurn (SVG)")
        );
        assert_eq!(app.document().profile(), Some("lightburn"));
        assert_eq!(app.undo().as_deref(), Some("Set machine profile"));
        assert_eq!(app.document().profile(), None);
    }

    #[test]
    fn a_through_cut_becomes_the_other_machines_through_cut() {
        assert_eq!(
            counterpart(OperationKind::Cut, MachineKind::Cutter),
            OperationKind::DragKnife
        );
        assert_eq!(
            counterpart(OperationKind::DragKnife, MachineKind::Laser),
            OperationKind::Cut
        );
        assert_eq!(
            counterpart(OperationKind::Cut, MachineKind::Plotter),
            OperationKind::Draw
        );
        assert_eq!(
            counterpart(OperationKind::Reference, MachineKind::Laser),
            OperationKind::Reference
        );
    }

    #[test]
    fn layers_that_fit_get_no_offer() {
        let mut app = AppState::untitled();
        assert!(app.choose_profile(Some("generic-svg".into()), &Settings::default()));
        assert!(app.view.profile_offer.is_none());
        // Choosing the same profile again is not another undo step.
        assert!(app.choose_profile(Some("generic-svg".into()), &Settings::default()));
        assert_eq!(app.undo().as_deref(), Some("Set machine profile"));
        assert_eq!(app.undo(), None);
    }
}
