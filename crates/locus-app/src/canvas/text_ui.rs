// SPDX-License-Identifier: AGPL-3.0-or-later

//! Text tool settings and the font picker (SPEC §9, #180).
//!
//! The same controls serve the Text tool (settings for the next text placed)
//! and the properties panel (editing the selected text). Fonts come from three
//! places: bundled, files the user loads, and installed system fonts on
//! desktop only (a browser cannot list them). The document keeps only a font
//! *reference* plus cached geometry (ADR 0012), so a missing font degrades to
//! "pick another font to edit", never to broken output.

use locus_doc::{DocError, TextData};
use locus_text::{Align, BundledFont, FontLibrary, FontProvider, FontRef};

/// Everything a text needs besides its geometry: what the user edits.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSettings {
    pub content: String,
    pub font: FontRef,
    pub size: f64,
    pub align: Align,
    pub line_spacing: f64,
}

impl Default for TextSettings {
    fn default() -> Self {
        Self {
            content: "Text".to_owned(),
            font: FontRef::bundled(BundledFont::Figtree),
            size: 10.0,
            align: Align::Left,
            line_spacing: 1.0,
        }
    }
}

impl TextSettings {
    pub fn from_data(t: &TextData) -> Self {
        Self {
            content: t.content.clone(),
            font: t.font.clone(),
            size: t.size,
            align: t.align,
            line_spacing: t.line_spacing,
        }
    }

    /// Lay the text out with the fonts available now.
    pub fn to_data(&self, fonts: &dyn FontProvider) -> Result<TextData, DocError> {
        TextData::new(
            &self.content,
            self.font.clone(),
            self.size,
            self.align,
            self.line_spacing,
            fonts,
        )
    }
}

/// What a frame of editing changed.
#[derive(Debug, Default, Clone, Copy)]
pub struct Edited {
    /// The content text changed (not yet committed).
    pub content: bool,
    /// The content field lost focus: commit it.
    pub content_done: bool,
    /// Font, size, alignment or spacing changed: commit now.
    pub style: bool,
}

impl Edited {
    pub fn commit(self) -> bool {
        self.style || self.content_done
    }
}

/// Loaded fonts and the picker's transient state.
pub struct FontCatalog {
    pub library: FontLibrary,
    filter: String,
    /// Last problem loading a font file, shown under the picker.
    pub notice: Option<String>,
    #[cfg(target_arch = "wasm32")]
    web: web::Pick,
}

impl std::fmt::Debug for FontCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontCatalog").finish_non_exhaustive()
    }
}

impl FontCatalog {
    /// No system fonts and no browser picker: what a headless canvas (tests,
    /// first frame) has until the app installs the real catalog.
    pub fn headless() -> Self {
        Self {
            library: FontLibrary::new(),
            filter: String::new(),
            notice: None,
            #[cfg(target_arch = "wasm32")]
            web: web::Pick::new(&egui::Context::default()),
        }
    }

    pub fn new(ctx: &egui::Context) -> Self {
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut library = FontLibrary::new();
        #[cfg(not(target_arch = "wasm32"))]
        library.set_system_source(Box::new(system::DirFonts::scan()));
        #[cfg(target_arch = "wasm32")]
        let web = web::Pick::new(ctx);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = ctx;
        Self {
            library,
            filter: String::new(),
            notice: None,
            #[cfg(target_arch = "wasm32")]
            web,
        }
    }

    /// A font file picked in the browser arrives asynchronously.
    pub fn poll(&mut self) -> Option<FontRef> {
        #[cfg(target_arch = "wasm32")]
        {
            let (name, bytes) = self.web.poll()?;
            return self.add_file(&name, bytes);
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }

    fn add_file(&mut self, name: &str, bytes: Vec<u8>) -> Option<FontRef> {
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        match self.library.add_user_font_bytes(stem, bytes) {
            Ok(r) => {
                self.notice = None;
                Some(r)
            }
            Err(_) => {
                self.notice = Some("That file is not a usable font.".to_owned());
                None
            }
        }
    }

    /// "Load font file…": a dialog on desktop, the browser's picker on the web.
    fn load_file(&mut self) -> Option<FontRef> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (name, bytes) = match system::pick_font_file() {
                Ok(Some(v)) => v,
                Ok(None) => return None,
                Err(msg) => {
                    self.notice = Some(msg);
                    return None;
                }
            };
            return self.add_file(&name, bytes);
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.web.start();
            None
        }
    }

    /// Is this font unavailable right now?
    pub fn missing(&self, font: &FontRef) -> bool {
        self.library.resolve(font).is_none()
    }
}

/// The text controls. Returns what changed; `content_enabled` is false while
/// the text's font is missing (the content can't be re-laid out then).
pub fn controls(
    ui: &mut egui::Ui,
    s: &mut TextSettings,
    cat: &mut FontCatalog,
    content_enabled: bool,
    multiline_rows: usize,
) -> Edited {
    let mut out = Edited::default();
    ui.add_enabled_ui(content_enabled, |ui| {
        let r = ui.add(
            egui::TextEdit::multiline(&mut s.content)
                .desired_rows(multiline_rows)
                .desired_width(180.0)
                .hint_text("Text"),
        );
        out.content = r.changed();
        out.content_done = r.lost_focus() && !ui.input(|i| i.key_pressed(egui::Key::Escape));
    });
    if let Some(f) = cat.poll() {
        s.font = f;
        out.style = true;
    }
    out.style |= font_picker(ui, s, cat);
    out.style |= ui
        .add(
            egui::DragValue::new(&mut s.size)
                .range(0.1..=10_000.0)
                .speed(0.1)
                .suffix(" mm"),
        )
        .on_hover_text("Font size (the em height)")
        .changed();
    egui::ComboBox::from_id_salt("text-align")
        .width(70.0)
        .selected_text(match s.align {
            Align::Left => "Left",
            Align::Center => "Centre",
            Align::Right => "Right",
        })
        .show_ui(ui, |ui| {
            for (a, l) in [
                (Align::Left, "Left"),
                (Align::Center, "Centre"),
                (Align::Right, "Right"),
            ] {
                out.style |= ui.selectable_value(&mut s.align, a, l).changed();
            }
        });
    out.style |= ui
        .add(
            egui::DragValue::new(&mut s.line_spacing)
                .range(0.2..=10.0)
                .speed(0.01)
                .prefix("× "),
        )
        .on_hover_text("Line spacing")
        .changed();
    if let Some(n) = &cat.notice {
        ui.colored_label(ui.visuals().warn_fg_color, n);
    }
    out
}

fn font_picker(ui: &mut egui::Ui, s: &mut TextSettings, cat: &mut FontCatalog) -> bool {
    let mut changed = false;
    let missing = cat.missing(&s.font);
    let label = if missing {
        format!("{} (missing)", s.font.display_name())
    } else {
        s.font.display_name()
    };
    egui::ComboBox::from_id_salt("text-font")
        .width(190.0)
        .selected_text(label)
        .show_ui(ui, |ui| {
            ui.weak("Bundled");
            for f in BundledFont::ALL {
                let r = FontRef::bundled(f);
                changed |= ui
                    .selectable_value(&mut s.font, r, f.display_name())
                    .changed();
            }
            let files: Vec<String> = cat.library.user_fonts().map(str::to_owned).collect();
            if !files.is_empty() {
                ui.separator();
                ui.weak("Loaded files");
                for n in files {
                    changed |= ui
                        .selectable_value(&mut s.font, FontRef::File(n.clone()), &n)
                        .changed();
                }
            }
            if cat.library.has_system_fonts() {
                ui.separator();
                ui.weak("System fonts");
                ui.add(
                    egui::TextEdit::singleline(&mut cat.filter)
                        .hint_text("Filter")
                        .desired_width(150.0),
                );
                let needle = cat.filter.to_lowercase();
                let families: Vec<String> = cat
                    .library
                    .system_families()
                    .into_iter()
                    .filter(|f| needle.is_empty() || f.to_lowercase().contains(&needle))
                    .collect();
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show_rows(ui, 20.0, families.len(), |ui, range| {
                        for f in &families[range] {
                            changed |= ui
                                .selectable_value(&mut s.font, FontRef::System(f.clone()), f)
                                .changed();
                        }
                    });
            }
            ui.separator();
            if ui.button("Load font file…").clicked() {
                if let Some(r) = cat.load_file() {
                    s.font = r;
                    changed = true;
                }
                ui.close();
            }
        });
    if missing {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Font not available here: the saved outlines still export. Pick another font to edit.",
        );
    }
    changed
}

#[cfg(not(target_arch = "wasm32"))]
mod system {
    //! Installed fonts on desktop, found by walking the platform's font
    //! folders. Listing needs no file contents; a font is read only when a
    //! text uses it. Names are file stems, which is what the document stores.

    use locus_text::SystemFontSource;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    const MAX_DEPTH: usize = 4;
    /// Matches `locus_text::outline::MAX_OUTLINE_FONT_BYTES`.
    const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;

    #[derive(Debug)]
    pub struct DirFonts {
        files: BTreeMap<String, PathBuf>,
    }

    impl DirFonts {
        pub fn scan() -> Self {
            let mut files = BTreeMap::new();
            for dir in font_dirs() {
                walk(&dir, 0, &mut files);
            }
            Self { files }
        }
    }

    fn env_dir(var: &str) -> Option<PathBuf> {
        std::env::var_os(var).map(PathBuf::from)
    }

    fn font_dirs() -> Vec<PathBuf> {
        let mut v = Vec::new();
        if cfg!(windows) {
            v.push(env_dir("WINDIR").unwrap_or_else(|| "C:\\Windows".into()).join("Fonts"));
            if let Some(l) = env_dir("LOCALAPPDATA") {
                v.push(l.join("Microsoft").join("Windows").join("Fonts"));
            }
        } else if cfg!(target_os = "macos") {
            v.push("/System/Library/Fonts".into());
            v.push("/Library/Fonts".into());
            if let Some(h) = env_dir("HOME") {
                v.push(h.join("Library").join("Fonts"));
            }
        } else {
            v.push("/usr/share/fonts".into());
            v.push("/usr/local/share/fonts".into());
            if let Some(h) = env_dir("HOME") {
                v.push(h.join(".local").join("share").join("fonts"));
                v.push(h.join(".fonts"));
            }
        }
        v
    }

    fn is_font(p: &Path) -> bool {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ["ttf", "otf", "ttc"].iter().any(|x| e.eq_ignore_ascii_case(x)))
    }

    fn walk(dir: &Path, depth: usize, out: &mut BTreeMap<String, PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let Ok(ty) = entry.file_type() else { continue };
            if ty.is_dir() {
                if depth < MAX_DEPTH {
                    walk(&p, depth + 1, out);
                }
            } else if is_font(&p)
                && let Some(stem) = p.file_stem().and_then(|s| s.to_str())
            {
                out.entry(stem.to_owned()).or_insert(p);
            }
        }
    }

    impl SystemFontSource for DirFonts {
        fn families(&self) -> Vec<String> {
            self.files.keys().cloned().collect()
        }

        fn load(&self, family: &str) -> Option<Vec<u8>> {
            let path = self.files.get(family)?;
            if std::fs::metadata(path).ok()?.len() > MAX_FONT_BYTES {
                return None;
            }
            std::fs::read(path).ok()
        }
    }

    /// The "Load font file…" dialog. `Ok(None)` = cancelled. Messages never
    /// include paths.
    pub fn pick_font_file() -> Result<Option<(String, Vec<u8>)>, String> {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Fonts", &["ttf", "otf", "svg"])
            .pick_file()
        else {
            return Ok(None);
        };
        let io = |e: std::io::Error| format!("Could not read the font ({}).", e.kind());
        let meta = std::fs::metadata(&path).map_err(io)?;
        if meta.len() > MAX_FONT_BYTES {
            return Err("That font file is too large.".to_owned());
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("font")
            .to_owned();
        Ok(Some((name, std::fs::read(&path).map_err(io)?)))
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    //! Browser font picker; the bytes arrive asynchronously (see `text_web.js`).

    use std::sync::mpsc::{Receiver, Sender, channel};
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(module = "/src/canvas/text_web.js")]
    extern "C" {
        #[wasm_bindgen(catch)]
        async fn pick_font() -> Result<JsValue, JsValue>;
        fn take_font() -> Vec<u8>;
    }

    pub struct Pick {
        tx: Sender<(String, Vec<u8>)>,
        rx: Receiver<(String, Vec<u8>)>,
        ctx: egui::Context,
    }

    impl Pick {
        pub fn new(ctx: &egui::Context) -> Self {
            let (tx, rx) = channel();
            Self {
                tx,
                rx,
                ctx: ctx.clone(),
            }
        }

        pub fn start(&self) {
            let tx = self.tx.clone();
            let ctx = self.ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(v) = pick_font().await
                    && let Some(name) = v.as_string()
                {
                    let _ = tx.send((name, take_font()));
                    ctx.request_repaint();
                }
            });
        }

        pub fn poll(&mut self) -> Option<(String, Vec<u8>)> {
            self.rx.try_recv().ok()
        }
    }
}
