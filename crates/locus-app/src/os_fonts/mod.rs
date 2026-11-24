// SPDX-License-Identifier: AGPL-3.0-or-later
//! Installed fonts, listed by asking the OS (#477, ADR 0019 decision 7).
//!
//! - **Windows:** DirectWrite's system font collection, which includes
//!   fonts registered for the session: Adobe Creative Cloud fonts appear
//!   while Creative Cloud has them activated. Bytes come through
//!   DirectWrite's own font-file stream.
//! - **macOS:** Core Text's available fonts (also session-registered ones),
//!   read from the URL Core Text gives.
//! - **Linux:** fontconfig through `fc-list`, when it is installed.
//! - **Anywhere the OS lists nothing:** today's folder scan.
//!
//! Each face is listed as its family name for the regular face, or
//! "Family Style" for the others, which is what a document stores. The
//! folder scan still runs so documents saved before #477, which store a
//! font's file name ("arial"), keep loading.
//!
//! Listing runs off the UI thread (S4 measured 25–100 ms on Windows with
//! 531 faces). Font files are only ever read, never copied or embedded
//! (Adobe's licence), and their paths are never logged.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use locus_text::SystemFontSource;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;

/// Matches `locus_text::outline::MAX_OUTLINE_FONT_BYTES`.
pub(crate) const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;

/// One face the OS lists.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Face {
    pub family: String,
    pub style: String,
    pub at: Locator,
}

/// How to read a listed face again.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Locator {
    /// Found again by family and face name through DirectWrite (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    DirectWrite,
    /// A font file, with its face index in a collection when the OS gave
    /// one (fontconfig), or `None` to find the face by name (Core Text).
    #[cfg_attr(windows, allow(dead_code))]
    File { path: PathBuf, index: Option<u32> },
}

/// The name a face is listed and stored under: the family for the regular
/// face, "Family Style" for the others.
pub(crate) fn display_name(family: &str, style: &str) -> String {
    let regular = ["", "regular", "normal", "book", "roman"];
    if regular.contains(&style.trim().to_lowercase().as_str()) {
        family.to_owned()
    } else {
        format!("{family} {style}")
    }
}

/// What the picker lists and how each name loads.
#[derive(Debug, Default)]
pub(crate) struct Listing {
    /// OS faces by display name; the first face listed under a name wins.
    faces: BTreeMap<String, Face>,
    /// The folder scan by file stem: listed only when the OS lists nothing,
    /// and always loadable for documents saved before #477.
    files: BTreeMap<String, PathBuf>,
}

impl Listing {
    pub(crate) fn new(os: Vec<Face>, files: BTreeMap<String, PathBuf>) -> Self {
        let mut faces = BTreeMap::new();
        for f in os {
            if f.family.is_empty() || f.family.chars().any(char::is_control) {
                continue;
            }
            faces.entry(display_name(&f.family, &f.style)).or_insert(f);
        }
        Self { faces, files }
    }

    pub(crate) fn names(&self) -> Vec<String> {
        if self.faces.is_empty() {
            self.files.keys().cloned().collect()
        } else {
            self.faces.keys().cloned().collect()
        }
    }

    fn load(&self, name: &str) -> Option<(Vec<u8>, u32)> {
        if let Some(face) = self.faces.get(name) {
            let loaded = match &face.at {
                #[cfg(windows)]
                Locator::DirectWrite => windows::load(&face.family, &face.style),
                #[cfg(not(windows))]
                Locator::DirectWrite => None,
                Locator::File { path, index } => {
                    let data = read_capped(path)?;
                    let index = index
                        .or_else(|| locus_text::face_index(&data, &face.family, &face.style))
                        .unwrap_or(0);
                    Some((data, index))
                }
            };
            if loaded.is_some() {
                return loaded;
            }
        }
        // A file-stem name from an older document, or a font the OS
        // stopped listing that is still in a font folder.
        read_capped(self.files.get(name)?).map(|d| (d, 0))
    }
}

/// A font file's bytes, if it is within the size cap and readable.
pub(crate) fn read_capped(path: &std::path::Path) -> Option<Vec<u8>> {
    if std::fs::metadata(path).ok()?.len() > MAX_FONT_BYTES {
        return None;
    }
    std::fs::read(path).ok()
}

/// The OS's faces; empty where the OS can't be asked.
fn os_faces() -> Vec<Face> {
    #[cfg(windows)]
    return windows::faces();
    #[cfg(target_os = "macos")]
    return macos::faces();
    #[cfg(target_os = "linux")]
    return linux::faces();
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    Vec::new()
}

/// The desktop's [`SystemFontSource`]: the OS list merged with the folder
/// scan, built on a worker thread and waited for on first use.
pub struct SystemFonts {
    listing: OnceLock<Listing>,
    pending: Mutex<Option<JoinHandle<Listing>>>,
}

impl SystemFonts {
    /// Starts listing in the background. `scan` is the folder scan.
    pub fn start(scan: impl FnOnce() -> BTreeMap<String, PathBuf> + Send + 'static) -> Self {
        let job = std::thread::Builder::new()
            .name("locus-fonts".to_owned())
            .spawn(move || Listing::new(os_faces(), scan()));
        if let Err(e) = &job {
            tracing::warn!("couldn't start the font listing thread: {}", e.kind());
        }
        Self {
            listing: OnceLock::new(),
            pending: Mutex::new(job.ok()),
        }
    }

    fn listing(&self) -> &Listing {
        self.listing.get_or_init(|| {
            let job = self.pending.lock().ok().and_then(|mut g| g.take());
            job.and_then(|j| j.join().ok()).unwrap_or_default()
        })
    }
}

impl SystemFontSource for SystemFonts {
    fn families(&self) -> Vec<String> {
        self.listing().names()
    }

    fn load(&self, family: &str) -> Option<Vec<u8>> {
        self.load_face(family).map(|(d, _)| d)
    }

    fn load_face(&self, family: &str) -> Option<(Vec<u8>, u32)> {
        self.listing().load(family)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn face(family: &str, style: &str) -> Face {
        Face {
            family: family.to_owned(),
            style: style.to_owned(),
            at: Locator::File {
                path: PathBuf::from(format!("/nowhere/{family}-{style}.ttf")),
                index: Some(0),
            },
        }
    }

    #[test]
    fn regular_faces_are_listed_by_family_and_others_with_their_style() {
        assert_eq!(display_name("Arial", "Regular"), "Arial");
        assert_eq!(display_name("Source Serif", "Book"), "Source Serif");
        assert_eq!(display_name("Arial", "Bold Italic"), "Arial Bold Italic");
    }

    #[test]
    fn the_os_list_wins_and_duplicates_collapse() {
        let files = BTreeMap::from([
            ("arial".to_owned(), PathBuf::from("/fonts/arial.ttf")),
            ("arialbd".to_owned(), PathBuf::from("/fonts/arialbd.ttf")),
        ]);
        let os = vec![
            face("Arial", "Regular"),
            face("Arial", "Bold"),
            // A second file for the same face: the first listed wins.
            Face {
                at: Locator::File {
                    path: PathBuf::from("/elsewhere/arial.ttf"),
                    index: Some(0),
                },
                ..face("Arial", "Regular")
            },
            // Nameless or control-character names are never listed.
            face("", "Regular"),
            face("Bad\u{7}", "Regular"),
        ];
        let l = Listing::new(os, files);
        assert_eq!(l.names(), ["Arial", "Arial Bold"]);
        assert_eq!(l.faces.get("Arial"), Some(&face("Arial", "Regular")));
    }

    #[test]
    fn the_folder_scan_is_listed_when_the_os_lists_nothing() {
        let files = BTreeMap::from([("arial".to_owned(), PathBuf::from("/fonts/arial.ttf"))]);
        assert_eq!(Listing::new(Vec::new(), files).names(), ["arial"]);
    }

    #[test]
    fn missing_fonts_load_nothing_and_never_panic() {
        let l = Listing::new(vec![face("Ghost", "Regular")], BTreeMap::new());
        assert!(l.load("Ghost").is_none());
        assert!(l.load("not listed").is_none());
    }

    #[test]
    fn a_listed_file_loads_with_its_face_index() {
        let dir = std::env::temp_dir().join(format!("locus-osfonts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.ttf");
        std::fs::write(&path, b"bytes").unwrap();
        let mut f = face("Mine", "Regular");
        f.at = Locator::File {
            path: path.clone(),
            index: Some(2),
        };
        let files = BTreeMap::from([("mine-old".to_owned(), path)]);
        let l = Listing::new(vec![f], files);
        assert_eq!(l.load("Mine"), Some((b"bytes".to_vec(), 2)));
        // The pre-#477 file-stem name still loads.
        assert_eq!(l.load("mine-old"), Some((b"bytes".to_vec(), 0)));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
