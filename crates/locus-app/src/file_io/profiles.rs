// SPDX-License-Identifier: AGPL-3.0-or-later
//! Machine profiles (SPEC §7, #236, #238, #286): the list of choices,
//! importing a single profile `.json`, and the profile manager dialog
//! (M5 design §6.4).
//!
//! User profiles are JSON in app settings (owner decision; on the web that
//! is the browser storage eframe uses). Built-in presets are read-only but
//! can be duplicated. Imported files and stored settings are untrusted: the
//! model validates and bounds them, and a user profile may not reuse a
//! preset's id, so a preset can never be shadowed. A profile's id never
//! changes once made, because documents store it (§10).

use crate::settings::{MAX_USER_PROFILES, Settings};
use locus_doc::OperationKind;
use locus_export::MachineProfile;
use locus_export::profile::{self, Bed, Origin, ProfileUnits, is_builtin};

/// Presets first, then the user's own, in settings order.
pub(super) fn all(settings: &Settings) -> Vec<MachineProfile> {
    let mut v = profile::presets();
    v.extend(settings.profiles.iter().cloned());
    v
}

/// Add (or replace, by id) an imported profile. Returns its id.
///
/// # Errors
/// A message safe to show: invalid file, preset id clash or too many profiles.
pub(super) fn import(text: &str, settings: &mut Settings) -> Result<String, String> {
    let p = MachineProfile::from_json(text)
        .map_err(|e| format!("Could not import the profile: {e}."))?;
    if is_builtin(&p.id) {
        return Err(
            "That profile uses the id of a built-in preset; change its id first.".to_owned(),
        );
    }
    let id = p.id.clone();
    match settings.profiles.iter().position(|q| q.id == id) {
        Some(i) => settings.profiles[i] = p,
        None if settings.profiles.len() >= MAX_USER_PROFILES => {
            return Err("Too many user profiles.".to_owned());
        }
        None => settings.profiles.push(p),
    }
    Ok(id)
}

/// A profile id from a display name: lower-case `a-z 0-9 -`, never empty,
/// at most 48 bytes (ids allow 64; room for a `-2` suffix).
fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
        if s.len() >= 48 {
            break;
        }
    }
    let s = s.trim_end_matches('-').to_owned();
    if s.is_empty() {
        "profile".to_owned()
    } else {
        s
    }
}

/// An id based on `name` that no preset or user profile has.
fn unique_id(name: &str, settings: &Settings) -> String {
    let base = slug(name);
    let taken = |id: &str| is_builtin(id) || settings.profiles.iter().any(|p| p.id == id);
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken(id))
        .unwrap_or(base)
}

/// A name no other profile shows: "X copy", "X copy 2", …
fn unique_name(name: &str, settings: &Settings) -> String {
    let taken = |n: &str| all(settings).iter().any(|p| p.name == n);
    let stem: String = name.chars().take(profile::MAX_NAME_CHARS - 8).collect();
    let first = format!("{stem} copy");
    if !taken(&first) {
        return first;
    }
    (2..)
        .map(|n| format!("{stem} copy {n}"))
        .find(|n| !taken(n))
        .unwrap_or(first)
}

/// A new user profile copied from `src` (a preset or a user profile).
///
/// # Errors
/// When the user already has [`MAX_USER_PROFILES`].
fn duplicate(src: &MachineProfile, settings: &mut Settings) -> Result<String, String> {
    if settings.profiles.len() >= MAX_USER_PROFILES {
        return Err("Too many user profiles.".to_owned());
    }
    let mut p = src.clone();
    p.name = unique_name(&src.name, settings);
    p.id = unique_id(&p.name, settings);
    let id = p.id.clone();
    settings.profiles.push(p);
    Ok(id)
}

/// A blank user profile: SVG, millimetres, no bed.
fn new_profile(settings: &mut Settings) -> Result<String, String> {
    let blank = MachineProfile {
        id: String::new(),
        name: "My machine".to_owned(),
        format: "svg".to_owned(),
        units: ProfileUnits::Mm,
        colours: Default::default(),
        bed: None,
        defaults: Default::default(),
        hairlines: Default::default(),
    };
    let id = duplicate(&blank, settings)?;
    // "My machine copy" reads oddly for the first new one.
    let free = !all(settings).iter().any(|p| p.name == blank.name);
    if free && let Some(p) = settings.profiles.iter_mut().find(|p| p.id == id) {
        p.name.clone_from(&blank.name);
    }
    Ok(id)
}

/// Store an edited user profile. The id is the one it was made with.
///
/// # Errors
/// The validation message, or that the profile is a preset or gone.
fn store(draft: &MachineProfile, settings: &mut Settings) -> Result<(), String> {
    if is_builtin(&draft.id) {
        return Err("Built-in presets are read-only; duplicate it to edit.".to_owned());
    }
    draft
        .validate()
        .map_err(|e| format!("Can't save the profile: {e}."))?;
    match settings.profiles.iter_mut().find(|p| p.id == draft.id) {
        Some(p) => {
            *p = draft.clone();
            Ok(())
        }
        None => Err("That profile no longer exists.".to_owned()),
    }
}

/// What the manager asks its owner to do (file dialogs live in `FileIo`).
#[derive(Debug, PartialEq)]
pub(super) enum Request {
    /// Pick a `.json` to import.
    Import,
    /// Save this profile as a `.json`.
    Export(MachineProfile),
    /// A profile changed or went away: refresh anything showing it.
    Changed(String),
}

/// The profile manager dialog's state (design §6.4).
#[derive(Debug, Default)]
pub(super) struct Manager {
    open: bool,
    selected: Option<String>,
    /// The edits to the selected user profile, not yet saved.
    draft: Option<MachineProfile>,
    /// Waiting for "Delete" to be confirmed.
    confirm_delete: bool,
    message: Option<String>,
}

impl Manager {
    pub(super) fn open(&mut self, select: Option<&str>) {
        self.open = true;
        if let Some(id) = select {
            self.select(id.to_owned());
        }
    }

    fn select(&mut self, id: String) {
        self.selected = Some(id);
        self.draft = None;
        self.confirm_delete = false;
    }

    /// Imported from a file: select it.
    pub(super) fn imported(&mut self, id: String) {
        self.message = Some("Profile imported.".to_owned());
        self.select(id);
    }

    pub(super) fn error(&mut self, msg: String) {
        self.message = Some(msg);
    }

    /// Draw the dialog. Returns what the owner should do.
    pub(super) fn window(
        &mut self,
        ctx: &egui::Context,
        settings: &mut Settings,
        exporters: &locus_export::Registry,
    ) -> Vec<Request> {
        let mut out = Vec::new();
        if !self.open {
            return out;
        }
        let mut open = true;
        crate::dialog::Dialog::new("Machine profiles")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([640.0, 460.0])
            .show(ctx, |ui| {
                if let Some(m) = &self.message {
                    ui.colored_label(ui.visuals().warn_fg_color, m);
                }
                ui.columns(2, |cols| {
                    self.list(&mut cols[0], settings, &mut out);
                    egui::ScrollArea::vertical().id_salt("profile-editor").show(
                        &mut cols[1],
                        |ui| {
                            self.editor(ui, settings, exporters, &mut out);
                        },
                    );
                });
            });
        if !open {
            self.open = false;
            self.draft = None;
            self.message = None;
        }
        out
    }

    /// Left: user profiles, then presets (locked), and the actions.
    fn list(&mut self, ui: &mut egui::Ui, settings: &mut Settings, out: &mut Vec<Request>) {
        ui.strong("Your profiles");
        if settings.profiles.is_empty() {
            ui.weak("None yet. Duplicate a preset or start a new one.");
        }
        let mut pick = None;
        for p in &settings.profiles {
            let on = self.selected.as_deref() == Some(p.id.as_str());
            if ui.selectable_label(on, &p.name).clicked() {
                pick = Some(p.id.clone());
            }
        }
        ui.add_space(6.0);
        ui.strong("Built-in presets");
        for p in profile::presets() {
            let on = self.selected.as_deref() == Some(p.id.as_str());
            if ui
                .selectable_label(on, format!("{} (read-only)", p.name))
                .clicked()
            {
                pick = Some(p.id);
            }
        }
        if let Some(id) = pick {
            self.select(id);
        }
        ui.separator();
        let sel = self
            .selected
            .as_deref()
            .and_then(|id| profile::find(id, &settings.profiles));
        let user = sel.as_ref().is_some_and(|p| !is_builtin(&p.id));
        ui.horizontal_wrapped(|ui| {
            if ui.button("New").clicked() {
                match new_profile(settings) {
                    Ok(id) => self.select(id),
                    Err(e) => self.message = Some(e),
                }
            }
            if ui
                .add_enabled(sel.is_some(), egui::Button::new("Duplicate"))
                .clicked()
                && let Some(src) = &sel
            {
                match duplicate(src, settings) {
                    Ok(id) => self.select(id),
                    Err(e) => self.message = Some(e),
                }
            }
            if ui.add_enabled(user, egui::Button::new("Delete")).clicked() {
                self.confirm_delete = true;
            }
            if ui.button("Import .json…").clicked() {
                out.push(Request::Import);
            }
            if ui
                .add_enabled(sel.is_some(), egui::Button::new("Export .json…"))
                .clicked()
                && let Some(p) = &sel
            {
                // Export what is saved, not unsaved edits.
                out.push(Request::Export(p.clone()));
            }
        });
        if self.confirm_delete
            && let Some(p) = sel.filter(|p| !is_builtin(&p.id))
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "Delete \u{201c}{}\u{201d}? This can't be undone.",
                    p.name
                ));
                if ui.button("Delete").clicked() {
                    settings.profiles.retain(|q| q.id != p.id);
                    out.push(Request::Changed(p.id.clone()));
                    self.selected = None;
                    self.draft = None;
                    self.confirm_delete = false;
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_delete = false;
                }
            });
        }
    }

    /// Right: the selected profile's fields. Presets show read-only.
    fn editor(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut Settings,
        exporters: &locus_export::Registry,
        out: &mut Vec<Request>,
    ) {
        let Some(id) = self.selected.clone() else {
            ui.weak("Select a profile.");
            return;
        };
        let Some(saved) = profile::find(&id, &settings.profiles) else {
            self.selected = None;
            return;
        };
        let builtin = is_builtin(&id);
        if builtin {
            ui.horizontal(|ui| {
                ui.weak("Built-in preset: read-only.");
                if ui.button("Duplicate to edit").clicked() {
                    match duplicate(&saved, settings) {
                        Ok(id) => self.select(id),
                        Err(e) => self.message = Some(e),
                    }
                }
            });
        }
        let mut p = self
            .draft
            .clone()
            .filter(|d| d.id == id)
            .unwrap_or_else(|| saved.clone());
        ui.add_enabled_ui(!builtin, |ui| fields(ui, &mut p, exporters));
        let dirty = p != saved;
        if !builtin {
            ui.separator();
            let check = p.validate();
            if let Err(e) = &check {
                ui.colored_label(ui.visuals().error_fg_color, e.to_string());
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(dirty && check.is_ok(), egui::Button::new("Save"))
                    .clicked()
                {
                    match store(&p, settings) {
                        Ok(()) => {
                            out.push(Request::Changed(p.id.clone()));
                            self.message = None;
                        }
                        Err(e) => self.message = Some(e),
                    }
                }
                if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                    p = saved.clone();
                }
            });
            self.draft = Some(p);
        }
    }
}

/// Lengths are shown in the profile's own units (SPEC §4: stored as mm).
fn len_edit(ui: &mut egui::Ui, mm: &mut f64, units: ProfileUnits, max_mm: f64) {
    let (k, suffix, speed) = match units {
        ProfileUnits::Mm => (1.0, " mm", 0.1),
        ProfileUnits::Inch => (25.4, " in", 0.005),
    };
    let mut v = *mm / k;
    if ui
        .add(
            egui::DragValue::new(&mut v)
                .speed(speed)
                .range(0.0..=max_mm / k)
                .suffix(suffix)
                .max_decimals(4),
        )
        .changed()
    {
        *mm = v * k;
    }
}

/// An optional length: a checkbox, then the value.
fn opt_len(ui: &mut egui::Ui, label: &str, v: &mut Option<f64>, default: f64, units: ProfileUnits) {
    ui.horizontal(|ui| {
        let mut on = v.is_some();
        if ui.checkbox(&mut on, label).changed() {
            *v = on.then_some(default);
        }
        if let Some(x) = v {
            len_edit(ui, x, units, profile::MAX_DEFAULT_MM);
        }
    });
}

fn fields(ui: &mut egui::Ui, p: &mut MachineProfile, exporters: &locus_export::Registry) {
    egui::Grid::new("profile-fields")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Name");
            ui.add(
                egui::TextEdit::singleline(&mut p.name)
                    .char_limit(profile::MAX_NAME_CHARS)
                    .desired_width(220.0),
            );
            ui.end_row();
            ui.label("Id");
            ui.weak(&p.id)
                .on_hover_text("Documents remember a profile by its id, so it never changes.");
            ui.end_row();
            ui.label("Format");
            let installed = exporters.get(&p.format).map(|e| e.name().to_owned());
            egui::ComboBox::from_id_salt("profile-format")
                .selected_text(installed.unwrap_or_else(|| format!("{} (not installed)", p.format)))
                .show_ui(ui, |ui| {
                    for e in exporters.iter() {
                        ui.selectable_value(&mut p.format, e.id().to_owned(), e.name());
                    }
                });
            ui.end_row();
            ui.label("Units");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut p.units, ProfileUnits::Mm, "mm");
                ui.selectable_value(&mut p.units, ProfileUnits::Inch, "in");
            });
            ui.end_row();
        });
    ui.separator();
    let mut has_bed = p.bed.is_some();
    if ui.checkbox(&mut has_bed, "Bed").changed() {
        p.bed = has_bed.then_some(Bed {
            width_mm: 400.0,
            height_mm: 400.0,
            origin: Origin::TopLeft,
        });
    }
    if let Some(bed) = &mut p.bed {
        ui.horizontal(|ui| {
            ui.label("Size");
            len_edit(ui, &mut bed.width_mm, p.units, profile::MAX_BED_MM);
            ui.label("×");
            len_edit(ui, &mut bed.height_mm, p.units, profile::MAX_BED_MM);
        });
        ui.horizontal(|ui| {
            ui.label("Origin");
            // A 2 × 2 picker laid out as the bed is seen (design §6.4).
            egui::Grid::new("profile-origin").show(ui, |ui| {
                for row in [
                    [Origin::TopLeft, Origin::TopRight],
                    [Origin::BottomLeft, Origin::BottomRight],
                ] {
                    for o in row {
                        ui.selectable_value(&mut bed.origin, o, o.name())
                            .on_hover_text("The machine's zero, looking at the bed");
                    }
                    ui.end_row();
                }
            });
        });
    }
    ui.separator();
    ui.strong("Operations");
    egui::Grid::new("profile-ops")
        .num_columns(3)
        .striped(true)
        .show(ui, |ui| {
            ui.weak("");
            ui.weak("Colour");
            ui.weak("Hairline");
            ui.end_row();
            for op in OperationKind::ALL {
                if op == OperationKind::Reference {
                    continue; // never exported
                }
                ui.label(op.name());
                ui.horizontal(|ui| {
                    let mut on = p.colours.contains_key(&op);
                    if ui
                        .checkbox(&mut on, "")
                        .on_hover_text("Unticked: the layer keeps its own colour")
                        .changed()
                    {
                        if on {
                            p.colours.insert(op, [0, 0, 0]);
                        } else {
                            p.colours.remove(&op);
                        }
                    }
                    if let Some(c) = p.colours.get_mut(&op) {
                        ui.color_edit_button_srgb(c);
                    }
                });
                if op == OperationKind::Draw {
                    ui.weak("each object's own");
                } else {
                    let mut v = p.hairlines.get(&op).copied();
                    opt_len(ui, "", &mut v, 0.01, p.units);
                    match v {
                        Some(w) => p.hairlines.insert(op, w),
                        None => p.hairlines.remove(&op),
                    };
                }
                ui.end_row();
            }
        });
    ui.separator();
    ui.strong("Defaults for new layers");
    opt_len(ui, "Kerf", &mut p.defaults.kerf_mm, 0.1, p.units);
    opt_len(
        ui,
        "Blade offset",
        &mut p.defaults.blade_offset_mm,
        0.25,
        p.units,
    );
    opt_len(ui, "Pen width", &mut p.defaults.pen_width_mm, 0.3, p.units);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn import_adds_replaces_and_refuses_presets() {
        let mut s = Settings::default();
        let mut p = profile::find("generic-svg", &[]).unwrap();
        // A preset id is refused.
        assert!(import(&p.to_json().unwrap(), &mut s).is_err());
        p.id = "mine".into();
        assert_eq!(import(&p.to_json().unwrap(), &mut s).unwrap(), "mine");
        p.name = "Renamed".into();
        import(&p.to_json().unwrap(), &mut s).unwrap();
        assert_eq!(s.profiles.len(), 1);
        assert_eq!(s.profiles[0].name, "Renamed");
        assert_eq!(all(&s).last().unwrap().id, "mine");
        // Garbage never panics.
        assert!(import("{\"id\": 1", &mut s).is_err());
    }

    #[test]
    fn slugs_are_valid_ids() {
        for (name, want) in [
            ("LightBurn — Shop Laser!", "lightburn-shop-laser"),
            ("  ", "profile"),
            ("Ünïcode 12\"", "n-code-12"),
        ] {
            assert_eq!(slug(name), want);
            assert!(locus_doc::check_profile_id(&slug(name)).is_ok());
        }
        assert!(slug(&"x".repeat(500)).len() <= 48);
    }

    #[test]
    fn duplicating_a_preset_makes_an_editable_copy() {
        let mut s = Settings::default();
        let src = profile::presets()[0].clone();
        let id = duplicate(&src, &mut s).unwrap();
        assert!(!is_builtin(&id));
        let copy = s.profiles.iter().find(|p| p.id == id).unwrap();
        assert_eq!(copy.name, format!("{} copy", src.name));
        assert_eq!(copy.colours, src.colours);
        assert!(copy.validate().is_ok());
        // Again: a fresh id and name.
        let id2 = duplicate(&src, &mut s).unwrap();
        assert_ne!(id, id2);
        assert_eq!(s.profiles[1].name, format!("{} copy 2", src.name));
    }

    #[test]
    fn new_profiles_are_valid_and_unique() {
        let mut s = Settings::default();
        let a = new_profile(&mut s).unwrap();
        let b = new_profile(&mut s).unwrap();
        assert_ne!(a, b);
        assert!(s.profiles.iter().all(|p| p.validate().is_ok()));
        assert_eq!(s.profiles[0].name, "My machine");
    }

    #[test]
    fn store_validates_and_refuses_presets() {
        let mut s = Settings::default();
        let id = new_profile(&mut s).unwrap();
        let mut p = s.profiles[0].clone();
        p.bed = Some(Bed {
            width_mm: 600.0,
            height_mm: 300.0,
            origin: Origin::BottomRight,
        });
        store(&p, &mut s).unwrap();
        assert_eq!(s.profiles[0].bed, p.bed);
        // Invalid: refused, stored copy unchanged.
        let mut bad = p.clone();
        bad.name = String::new();
        assert!(store(&bad, &mut s).is_err());
        assert_eq!(s.profiles[0].name, "My machine");
        // A preset can't be overwritten, nor a deleted one resurrected.
        assert!(store(&profile::presets()[0], &mut s).is_err());
        s.profiles.retain(|q| q.id != id);
        assert!(store(&p, &mut s).is_err());
    }

    #[test]
    fn duplicating_stops_at_the_limit() {
        let mut s = Settings::default();
        for _ in 0..MAX_USER_PROFILES {
            new_profile(&mut s).unwrap();
        }
        assert!(new_profile(&mut s).is_err());
        assert!(duplicate(&profile::presets()[0], &mut s).is_err());
    }
}
