// SPDX-License-Identifier: AGPL-3.0-or-later

//! Text tool settings and the font picker (SPEC §9, #180).
//!
//! The same controls serve the Text tool (settings for the next text placed),
//! the Text dialog and the inspector (editing a placed text, #222). Fonts
//! come from three places: bundled, fonts the user added (kept between
//! sessions, `crate::font_store`), and installed system fonts on desktop only
//! (a browser cannot list them). The font list is resizable, searchable,
//! filtered by where the font comes from, and previews each font in its own
//! face (design §6.5). The document keeps only a font
//! *reference* plus cached geometry (ADR 0012), so a missing font degrades to
//! "pick another font to edit", never to broken output.

use locus_doc::{DocError, TextData};
use locus_geom::Vec2;
use locus_text::{Align, BundledFont, FontLibrary, FontProvider, FontRef, TextStyle};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Everything a text needs besides its geometry: what the user edits.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSettings {
    pub content: String,
    pub font: FontRef,
    pub size: f64,
    pub align: Align,
    pub line_spacing: f64,
    /// Letter spacing in mm (#222).
    pub tracking: f64,
}

impl Default for TextSettings {
    fn default() -> Self {
        Self {
            content: "Text".to_owned(),
            font: FontRef::bundled(BundledFont::Figtree),
            size: 10.0,
            align: Align::Left,
            line_spacing: 1.0,
            tracking: 0.0,
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
            tracking: t.tracking,
        }
    }

    /// Lay the text out with the fonts available now.
    pub fn to_data(&self, fonts: &dyn FontProvider) -> Result<TextData, DocError> {
        let t = TextData::new(
            &self.content,
            self.font.clone(),
            self.size,
            self.align,
            self.line_spacing,
            fonts,
        )?;
        if self.tracking == 0.0 {
            Ok(t)
        } else {
            t.edited(fonts, |t| t.tracking = self.tracking)
        }
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
/// Which fonts the list shows (design §6.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontKind {
    All,
    System,
    Bundled,
    Custom,
}

impl FontKind {
    const ALL: [FontKind; 4] = [
        FontKind::All,
        FontKind::System,
        FontKind::Bundled,
        FontKind::Custom,
    ];

    fn label(self) -> &'static str {
        match self {
            FontKind::All => "All",
            FontKind::System => "System",
            FontKind::Bundled => "Bundled",
            FontKind::Custom => "Custom",
        }
    }
}

/// A font's name drawn in its own face, as polylines in em units (y up).
type Preview = Option<Arc<Vec<Vec<Vec2>>>>;

pub struct FontCatalog {
    pub library: FontLibrary,
    filter: String,
    kind: FontKind,
    /// Previews made so far (a font is laid out once, when first shown).
    previews: RefCell<BTreeMap<FontRef, Preview>>,
    /// Last problem loading a font file, shown under the picker.
    pub notice: Option<String>,
    #[cfg(target_arch = "wasm32")]
    web: web::Pick,
    #[cfg(target_arch = "wasm32")]
    store: crate::font_store::web::Store,
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
            kind: FontKind::All,
            previews: RefCell::new(BTreeMap::new()),
            notice: None,
            #[cfg(target_arch = "wasm32")]
            web: web::Pick::new(&egui::Context::default()),
            #[cfg(target_arch = "wasm32")]
            store: crate::font_store::web::Store::open(&egui::Context::default()),
        }
    }

    pub fn new(ctx: &egui::Context) -> Self {
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut library = FontLibrary::new();
        #[cfg(not(target_arch = "wasm32"))]
        library.set_system_source(Box::new(system::DirFonts::scan()));
        // Fonts added in earlier sessions (#222). A file that no longer
        // parses is skipped, never fatal.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(dir) = crate::font_store::dir() {
            for (name, bytes) in crate::font_store::load_all(&dir) {
                let _ = library.add_user_font_bytes(&name, bytes);
            }
        }
        #[cfg(target_arch = "wasm32")]
        let web = web::Pick::new(ctx);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = ctx;
        Self {
            library,
            filter: String::new(),
            kind: FontKind::All,
            previews: RefCell::new(BTreeMap::new()),
            notice: None,
            #[cfg(target_arch = "wasm32")]
            web,
            #[cfg(target_arch = "wasm32")]
            store: crate::font_store::web::Store::open(ctx),
        }
    }

    /// A font file picked in the browser arrives asynchronously.
    pub fn poll(&mut self) -> Option<FontRef> {
        #[cfg(target_arch = "wasm32")]
        {
            // Fonts kept from earlier sessions arrive first, quietly.
            while let Some((name, bytes)) = self.store.poll() {
                let _ = self.library.add_user_font_bytes(&name, bytes);
            }
            let (name, bytes) = self.web.poll()?;
            self.add_file(&name, bytes)
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }

    /// Add a font file: checked first, then kept for later sessions under a
    /// safe name, which is also the name documents refer to it by.
    fn add_file(&mut self, name: &str, bytes: Vec<u8>) -> Option<FontRef> {
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        let stem = crate::font_store::safe_stem(stem);
        let kept = bytes.clone();
        match self.library.add_user_font_bytes(&stem, bytes) {
            Ok(r) => {
                self.notice = None;
                self.previews.borrow_mut().remove(&r);
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(dir) = crate::font_store::dir()
                    && let Err(e) = crate::font_store::save(&dir, &stem, &kept)
                {
                    self.notice = Some(format!("Added for this session only. {e}"));
                }
                #[cfg(target_arch = "wasm32")]
                crate::font_store::web::Store::save(&stem, kept);
                Some(r)
            }
            Err(_) => {
                self.notice = Some("That file is not a usable font.".to_owned());
                None
            }
        }
    }

    /// Remove an added font, here and from the store.
    fn remove(&mut self, name: &str) {
        self.library.remove_user_font(name);
        self.previews
            .borrow_mut()
            .remove(&FontRef::File(name.to_owned()));
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(dir) = crate::font_store::dir() {
            crate::font_store::remove(&dir, name);
        }
        #[cfg(target_arch = "wasm32")]
        crate::font_store::web::Store::remove(name);
    }

    /// The list's rows: (font, name, tag), filtered by kind and search.
    fn rows(&self) -> Vec<(FontRef, String, &'static str)> {
        let needle = self.filter.trim().to_lowercase();
        let want = |k: FontKind| self.kind == FontKind::All || self.kind == k;
        let mut v = Vec::new();
        if want(FontKind::Bundled) {
            for f in BundledFont::ALL {
                let tag = if f.is_single_line() {
                    "Single-line"
                } else {
                    "Bundled"
                };
                v.push((FontRef::bundled(f), f.display_name().to_owned(), tag));
            }
        }
        if want(FontKind::Custom) {
            for n in self.library.user_fonts() {
                v.push((FontRef::File(n.to_owned()), n.to_owned(), "Custom"));
            }
        }
        if want(FontKind::System) {
            for f in self.library.system_families() {
                v.push((FontRef::System(f.clone()), f, "System"));
            }
        }
        v.retain(|(_, name, _)| needle.is_empty() || name.to_lowercase().contains(&needle));
        v
    }

    /// `name` laid out in `font`, once (None if the font can't be loaded).
    fn preview(&self, font: &FontRef, name: &str) -> Preview {
        if let Some(p) = self.previews.borrow().get(font) {
            return p.clone();
        }
        let style = TextStyle {
            size: 1.0,
            ..TextStyle::default()
        };
        let p = self
            .library
            .resolve(font)
            .and_then(|f| f.layout(name, &style).ok())
            .map(|g| {
                Arc::new(
                    g.contours
                        .iter()
                        .map(|c| {
                            let mut pts = vec![c.start];
                            for k in c.cubics() {
                                pts.extend(k.flatten(0.01).into_iter().skip(1));
                            }
                            pts
                        })
                        .collect(),
                )
            });
        self.previews.borrow_mut().insert(font.clone(), p.clone());
        p
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
            self.add_file(&name, bytes)
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
    focus: bool,
) -> Edited {
    let mut out = Edited::default();
    ui.add_enabled_ui(content_enabled, |ui| {
        let out_edit = egui::TextEdit::multiline(&mut s.content)
            .desired_rows(multiline_rows)
            .desired_width(300.0)
            .hint_text("Text")
            .show(ui);
        let r = out_edit.response.response.clone();
        if focus {
            // Just placed or double-clicked: the content is selected, so
            // typing replaces it (design §6.5).
            r.request_focus();
            let mut state = out_edit.state;
            let end = s.content.chars().count();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(end),
                )));
            state.store(ui.ctx(), r.id);
        }
        out.content = r.changed();
        // Commit on focus loss or Ctrl+Enter (clicking blank panel space does not drop focus in egui).
        out.content_done = (r.lost_focus() && !ui.input(|i| i.key_pressed(egui::Key::Escape)))
            || (r.has_focus()
                && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)));
    });
    if let Some(f) = cat.poll() {
        s.font = f;
        out.style = true;
    }
    out.style |= font_picker(ui, s, cat);
    egui::Grid::new(ui.id().with("text-style"))
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Height");
            out.style |= ui
                .add(
                    egui::DragValue::new(&mut s.size)
                        .range(0.1..=10_000.0)
                        .speed(0.1)
                        .suffix(" mm"),
                )
                .on_hover_text("Font size (the em height)")
                .changed();
            ui.end_row();
            ui.label("Alignment");
            ui.horizontal(|ui| {
                for (a, l) in [
                    (Align::Left, "Left"),
                    (Align::Center, "Centre"),
                    (Align::Right, "Right"),
                ] {
                    out.style |= ui.selectable_value(&mut s.align, a, l).changed();
                }
            });
            ui.end_row();
            ui.label("Line spacing");
            out.style |= ui
                .add(
                    egui::DragValue::new(&mut s.line_spacing)
                        .range(0.2..=10.0)
                        .speed(0.01)
                        .prefix("× "),
                )
                .changed();
            ui.end_row();
            ui.label("Letter spacing");
            out.style |= ui
                .add(
                    egui::Slider::new(&mut s.tracking, -2.0..=10.0)
                        .clamping(egui::SliderClamping::Never)
                        .step_by(0.05)
                        .suffix(" mm"),
                )
                .on_hover_text("Space added between letters (tracking): drag, or click to type; negative tightens")
                .changed();
            ui.end_row();
        });
    if let Some(n) = &cat.notice {
        ui.colored_label(ui.visuals().warn_fg_color, n);
    }
    out
}

/// Height of one row in the font list, points.
const ROW_PT: f32 = 26.0;
/// The list's height: remembered per user (egui's persisted memory).
const LIST_HEIGHT_DEFAULT: f32 = 180.0;
const LIST_HEIGHT_MIN: f32 = 80.0;
const LIST_HEIGHT_MAX: f32 = 900.0;

/// The font list (design §6.5): a search box, an All / System / Bundled /
/// Custom filter, then a resizable list with each font in its own face and
/// a tag saying where it comes from. "Add font…" loads a file; added fonts
/// can be removed. Returns whether the font changed.
fn font_picker(ui: &mut egui::Ui, s: &mut TextSettings, cat: &mut FontCatalog) -> bool {
    let mut changed = false;
    let missing = cat.missing(&s.font);
    ui.horizontal(|ui| {
        ui.label("Font");
        ui.strong(if missing {
            format!("{} (missing)", s.font.display_name())
        } else {
            s.font.display_name()
        });
    });
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut cat.filter)
                .hint_text("Search fonts")
                .desired_width(140.0),
        );
        for k in FontKind::ALL {
            if k == FontKind::System && !cat.library.has_system_fonts() {
                continue;
            }
            ui.selectable_value(&mut cat.kind, k, k.label());
        }
    });
    let id = ui.id().with("font-list-height");
    let mut height = ui.data_mut(|d| *d.get_persisted_mut_or(id, LIST_HEIGHT_DEFAULT));
    let rows = cat.rows();
    let mut remove = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_min_height(height);
        ui.set_max_height(height);
        egui::ScrollArea::vertical()
            .id_salt("font-list")
            .auto_shrink([false, false])
            .show_rows(ui, ROW_PT, rows.len(), |ui, range| {
                for (font, name, tag) in &rows[range] {
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), ROW_PT),
                        egui::Sense::click(),
                    );
                    let selected = s.font == *font;
                    let vis = ui.visuals();
                    if selected {
                        ui.painter().rect_filled(rect, 3.0, vis.selection.bg_fill);
                    } else if resp.hovered() {
                        ui.painter()
                            .rect_filled(rect, 3.0, vis.widgets.hovered.weak_bg_fill);
                    }
                    let fg = if selected {
                        vis.selection.stroke.color
                    } else {
                        vis.text_color()
                    };
                    // The tag, right-aligned; Custom rows also get a remove ✕.
                    let mut right = rect.right() - 6.0;
                    if *tag == "Custom" {
                        let x = egui::Rect::from_center_size(
                            egui::pos2(right - 8.0, rect.center().y),
                            egui::vec2(16.0, 16.0),
                        );
                        let xr = ui.interact(x, resp.id.with("remove"), egui::Sense::click());
                        ui.painter().text(
                            x.center(),
                            egui::Align2::CENTER_CENTER,
                            "✕",
                            egui::FontId::proportional(12.0),
                            if xr.hovered() {
                                vis.error_fg_color
                            } else {
                                vis.weak_text_color()
                            },
                        );
                        if xr.on_hover_text("Remove this font").clicked() {
                            remove = Some(name.clone());
                        }
                        right -= 22.0;
                    }
                    let tag_rect = ui.painter().text(
                        egui::pos2(right, rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        *tag,
                        egui::FontId::proportional(10.0),
                        vis.weak_text_color(),
                    );
                    // The name in its own face, or in the UI font when the
                    // font can't be drawn here.
                    let left = egui::pos2(rect.left() + 6.0, rect.bottom() - 7.0);
                    let drawn = cat.preview(font, name).is_some_and(|p| {
                        let em = 15.0_f32;
                        let clip = ui.painter().with_clip_rect(egui::Rect::from_min_max(
                            rect.min,
                            egui::pos2(tag_rect.left() - 6.0, rect.max.y),
                        ));
                        for line in p.iter() {
                            let pts: Vec<egui::Pos2> = line
                                .iter()
                                .map(|q| left + egui::vec2(q.x as f32 * em, -(q.y as f32) * em))
                                .collect();
                            clip.add(egui::Shape::line(pts, egui::Stroke::new(1.1, fg)));
                        }
                        !p.is_empty()
                    });
                    if !drawn {
                        ui.painter().text(
                            egui::pos2(rect.left() + 6.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            name,
                            egui::FontId::proportional(14.0),
                            fg,
                        );
                    }
                    if resp.on_hover_text(name.as_str()).clicked() && !selected {
                        s.font = font.clone();
                        changed = true;
                    }
                }
            });
    });
    // Resize: drag the grip under the list, or ⤢ Taller.
    ui.horizontal(|ui| {
        let (grip, r) = ui.allocate_exact_size(egui::vec2(60.0, 10.0), egui::Sense::drag());
        ui.painter().hline(
            grip.x_range(),
            grip.center().y,
            egui::Stroke::new(2.0, ui.visuals().weak_text_color()),
        );
        if r.hovered() || r.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        height += r.on_hover_text("Drag to resize the list").drag_delta().y;
        if ui.small_button("↕ Taller").clicked() {
            height += 160.0;
        }
        if ui
            .button("Add font…")
            .on_hover_text(
                "Load a .ttf, .otf, .ttc or single-line .svg font; it is kept for next time",
            )
            .clicked()
            && let Some(r) = cat.load_file()
        {
            s.font = r;
            changed = true;
        }
    });
    height = height.clamp(LIST_HEIGHT_MIN, LIST_HEIGHT_MAX);
    ui.data_mut(|d| d.insert_persisted(id, height));
    if let Some(name) = remove {
        cat.remove(&name);
    }
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
            v.push(
                env_dir("WINDIR")
                    .unwrap_or_else(|| "C:\\Windows".into())
                    .join("Fonts"),
            );
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
        p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            ["ttf", "otf", "ttc"]
                .iter()
                .any(|x| e.eq_ignore_ascii_case(x))
        })
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
            .add_filter("Fonts", &["ttf", "otf", "ttc", "svg"])
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

    #[wasm_bindgen(module = "/src/views/text_web.js")]
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
