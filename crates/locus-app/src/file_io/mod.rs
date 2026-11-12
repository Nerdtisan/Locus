// SPDX-License-Identifier: AGPL-3.0-or-later

//! File handling for the GUI shell: Open / Save / Save As / Export (SVG, PDF, DXF via the pre-export panel), the
//! unsaved-changes guard and recent files (SPEC §10, M1 #74).
//!
//! The document is only ever replaced through [`Canvas::load`], which starts
//! a fresh command history (SPEC §11). Error messages shown to the user never
//! contain file names or document contents, and nothing here logs either
//! (CLAUDE.md, Secure Coding).

mod profiles;
#[cfg(target_arch = "wasm32")]
mod web;

use crate::canvas::Canvas;
use crate::settings::Settings;
use locus_doc::{Command, Document, EntityId};
use locus_export::MachineProfile;
use locus_export::{Prepared, Severity};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::path::PathBuf;

/// One finding from an extra pre-export check: a validator plugin (#234).
#[derive(Debug, Clone, PartialEq)]
pub struct ExtraFinding {
    pub error: bool,
    pub message: String,
    pub entities: Vec<EntityId>,
}

/// Extra pre-export checks, run after the built-in §8 ones and shown next to
/// them. They can't hide the built-in findings: those are computed first and
/// separately.
#[derive(Clone)]
pub struct ExtraChecks(pub std::sync::Arc<CheckFn>);

/// One extra check: export model in, findings out.
pub type CheckFn = dyn Fn(&locus_export::ExportModel) -> Vec<ExtraFinding> + Send + Sync;

impl std::fmt::Debug for ExtraChecks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExtraChecks")
    }
}

const RECENT_KEY: &str = "recent_files";
const MAX_RECENT: usize = 10;

/// Something that replaces the open document, so it needs the
/// unsaved-changes guard first.
#[derive(Debug)]
enum Action {
    New,
    OpenDialog,
    #[cfg(not(target_arch = "wasm32"))]
    Open(PathBuf),
    /// A built-in sample.
    Replace(Box<Document>),
    /// Close the window (desktop only; the browser tab has no close request).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Quit,
}

#[derive(Debug)]
pub struct FileIo {
    /// Where the open document was loaded from / last saved to (desktop).
    path: Option<PathBuf>,
    /// Display name of the open document (file name; no directory).
    name: Option<String>,
    /// The first frame adopts the canvas's starting document as the baseline.
    started: bool,
    /// The document as last saved or loaded; the document is dirty when it differs.
    baseline: Document,
    recent: Vec<PathBuf>,
    error: Option<String>,
    notice: Option<String>,
    pending: Option<Action>,
    /// The open pre-export panel (SPEC §8 Validation, #128).
    export: Option<ExportPanel>,
    exporters: locus_export::Registry,
    checks: Option<ExtraChecks>,
    allow_close: bool,
    last_title: String,
    #[cfg(target_arch = "wasm32")]
    web: web::Web,
}

impl FileIo {
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
    pub fn new(ctx: &egui::Context, storage: Option<&dyn eframe::Storage>) -> Self {
        let recent = storage
            .and_then(|s| s.get_string(RECENT_KEY))
            .and_then(|j| serde_json::from_str::<Vec<PathBuf>>(&j).ok())
            .unwrap_or_default();
        Self {
            path: None,
            name: None,
            started: false,
            baseline: Document::new(),
            recent: recent.into_iter().take(MAX_RECENT).collect(),
            error: None,
            notice: None,
            pending: None,
            export: None,
            exporters: locus_export::Registry::builtin(),
            checks: None,
            allow_close: false,
            last_title: String::new(),
            #[cfg(target_arch = "wasm32")]
            web: web::Web::new(ctx),
        }
    }

    /// Replace the export formats, e.g. the built-ins plus installed
    /// exporter plugins (#184).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_exporters(&mut self, exporters: locus_export::Registry) {
        self.exporters = exporters;
    }

    /// Replace the extra pre-export checks (validator plugins, #234).
    pub fn set_checks(&mut self, checks: Option<ExtraChecks>) {
        self.checks = checks;
    }

    fn extra(&self, prepared: &Prepared) -> Vec<ExtraFinding> {
        self.checks
            .as_ref()
            .map(|c| (c.0)(&prepared.model))
            .unwrap_or_default()
    }

    pub fn save_state(&self, storage: &mut dyn eframe::Storage) {
        if let Ok(json) = serde_json::to_string(&self.recent) {
            storage.set_string(RECENT_KEY, json);
        }
    }

    fn dirty(&self, canvas: &Canvas) -> bool {
        canvas.document() != &self.baseline
    }

    /// Opens a `.locus` file named on the command line (a file-association
    /// double-click, packaging #245). Nothing is open yet, so no guard is
    /// needed; a bad file shows the usual error dialog.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_startup_path(&mut self, canvas: &mut Canvas, path: PathBuf) {
        self.open_path(canvas, path);
    }

    /// Replace the open document with a built-in sample (guarded).
    pub fn open_sample(&mut self, ctx: &egui::Context, canvas: &mut Canvas, doc: Document) {
        self.request(ctx, canvas, Action::Replace(Box::new(doc)));
    }

    /// Per-frame work: shortcuts, the close guard, window title and dialogs.
    pub fn update(&mut self, ctx: &egui::Context, canvas: &mut Canvas, settings: &mut Settings) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        if !self.started {
            self.started = true;
            self.baseline = canvas.document().clone();
        }
        // Shift+S is consumed first: egui lets Ctrl+S match Ctrl+Shift+S too.
        let (open, save_as, save) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&open_shortcut()),
                i.consume_shortcut(&KeyboardShortcut::new(
                    Modifiers::COMMAND | Modifiers::SHIFT,
                    Key::S,
                )),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::S)),
            )
        });
        if open {
            self.request(ctx, canvas, Action::OpenDialog);
        }
        if save_as {
            self.save(canvas, true);
        } else if save {
            self.save(canvas, false);
        }

        #[cfg(not(target_arch = "wasm32"))]
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.dirty(canvas)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Action::Quit);
        }

        #[cfg(target_arch = "wasm32")]
        self.web_update(ctx, canvas);

        self.sync_title(ctx, canvas);
        self.dialogs(ctx, canvas, settings);
    }

    fn sync_title(&mut self, ctx: &egui::Context, canvas: &Canvas) {
        let name = self.name.as_deref().unwrap_or("Untitled");
        let title = format!(
            "{}{name} — Locus",
            if self.dirty(canvas) { "● " } else { "" }
        );
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    /// The File menu entries up to (not including) Samples / Quit.
    pub fn file_menu(&mut self, ui: &mut egui::Ui, canvas: &mut Canvas, settings: &Settings) {
        let ctx = ui.ctx().clone();
        if ui.button("New").clicked() {
            self.request(&ctx, canvas, Action::New);
            ui.close();
        }
        if ui
            .add(egui::Button::new("Open…").shortcut_text(ctx.format_shortcut(&open_shortcut())))
            .clicked()
        {
            self.request(&ctx, canvas, Action::OpenDialog);
            ui.close();
        }
        #[cfg(not(target_arch = "wasm32"))]
        ui.add_enabled_ui(!self.recent.is_empty(), |ui| {
            ui.menu_button("Open recent", |ui| {
                for path in self.recent.clone() {
                    let label = path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    if ui
                        .button(label)
                        .on_hover_text(path.to_string_lossy())
                        .clicked()
                    {
                        self.request(&ctx, canvas, Action::Open(path));
                        ui.close();
                    }
                }
            });
        });
        ui.separator();
        if ui.button("Save").clicked() {
            self.save(canvas, false);
            ui.close();
        }
        if ui.button("Save As…").clicked() {
            self.save(canvas, true);
            ui.close();
        }
        ui.menu_button("Export", |ui| {
            if ui
                .button("For a machine…")
                .on_hover_text(
                    "Pick a machine profile (LightBurn, Cricut, Silhouette, …) and export with its settings",
                )
                .clicked()
            {
                self.begin_profile_export(canvas, settings);
                ui.close();
            }
            ui.separator();
            let formats: Vec<(String, String)> = self
                .exporters
                .iter()
                .map(|e| (e.id().to_owned(), e.name().to_owned()))
                .collect();
            for (id, name) in formats {
                if ui.button(format!("{name}…")).clicked() {
                    self.begin_export(&id, canvas);
                    ui.close();
                }
            }
        });
        #[cfg(target_arch = "wasm32")]
        {
            ui.separator();
            ui.weak("Autosaved in this browser only").on_hover_text(
                "Your sketch is autosaved to this browser's storage and restored on \
                 reload. It isn't shared with other browsers or devices — use Save to \
                 keep a copy.",
            );
        }
    }

    // ── Actions ──────────────────────────────────────────────────────────

    fn request(&mut self, ctx: &egui::Context, canvas: &mut Canvas, action: Action) {
        if self.dirty(canvas) {
            self.pending = Some(action);
        } else {
            self.perform(ctx, canvas, action);
        }
    }

    fn perform(&mut self, ctx: &egui::Context, canvas: &mut Canvas, action: Action) {
        match action {
            Action::New => self.replace(canvas, Document::new(), None, None),
            Action::Replace(doc) => self.replace(canvas, *doc, None, None),
            Action::OpenDialog => self.begin_open(canvas),
            #[cfg(not(target_arch = "wasm32"))]
            Action::Open(path) => self.open_path(canvas, path),
            Action::Quit => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn replace(
        &mut self,
        canvas: &mut Canvas,
        doc: Document,
        path: Option<PathBuf>,
        name: Option<String>,
    ) {
        canvas.load(doc);
        self.baseline = canvas.document().clone();
        self.path = path;
        self.name = name;
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn begin_open(&mut self, canvas: &mut Canvas) {
        if let Some(path) = pick_open() {
            self.open_path(canvas, path);
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn begin_open(&mut self, _canvas: &mut Canvas) {
        self.web.pick();
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn open_path(&mut self, canvas: &mut Canvas, path: PathBuf) {
        match read_document(&path) {
            Ok(doc) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                self.replace(canvas, doc, Some(path.clone()), name);
                self.remember(path);
            }
            Err(msg) => {
                self.recent.retain(|p| p != &path);
                self.error = Some(msg);
            }
        }
    }

    /// Browser: save is a download (the browser decides where it goes).
    #[cfg(target_arch = "wasm32")]
    fn save(&mut self, canvas: &Canvas, _force_dialog: bool) -> bool {
        let text = match locus_doc::file::save(canvas.document()) {
            Ok(t) => t,
            Err(e) => {
                self.error = Some(format!("Could not save: {e}."));
                return false;
            }
        };
        let name = self
            .name
            .clone()
            .unwrap_or_else(|| "untitled.locus".to_owned());
        web::Web::download(&name, text.as_bytes(), "application/octet-stream");
        self.name = Some(name);
        self.baseline = canvas.document().clone();
        true
    }

    /// Save to the current path, or ask for one. Returns whether it saved.
    #[cfg(not(target_arch = "wasm32"))]
    fn save(&mut self, canvas: &Canvas, force_dialog: bool) -> bool {
        let path = match (&self.path, force_dialog) {
            (Some(p), false) => p.clone(),
            _ => {
                let name = self.name.as_deref().unwrap_or("untitled.locus");
                let Some(p) = pick_save(name, "locus", "Locus document") else {
                    return false;
                };
                p
            }
        };
        match write_document(&path, canvas.document()) {
            Ok(()) => {
                self.baseline = canvas.document().clone();
                self.name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                self.path = Some(path.clone());
                self.remember(path);
                true
            }
            Err(msg) => {
                self.error = Some(msg);
                false
            }
        }
    }

    /// Validate, then either write straight away (nothing to report) or open
    /// the pre-export panel. Export always goes through validation (#128, #133).
    fn begin_export(&mut self, format: &str, canvas: &Canvas) {
        let prepared = locus_export::prepare(canvas.document(), false);
        let extra = self.extra(&prepared);
        if prepared.report.issues.is_empty() && extra.is_empty() {
            self.write_export(format, &prepared);
        } else {
            self.export = Some(ExportPanel {
                format: format.to_owned(),
                fixed: false,
                prepared,
                profile: None,
                picker: false,
                profile_changed: false,
                extra,
            });
        }
    }

    /// Open the export panel with the machine-profile picker (SPEC §7, #238).
    /// The document's remembered profile is preselected; otherwise generic SVG.
    fn begin_profile_export(&mut self, canvas: &Canvas, settings: &Settings) {
        let profile = canvas
            .document()
            .profile()
            .and_then(|id| locus_export::profile::find(id, &settings.profiles))
            .or_else(|| locus_export::profile::find("generic-svg", &[]));
        let prepared = locus_export::prepare_for(canvas.document(), false, profile.as_ref());
        let extra = self.extra(&prepared);
        self.export = Some(ExportPanel {
            format: profile
                .as_ref()
                .map_or_else(|| "svg".to_owned(), |p| p.format.clone()),
            fixed: false,
            prepared,
            profile,
            picker: true,
            profile_changed: false,
            extra,
        });
    }

    #[cfg(target_arch = "wasm32")]
    fn write_export(&mut self, format: &str, prepared: &Prepared) {
        let Some(done) = self.run_exporter(format, prepared) else {
            return;
        };
        let stem = self.name.as_deref().map_or("untitled", strip_ext);
        web::Web::download(
            &format!("{stem}.{}", done.extension),
            &done.bytes,
            &done.mime,
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn write_export(&mut self, format: &str, prepared: &Prepared) {
        let Some(done) = self.run_exporter(format, prepared) else {
            return;
        };
        let stem = self.name.as_deref().map_or("untitled", strip_ext);
        let filter = format!("{} file", format.to_uppercase());
        let Some(path) = pick_save(
            &format!("{stem}.{}", done.extension),
            &done.extension,
            &filter,
        ) else {
            return;
        };
        if let Err(msg) = write_atomic(&path, &done.bytes) {
            self.error = Some(msg);
        }
    }

    /// Run the registered exporter; a failure is shown to the user, never a crash.
    fn run_exporter(
        &mut self,
        format: &str,
        prepared: &Prepared,
    ) -> Option<locus_export::Exported> {
        let result = self
            .exporters
            .get(format)
            .ok_or_else(|| "That export format is not available.".to_owned())
            .and_then(|e| {
                e.export(&prepared.model)
                    .map_err(|e| format!("Export failed: {e}."))
            });
        match result {
            Ok(done) => Some(done),
            Err(msg) => {
                self.error = Some(msg);
                None
            }
        }
    }

    /// The pre-export panel: fixes applied, errors (which block export) and
    /// warnings, each selectable to highlight its entities on the canvas.
    fn export_panel(&mut self, ctx: &egui::Context, canvas: &mut Canvas, settings: &mut Settings) {
        let checks = self.checks.clone();
        let Some(panel) = self.export.as_mut() else {
            return;
        };
        let extra_for = |p: &Prepared| checks.as_ref().map(|c| (c.0)(&p.model)).unwrap_or_default();
        let blocked =
            |p: &Prepared, extra: &[ExtraFinding]| p.blocked() || extra.iter().any(|f| f.error);
        enum Do {
            Fix,
            Export,
            Cancel,
        }
        let mut act = None;
        let mut picked_error: Option<String> = None;
        let mut select: Option<Vec<EntityId>> = None;
        let available = self.exporters.get(&panel.format).is_some();
        let fixable = panel.prepared.report.issues.iter().any(|i| {
            matches!(
                i.kind,
                locus_export::IssueKind::DuplicateSegment | locus_export::IssueKind::Degenerate
            )
        });
        let title = self
            .exporters
            .get(&panel.format)
            .map_or_else(|| "Export".to_owned(), |e| e.name().to_owned());
        egui::Window::new(format!("Export {title}"))
            .collapsible(false)
            .resizable(true)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                if panel.picker {
                    profile_picker(
                        ui,
                        panel,
                        canvas,
                        settings,
                        &self.exporters,
                        &mut picked_error,
                    );
                    // A new profile re-prepared the model: re-run the plugin checks.
                    if panel.profile_changed {
                        panel.profile_changed = false;
                        panel.extra = extra_for(&panel.prepared);
                    }
                }
                let counts = panel.prepared.report.counts();
                let extra_errors = panel.extra.iter().filter(|f| f.error).count();
                ui.label(format!(
                    "{} error(s), {} warning(s)",
                    counts.errors + extra_errors,
                    counts.warnings + panel.extra.len() - extra_errors
                ));
                for f in &panel.prepared.fixes {
                    ui.weak(format!("Fixed: {}", f.message));
                }
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        for issue in &panel.prepared.report.issues {
                            let colour = match issue.severity {
                                Severity::Error => ui.visuals().error_fg_color,
                                Severity::Warning => ui.visuals().warn_fg_color,
                            };
                            let text = egui::RichText::new(&issue.message).color(colour);
                            if ui
                                .selectable_label(false, text)
                                .on_hover_text("Click to highlight on the canvas")
                                .clicked()
                            {
                                select = Some(issue.entities.clone());
                            }
                        }
                        for f in &panel.extra {
                            let colour = if f.error {
                                ui.visuals().error_fg_color
                            } else {
                                ui.visuals().warn_fg_color
                            };
                            let text = egui::RichText::new(&f.message).color(colour);
                            if ui
                                .selectable_label(false, text)
                                .on_hover_text(
                                    "From a validator plugin. Click to highlight on the canvas",
                                )
                                .clicked()
                            {
                                select = Some(f.entities.clone());
                            }
                        }
                    });
                if blocked(&panel.prepared, &panel.extra) {
                    ui.label(
                        "Errors block export. Fix them in the sketch or apply the auto-fixes.",
                    );
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            fixable && !panel.fixed,
                            egui::Button::new("Apply auto-fixes"),
                        )
                        .on_hover_text(
                            "Merge duplicate segments and drop zero-length ones in the \
                             exported geometry. Your sketch is not changed.",
                        )
                        .clicked()
                    {
                        act = Some(Do::Fix);
                    }
                    if ui
                        .add_enabled(
                            available && !blocked(&panel.prepared, &panel.extra),
                            egui::Button::new("Export…"),
                        )
                        .clicked()
                    {
                        act = Some(Do::Export);
                    }
                    if ui.button("Cancel").clicked() {
                        act = Some(Do::Cancel);
                    }
                });
            });
        if let Some(ids) = select {
            canvas.select_entities(&ids);
        }
        match act {
            Some(Do::Fix) => {
                panel.prepared =
                    locus_export::prepare_for(canvas.document(), true, panel.profile.as_ref());
                panel.extra = extra_for(&panel.prepared);
                panel.fixed = true;
            }
            Some(Do::Export) => {
                // Re-prepare: the sketch may have changed while the panel was open.
                let prepared = locus_export::prepare_for(
                    canvas.document(),
                    panel.fixed,
                    panel.profile.as_ref(),
                );
                let extra = extra_for(&prepared);
                let format = panel.format.clone();
                if blocked(&prepared, &extra) {
                    panel.prepared = prepared;
                    panel.extra = extra;
                } else {
                    self.export = None;
                    self.write_export(&format, &prepared);
                }
            }
            Some(Do::Cancel) => self.export = None,
            None => {}
        }
        if picked_error.is_some() {
            self.error = picked_error;
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn remember(&mut self, path: PathBuf) {
        push_recent(&mut self.recent, path);
    }

    /// Browser: pick results, restore and debounced autosave.
    #[cfg(target_arch = "wasm32")]
    fn web_update(&mut self, ctx: &egui::Context, canvas: &mut Canvas) {
        let now = ctx.input(|i| i.time);
        for msg in self.web.poll(now) {
            match msg {
                web::Msg::Picked(Ok(Some((name, text)))) => match locus_doc::file::load(&text) {
                    Ok(doc) => self.replace(canvas, doc, None, Some(name)),
                    Err(e) => self.error = Some(format!("Could not open the file: {e}.")),
                },
                web::Msg::Picked(Ok(None)) => {}
                web::Msg::Picked(Err(msg)) => self.error = Some(msg),
                web::Msg::Restore(text) => {
                    // Only if the user has not started on something else yet.
                    let untouched = !self.dirty(canvas) && self.name.is_none();
                    match text.map(|t| locus_doc::file::load(&t)) {
                        Some(Ok(doc)) if untouched => {
                            canvas.load(doc);
                            // Never downloaded: stays "unsaved" so the guard applies.
                            self.baseline = Document::new();
                            self.web.mark_saved(canvas.document());
                            self.notice = Some(
                                "Restored your autosaved sketch from this browser.".to_owned(),
                            );
                        }
                        // Nothing usable stored, or the user got going first:
                        // the next change replaces the stored copy.
                        _ => self.web.mark_saved(&Document::new()),
                    }
                }
                web::Msg::AutosaveDone(_) => {}
            }
        }
        self.web.tick_autosave(now, canvas.document());
    }

    fn dialogs(&mut self, ctx: &egui::Context, canvas: &mut Canvas, settings: &mut Settings) {
        self.export_panel(ctx, canvas, settings);
        if self.pending.is_some() {
            let mut choice = None;
            egui::Window::new("Unsaved changes")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("Save your changes before continuing?");
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            choice = Some(true);
                        }
                        if ui.button("Don't save").clicked() {
                            choice = Some(false);
                        }
                        if ui.button("Cancel").clicked() {
                            self.pending = None;
                        }
                    });
                });
            if let Some(save_first) = choice
                && let Some(action) = self.pending.take()
                // A cancelled or failed save keeps the user where they are.
                && (!save_first || self.save(canvas, false))
            {
                self.perform(ctx, canvas, action);
            }
        }
        if let Some(msg) = self.notice.clone() {
            egui::Window::new("Restored")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(msg);
                    ui.weak("Autosave is stored in this browser only.");
                    if ui.button("OK").clicked() {
                        self.notice = None;
                    }
                });
        }
        if let Some(msg) = self.error.clone() {
            egui::Window::new("Error")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(msg);
                    if ui.button("OK").clicked() {
                        self.error = None;
                    }
                });
        }
    }
}

/// Open pre-export panel.
#[derive(Debug)]
struct ExportPanel {
    /// Id of the exporter in [`locus_export::Registry`].
    format: String,
    /// Auto-fixes have been applied to `prepared`.
    fixed: bool,
    prepared: Prepared,
    /// The machine profile in force (colour mapping, bed), if any.
    profile: Option<MachineProfile>,
    /// Show the profile picker (the "For a machine…" entry).
    picker: bool,
    /// The picker re-prepared `prepared`; the plugin checks must re-run.
    profile_changed: bool,
    /// Validator plugins' findings on `prepared` (#234).
    extra: Vec<ExtraFinding>,
}

/// The profile combo, import / save buttons and a one-line summary. A change
/// is remembered in the document (one undoable `SetProfile`, SPEC §7, #238).
fn profile_picker(
    ui: &mut egui::Ui,
    panel: &mut ExportPanel,
    canvas: &mut Canvas,
    settings: &mut Settings,
    exporters: &locus_export::Registry,
    error: &mut Option<String>,
) {
    let all = profiles::all(settings);
    let mut chosen: Option<String> = None;
    ui.horizontal(|ui| {
        ui.label("Machine");
        let current = panel.profile.as_ref().map_or("None", |p| p.name.as_str());
        egui::ComboBox::from_id_salt("export-profile")
            .selected_text(current.to_owned())
            .width(320.0)
            .show_ui(ui, |ui| {
                for p in &all {
                    let on = panel.profile.as_ref().is_some_and(|q| q.id == p.id);
                    if ui.selectable_label(on, &p.name).clicked() {
                        chosen = Some(p.id.clone());
                    }
                }
            });
    });
    if let Some(p) = &panel.profile {
        let bed = p.bed.map_or_else(
            || "no bed size".to_owned(),
            |b| {
                format!(
                    "bed {} × {} mm, origin {}",
                    b.width_mm,
                    b.height_mm,
                    b.origin.name().to_lowercase()
                )
            },
        );
        ui.weak(format!("{} — {bed}", p.format.to_uppercase()));
        if exporters.get(&p.format).is_none() {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!(
                    "The `{}` exporter is not installed; install its plugin to export.",
                    p.format
                ),
            );
        }
    }
    ui.horizontal(|ui| {
        if ui.button("Import profile…").clicked() {
            match import_profile_dialog(settings) {
                Ok(Some(id)) => chosen = Some(id),
                Ok(None) => {}
                Err(msg) => *error = Some(msg),
            }
        }
        if ui
            .add_enabled(panel.profile.is_some(), egui::Button::new("Save profile…"))
            .clicked()
            && let Some(p) = &panel.profile
            && let Err(msg) = save_profile_dialog(p)
        {
            *error = Some(msg);
        }
    });
    ui.separator();
    if let Some(id) = chosen
        && let Some(p) = locus_export::profile::find(&id, &settings.profiles)
    {
        if let Err(e) = canvas.apply(&Command::SetProfile { id: Some(id) }) {
            *error = Some(format!("Could not select the profile: {e}."));
        }
        panel.format.clone_from(&p.format);
        panel.prepared = locus_export::prepare_for(canvas.document(), panel.fixed, Some(&p));
        panel.profile = Some(p);
        panel.profile_changed = true;
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn import_profile_dialog(settings: &mut Settings) -> Result<Option<String>, String> {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Machine profile", &["json"])
        .pick_file()
    else {
        return Ok(None);
    };
    let io = |e: std::io::Error| format!("Could not read the file ({}).", e.kind());
    let len = std::fs::metadata(&path).map_err(io)?.len();
    if len > locus_export::profile::MAX_PROFILE_JSON_BYTES as u64 {
        return Err("The profile file is too large.".to_owned());
    }
    let text = std::fs::read_to_string(&path).map_err(io)?;
    profiles::import(&text, settings).map(Some)
}

#[cfg(not(target_arch = "wasm32"))]
fn save_profile_dialog(p: &MachineProfile) -> Result<(), String> {
    let text = p.to_json().ok_or("Could not save the profile.")?;
    let Some(path) = pick_save(&format!("{}.json", p.id), "json", "Machine profile") else {
        return Ok(());
    };
    write_atomic(&path, text.as_bytes())
}

// The browser has no synchronous file picker; profile import there is not
// available yet (a profile can still be saved as a download).
#[cfg(target_arch = "wasm32")]
fn import_profile_dialog(_: &mut Settings) -> Result<Option<String>, String> {
    Err("Importing a profile file is not available in the browser yet.".to_owned())
}

#[cfg(target_arch = "wasm32")]
fn save_profile_dialog(p: &MachineProfile) -> Result<(), String> {
    let text = p.to_json().ok_or("Could not save the profile.")?;
    web::Web::download(
        &format!("{}.json", p.id),
        text.as_bytes(),
        "application/json",
    );
    Ok(())
}

fn open_shortcut() -> egui::KeyboardShortcut {
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::O)
}

fn strip_ext(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// Most recent first, no duplicates, capped.
#[cfg(not(target_arch = "wasm32"))]
fn push_recent(list: &mut Vec<PathBuf>, path: PathBuf) {
    list.retain(|p| p != &path);
    list.insert(0, path);
    list.truncate(MAX_RECENT);
}

// ── Platform I/O ────────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
fn pick_open() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Locus document", &["locus"])
        .pick_file()
}

#[cfg(not(target_arch = "wasm32"))]
fn pick_save(name: &str, ext: &str, filter: &str) -> Option<PathBuf> {
    let path = rfd::FileDialog::new()
        .add_filter(filter, &[ext])
        .set_file_name(name)
        .save_file()?;
    // Some platforms don't append the filter's extension.
    Some(if path.extension().is_none() {
        path.with_extension(ext)
    } else {
        path
    })
}

/// Read and parse a `.locus` file. Messages are safe to show (no paths).
#[cfg(not(target_arch = "wasm32"))]
fn read_document(path: &Path) -> Result<Document, String> {
    let io = |e: std::io::Error| format!("Could not read the file ({}).", e.kind());
    let meta = std::fs::metadata(path).map_err(io)?;
    if meta.len() > locus_doc::file::MAX_FILE_BYTES as u64 {
        return Err("The file is too large to open.".to_owned());
    }
    let text = std::fs::read_to_string(path).map_err(io)?;
    locus_doc::file::load(&text).map_err(|e| format!("Could not open the file: {e}."))
}

#[cfg(not(target_arch = "wasm32"))]
fn write_document(path: &Path, doc: &Document) -> Result<(), String> {
    let text = locus_doc::file::save(doc).map_err(|e| format!("Could not save: {e}."))?;
    write_atomic(path, text.as_bytes())
}

/// Temp file in the same directory, flushed, then renamed over the target, so
/// a failure at any point leaves the existing file untouched (SPEC §10).
#[cfg(not(target_arch = "wasm32"))]
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    let io = |e: std::io::Error| format!("Could not write the file ({}).", e.kind());
    let name = path.file_name().ok_or("Invalid file name.")?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(io(e));
    }
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use locus_doc::{Command, Editor, Geom};
    use locus_geom::Vec2;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locus-app-io-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> Document {
        let mut ed = Editor::new(Document::new());
        ed.apply(&Command::AddEntity {
            layer: Document::default_layer(),
            construction: false,
            geom: Geom::Point {
                pos: Vec2::new(1.5, 2.5),
            },
        })
        .unwrap();
        ed.into_document()
    }

    #[test]
    fn save_then_open_round_trips() {
        let dir = scratch("rt");
        let path = dir.join("a.locus");
        let doc = sample();
        write_document(&path, &doc).unwrap();
        assert_eq!(read_document(&path).unwrap(), doc);
        // No temp files left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn save_replaces_existing_file() {
        let dir = scratch("replace");
        let path = dir.join("a.locus");
        write_document(&path, &Document::new()).unwrap();
        let doc = sample();
        write_document(&path, &doc).unwrap();
        assert_eq!(read_document(&path).unwrap(), doc);
    }

    #[test]
    fn failed_save_leaves_target_and_directory_clean() {
        let dir = scratch("fail");
        // A non-empty directory in the target's place makes the final rename fail.
        let target = dir.join("a.locus");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("keep"), b"x").unwrap();
        let err = write_document(&target, &sample()).unwrap_err();
        assert!(err.starts_with("Could not write"), "{err}");
        assert!(target.join("keep").exists());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "temp file leaked"
        );
    }

    #[test]
    fn save_into_missing_directory_fails_cleanly() {
        let dir = scratch("nodir");
        let err = write_document(&dir.join("nope").join("a.locus"), &sample()).unwrap_err();
        assert!(err.starts_with("Could not write"));
    }

    #[test]
    fn bad_files_report_an_error_without_paths() {
        let dir = scratch("bad");
        let path = dir.join("secret-name.locus");
        std::fs::write(&path, "garbage").unwrap();
        let err = read_document(&path).unwrap_err();
        assert!(err.starts_with("Could not open the file"));
        assert!(!err.contains("secret-name"));
        let err = read_document(&dir.join("missing-name.locus")).unwrap_err();
        assert!(err.starts_with("Could not read"));
        assert!(!err.contains("missing-name"));
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        assert!(read_document(&path).is_err());
    }

    #[test]
    fn recent_files_are_mru_unique_and_capped() {
        let mut r = Vec::new();
        for i in 0..15 {
            push_recent(&mut r, PathBuf::from(format!("f{i}")));
        }
        assert_eq!(r.len(), MAX_RECENT);
        assert_eq!(r[0], PathBuf::from("f14"));
        push_recent(&mut r, PathBuf::from("f10"));
        assert_eq!(r[0], PathBuf::from("f10"));
        assert_eq!(r.iter().filter(|p| p.as_os_str() == "f10").count(), 1);
    }
}
