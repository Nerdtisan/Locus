// SPDX-License-Identifier: AGPL-3.0-or-later
//! Fonts the user added, kept between sessions (#222, design §6.5).
//!
//! On the desktop they are copied into the app's `fonts` folder beside its
//! settings and plugins. On the web they go into the browser's private file
//! system (OPFS). Documents keep only the font *reference* (ADR 0012), so a
//! store that is lost or emptied costs nothing but the ability to edit.
//!
//! Font files are untrusted input: names are sanitised, sizes and counts
//! are bounded, and a file that doesn't parse is refused before it is kept.
//! Paths and names are never logged.

/// Largest font file kept (matches `locus_text::outline::MAX_OUTLINE_FONT_BYTES`).
pub const MAX_FONT_BYTES: usize = 64 * 1024 * 1024;
/// Most fonts kept.
pub const MAX_FONTS: usize = 500;

/// A safe file stem for a font name: letters, digits, space, `-`, `_` and
/// `.`; at most 100 characters; never empty or hidden.
#[must_use]
pub fn safe_stem(name: &str) -> String {
    let s: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        .take(100)
        .collect();
    let s = s.trim().trim_start_matches('.').to_owned();
    if s.is_empty() { "font".to_owned() } else { s }
}

/// The extension a font file is stored under: what it is, not what it was
/// called (an SVG font is text; anything else is an OpenType binary).
#[must_use]
pub fn extension(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"ttcf") {
        "ttc"
    } else if bytes.starts_with(b"OTTO") {
        "otf"
    } else if std::str::from_utf8(bytes)
        .is_ok_and(|t| t.trim_start().starts_with(['<', '\u{feff}']))
    {
        "svg"
    } else {
        "ttf"
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use super::{MAX_FONT_BYTES, MAX_FONTS, extension, safe_stem};
    use std::io::Read;
    use std::path::{Path, PathBuf};

    /// The app's font folder.
    pub fn dir() -> Option<PathBuf> {
        eframe::storage_dir("locus").map(|d| d.join("fonts"))
    }

    fn is_font(p: &Path) -> bool {
        p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            ["ttf", "otf", "ttc", "svg"]
                .iter()
                .any(|x| e.eq_ignore_ascii_case(x))
        })
    }

    /// Every stored font as (name, bytes), bounded in size and count.
    pub fn load_all(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| is_font(p))
            .collect();
        paths.sort();
        paths
            .into_iter()
            .take(MAX_FONTS)
            .filter_map(|p| {
                let stem = p.file_stem()?.to_str()?.to_owned();
                let mut data = Vec::new();
                std::fs::File::open(&p)
                    .ok()?
                    .take(MAX_FONT_BYTES as u64 + 1)
                    .read_to_end(&mut data)
                    .ok()?;
                (data.len() <= MAX_FONT_BYTES).then_some((stem, data))
            })
            .collect()
    }

    /// Keep `bytes` as font `name`, atomically (temp file, then rename), so a
    /// failed write never leaves half a font. Returns the stored name.
    pub fn save(dir: &Path, name: &str, bytes: &[u8]) -> Result<String, String> {
        if bytes.len() > MAX_FONT_BYTES {
            return Err("That font file is too large.".to_owned());
        }
        let stem = safe_stem(name);
        let io = |e: std::io::Error| format!("Couldn't keep the font ({}).", e.kind());
        std::fs::create_dir_all(dir).map_err(io)?;
        let count = std::fs::read_dir(dir).map_err(io)?.flatten().count();
        if count >= MAX_FONTS {
            return Err(format!("At most {MAX_FONTS} fonts can be added."));
        }
        remove(dir, &stem);
        let path = dir.join(format!("{stem}.{}", extension(bytes)));
        let tmp = dir.join(format!(".{stem}.tmp"));
        std::fs::write(&tmp, bytes).map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            io(e)
        })?;
        Ok(stem)
    }

    /// Forget font `name` (every extension).
    pub fn remove(dir: &Path, name: &str) {
        let stem = safe_stem(name);
        for ext in ["ttf", "otf", "ttc", "svg"] {
            let _ = std::fs::remove_file(dir.join(format!("{stem}.{ext}")));
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use desktop::{dir, load_all, remove, save};

#[cfg(target_arch = "wasm32")]
pub mod web {
    //! OPFS storage (see `views/text_web.js`). Reads arrive asynchronously.

    use super::safe_stem;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(module = "/src/views/text_web.js")]
    extern "C" {
        #[wasm_bindgen(catch)]
        async fn font_store_save(name: &str, bytes: &[u8]) -> Result<JsValue, JsValue>;
        #[wasm_bindgen(catch)]
        async fn font_store_delete(name: &str) -> Result<JsValue, JsValue>;
        #[wasm_bindgen(catch)]
        async fn font_store_load_all() -> Result<JsValue, JsValue>;
        fn font_store_take(i: u32) -> Vec<u8>;
        fn font_store_name(i: u32) -> String;
    }

    /// Fonts read back at start-up, delivered to the catalog by `poll`.
    pub struct Store {
        rx: Receiver<(String, Vec<u8>)>,
    }

    impl Store {
        /// Start reading every stored font.
        pub fn open(ctx: &egui::Context) -> Self {
            let (tx, rx): (Sender<(String, Vec<u8>)>, _) = channel();
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(n) = font_store_load_all().await
                    && let Some(n) = n.as_f64()
                {
                    for i in 0..(n as u32).min(super::MAX_FONTS as u32) {
                        let _ = tx.send((font_store_name(i), font_store_take(i)));
                    }
                    ctx.request_repaint();
                }
            });
            Self { rx }
        }

        pub fn poll(&mut self) -> Option<(String, Vec<u8>)> {
            self.rx.try_recv().ok()
        }

        pub fn save(name: &str, bytes: Vec<u8>) {
            let stem = safe_stem(name);
            wasm_bindgen_futures::spawn_local(async move {
                let _ = font_store_save(&stem, &bytes).await;
            });
        }

        pub fn remove(name: &str) {
            let stem = safe_stem(name);
            wasm_bindgen_futures::spawn_local(async move {
                let _ = font_store_delete(&stem).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe() {
        assert_eq!(safe_stem("My Font-Bold_2.1"), "My Font-Bold_2.1");
        assert_eq!(safe_stem("../../etc/passwd"), "etcpasswd");
        assert_eq!(safe_stem("..."), "font");
        assert_eq!(safe_stem("a\u{0}b/c\\d:e"), "abcde");
        assert_eq!(safe_stem(&"x".repeat(500)).len(), 100);
    }

    #[test]
    fn extensions_follow_the_content() {
        assert_eq!(extension(b"ttcf...."), "ttc");
        assert_eq!(extension(b"OTTO...."), "otf");
        assert_eq!(extension(b"  <svg>"), "svg");
        assert_eq!(extension(&[0, 1, 0, 0]), "ttf");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn fonts_are_kept_replaced_and_removed() {
        let dir = std::env::temp_dir().join(format!("locus-font-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let font = include_bytes!("../../locus-text/fonts/figtree/Figtree.ttf");
        assert_eq!(save(&dir, "My/Font", font).ok().as_deref(), Some("MyFont"));
        assert_eq!(
            save(&dir, "My/Font", font).ok().as_deref(),
            Some("MyFont"),
            "replaced"
        );
        let all = load_all(&dir);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, "MyFont");
        assert_eq!(all[0].1.len(), font.len());
        remove(&dir, "MyFont");
        assert!(load_all(&dir).is_empty());
        assert!(save(&dir, "big", &vec![0u8; MAX_FONT_BYTES + 1]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
