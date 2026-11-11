// SPDX-License-Identifier: AGPL-3.0-or-later

//! Resolving a document's font reference to a loaded font (SPEC §9, §10).
//!
//! A `.locus` file never embeds font files (licences often forbid it), so it
//! stores a [`FontRef`] plus cached glyph geometry. The [`FontLibrary`] turns a
//! reference back into a font where one is available: bundled fonts always,
//! user-loaded files by name, and installed system fonts by family on desktop
//! (never on the web, which cannot list them). Discovering system fonts needs
//! the filesystem, so it lives in the app shell behind [`SystemFontSource`].

use crate::bundled::BundledFont;
use crate::error::TextError;
use crate::outline::OutlineFont;
use crate::stroke::StrokeFont;
use crate::Font;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Longest font name/family stored in a document, in characters.
pub const MAX_FONT_NAME: usize = 256;

/// How a document refers to a font (SPEC §9; #179).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FontRef {
    /// A font shipped with Locus, by [`BundledFont::id`].
    Bundled(String),
    /// A font file the user loaded, by the name it was loaded under.
    File(String),
    /// An installed system font, by family name (desktop only).
    System(String),
}

impl FontRef {
    #[must_use]
    pub fn bundled(f: BundledFont) -> Self {
        FontRef::Bundled(f.id().to_owned())
    }

    /// The name shown in the UI.
    #[must_use]
    pub fn display_name(&self) -> String {
        match self {
            FontRef::Bundled(id) => BundledFont::from_id(id)
                .map_or_else(|| id.clone(), |f| f.display_name().to_owned()),
            FontRef::File(n) | FontRef::System(n) => n.clone(),
        }
    }

    /// Is the reference well formed (bounded, non-empty)?
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let n = match self {
            FontRef::Bundled(n) | FontRef::File(n) | FontRef::System(n) => n,
        };
        !n.is_empty() && n.chars().count() <= MAX_FONT_NAME && !n.chars().any(char::is_control)
    }
}

/// Supplies installed system fonts. Implemented by the desktop app shell;
/// the web shell supplies none.
pub trait SystemFontSource {
    /// Installed family names, sorted, for the font picker.
    fn families(&self) -> Vec<String>;
    /// The bytes of the regular face of `family`, if installed.
    fn load(&self, family: &str) -> Option<Vec<u8>>;
}

/// Anything that can turn a [`FontRef`] into a font.
pub trait FontProvider {
    fn resolve(&self, font: &FontRef) -> Option<Font>;
}

/// Bundled fonts only: what a headless client or the CLI has.
#[derive(Debug, Clone, Copy, Default)]
pub struct BundledProvider;

impl FontProvider for BundledProvider {
    fn resolve(&self, font: &FontRef) -> Option<Font> {
        match font {
            FontRef::Bundled(id) => BundledFont::from_id(id)?.load().ok(),
            FontRef::File(_) | FontRef::System(_) => None,
        }
    }
}

/// Bundled fonts + user-loaded files + (optionally) system fonts.
#[derive(Default)]
pub struct FontLibrary {
    user: BTreeMap<String, Font>,
    system: Option<Box<dyn SystemFontSource>>,
    system_cache: std::cell::RefCell<BTreeMap<String, Option<Font>>>,
}

impl std::fmt::Debug for FontLibrary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontLibrary")
            .field("user", &self.user.keys().collect::<Vec<_>>())
            .field("has_system", &self.system.is_some())
            .finish_non_exhaustive()
    }
}

impl FontLibrary {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enables system fonts (desktop shells).
    pub fn set_system_source(&mut self, source: Box<dyn SystemFontSource>) {
        self.system = Some(source);
        self.system_cache.borrow_mut().clear();
    }

    #[must_use]
    pub fn has_system_fonts(&self) -> bool {
        self.system.is_some()
    }

    /// Installed system families for the picker; empty without a source.
    #[must_use]
    pub fn system_families(&self) -> Vec<String> {
        self.system.as_ref().map(|s| s.families()).unwrap_or_default()
    }

    /// Names of the fonts loaded from user files.
    pub fn user_fonts(&self) -> impl Iterator<Item = &str> {
        self.user.keys().map(String::as_str)
    }

    /// Loads a user font file (`.ttf`/`.otf`, or an SVG single-line font when
    /// `svg_text` is given) under `name`, replacing a same-named font.
    ///
    /// # Errors
    /// [`TextError`] for an unusable font or name.
    pub fn add_user_font_bytes(&mut self, name: &str, data: Vec<u8>) -> Result<FontRef, TextError> {
        let r = FontRef::File(name.to_owned());
        if !r.is_valid() {
            return Err(TextError::BadOutlineFont);
        }
        let font = match std::str::from_utf8(&data) {
            // SVG fonts are text; TrueType/OpenType data starts with a binary tag.
            Ok(text) if text.trim_start().starts_with(['<', '\u{feff}']) => {
                Font::Stroke(Arc::new(StrokeFont::from_svg_font(name, text)?))
            }
            _ => Font::Outline(OutlineFont::from_bytes(name, data)?),
        };
        self.user.insert(name.to_owned(), font);
        Ok(r)
    }
}

impl FontProvider for FontLibrary {
    fn resolve(&self, font: &FontRef) -> Option<Font> {
        match font {
            FontRef::Bundled(_) => BundledProvider.resolve(font),
            FontRef::File(n) => self.user.get(n).cloned(),
            FontRef::System(family) => {
                let source = self.system.as_ref()?;
                self.system_cache
                    .borrow_mut()
                    .entry(family.clone())
                    .or_insert_with(|| {
                        let bytes = source.load(family)?;
                        OutlineFont::from_bytes(family, bytes).ok().map(Font::Outline)
                    })
                    .clone()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_resolves_others_do_not() {
        let lib = FontLibrary::new();
        for f in BundledFont::ALL {
            assert!(lib.resolve(&FontRef::bundled(f)).is_some());
        }
        assert!(lib.resolve(&FontRef::Bundled("nope".into())).is_none());
        assert!(lib.resolve(&FontRef::File("x".into())).is_none());
        assert!(lib.resolve(&FontRef::System("Arial".into())).is_none());
    }

    #[test]
    fn user_fonts_by_name_ttf_and_svg() {
        let mut lib = FontLibrary::new();
        let ttf = include_bytes!("../fonts/figtree/Figtree.ttf").to_vec();
        let r = lib.add_user_font_bytes("Mine", ttf).unwrap();
        assert!(!lib.resolve(&r).unwrap().is_single_line());
        let svg = br#"<font horiz-adv-x="500"><font-face units-per-em="1000"/><glyph unicode="L" d="M0,700 L0,0 L400,0"/></font>"#;
        let r2 = lib.add_user_font_bytes("Stroke", svg.to_vec()).unwrap();
        assert!(lib.resolve(&r2).unwrap().is_single_line());
        assert!(lib.add_user_font_bytes("Bad", b"junk".to_vec()).is_err());
        assert!(lib.add_user_font_bytes("", b"junk".to_vec()).is_err());
        assert_eq!(lib.user_fonts().collect::<Vec<_>>(), ["Mine", "Stroke"]);
    }

    struct Fake;
    impl SystemFontSource for Fake {
        fn families(&self) -> Vec<String> {
            vec!["Fakey".into()]
        }
        fn load(&self, family: &str) -> Option<Vec<u8>> {
            (family == "Fakey").then(|| include_bytes!("../fonts/figtree/Figtree.ttf").to_vec())
        }
    }

    #[test]
    fn system_fonts_only_through_the_source() {
        let mut lib = FontLibrary::new();
        assert!(lib.system_families().is_empty());
        lib.set_system_source(Box::new(Fake));
        assert_eq!(lib.system_families(), ["Fakey"]);
        assert!(lib.resolve(&FontRef::System("Fakey".into())).is_some());
        assert!(lib.resolve(&FontRef::System("Missing".into())).is_none());
    }
}
