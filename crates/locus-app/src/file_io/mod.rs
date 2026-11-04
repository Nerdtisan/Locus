// SPDX-License-Identifier: AGPL-3.0-or-later

//! File handling for the GUI shell: Open / Save / Save As / Export SVG, the
//! unsaved-changes guard and recent files (SPEC §10, M1 #74).
//!
//! The document is only ever replaced through [`Canvas::load`], which starts
//! a fresh command history (SPEC §11). Error messages shown to the user never
//! contain file names or document contents, and nothing here logs either
//! (CLAUDE.md, Secure Coding).

#[cfg(target_arch = "wasm32")]
mod web;

use crate::canvas::Canvas;
use locus_doc::Document;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::path::PathBuf;

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
            allow_close: false,
            last_title: String::new(),
            #[cfg(target_arch = "wasm32")]
            web: web::Web::new(ctx),
        }
    }

    pub fn save_state(&self, storage: &mut dyn eframe::Storage) {
        if let Ok(json) = serde_json::to_string(&self.recent) {
            storage.set_string(RECENT_KEY, json);
        }
    }

    fn dirty(&self, canvas: &Canvas) -> bool {
        canvas.document() != &self.baseline
    }

    /// Replace the open document with a built-in sample (guarded).
    pub fn open_sample(&mut self, ctx: &egui::Context, canvas: &mut Canvas, doc: Document) {
        self.request(ctx, canvas, Action::Replace(Box::new(doc)));
    }

    /// Per-frame work: shortcuts, the close guard, window title and dialogs.
    pub fn update(&mut self, ctx: &egui::Context, canvas: &mut Canvas) {
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
        self.dialogs(ctx, canvas);
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
    pub fn file_menu(&mut self, ui: &mut egui::Ui, canvas: &mut Canvas) {
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
        if ui.button("Export SVG…").clicked() {
            self.export_svg(canvas);
            ui.close();
        }
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
        web::Web::download(&name, &text, "application/octet-stream");
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

    #[cfg(target_arch = "wasm32")]
    fn export_svg(&mut self, canvas: &Canvas) {
        let stem = self.name.as_deref().map_or("untitled", strip_ext);
        let svg = locus_export::to_svg(canvas.document());
        web::Web::download(
            &format!("{stem}.svg"),
            &String::from_utf8_lossy(&svg),
            "image/svg+xml",
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn export_svg(&mut self, canvas: &Canvas) {
        let stem = self.name.as_deref().map_or("untitled", strip_ext);
        let Some(path) = pick_save(&format!("{stem}.svg"), "svg", "SVG image") else {
            return;
        };
        let bytes = locus_export::to_svg(canvas.document());
        if let Err(msg) = write_atomic(&path, &bytes) {
            self.error = Some(msg);
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

    fn dialogs(&mut self, ctx: &egui::Context, canvas: &mut Canvas) {
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
