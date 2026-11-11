// SPDX-License-Identifier: AGPL-3.0-or-later

//! `.locusplugin` archives (§15.7, #182): a zip with exactly these entries at
//! its root:
//!
//! | entry         | required | limit                  |
//! |---------------|----------|------------------------|
//! | `plugin.toml` | yes      | 64 KiB                 |
//! | `plugin.wasm` | yes      | `MAX_COMPONENT_BYTES`  |
//! | `icon.png`    | no       | 1 MiB                  |
//! | `README.md`   | no       | 256 KiB                |
//!
//! The archive is untrusted: anything else in it (other files, directories,
//! paths, duplicates, encryption) is refused, and every entry is read through
//! a hard byte limit so a lying size header or a zip bomb can't exhaust
//! memory. The host reads the file; this module only sees bytes (§15.4).

use std::io::{Cursor, Read, Write};

use sha2::{Digest, Sha256};

use crate::manifest::{MAX_MANIFEST_BYTES, Manifest, ManifestError};
use crate::runtime::MAX_COMPONENT_BYTES;

/// Largest `.locusplugin` file accepted.
pub const MAX_ARCHIVE_BYTES: usize = 80 * 1024 * 1024;
const MAX_ICON_BYTES: usize = 1024 * 1024;
const MAX_README_BYTES: usize = 256 * 1024;
/// More entries than this and it isn't a plugin archive.
const MAX_ENTRIES: usize = 16;

pub const MANIFEST: &str = "plugin.toml";
pub const WASM: &str = "plugin.wasm";
pub const ICON: &str = "icon.png";
pub const README: &str = "README.md";

/// A parsed archive: the checked manifest and the raw parts.
#[derive(Clone, PartialEq, Eq)]
pub struct Archive {
    pub manifest: Manifest,
    /// `plugin.toml` as written, so an install stores exactly what was checked.
    pub manifest_toml: String,
    pub wasm: Vec<u8>,
    pub icon: Option<Vec<u8>>,
    pub readme: Option<String>,
}

impl std::fmt::Debug for Archive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Archive")
            .field("manifest", &self.manifest)
            .field("wasm_bytes", &self.wasm.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("the plugin file is larger than {} MB", MAX_ARCHIVE_BYTES / (1024 * 1024))]
    TooLarge,
    #[error("not a .locusplugin archive: {0}")]
    NotAnArchive(String),
    #[error("the archive contains `{0}`, which doesn't belong in a plugin")]
    UnexpectedEntry(String),
    #[error("the archive contains `{0}` twice")]
    Duplicate(String),
    #[error("the archive is missing `{0}`")]
    Missing(&'static str),
    #[error("`{0}` in the archive is too large")]
    EntryTooLarge(&'static str),
    #[error("`{0}` in the archive is encrypted or uses an unsupported compression")]
    Unsupported(String),
    #[error("{README} is not UTF-8 text")]
    ReadmeNotText,
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("plugin.wasm doesn't match the SHA-256 in plugin.toml")]
    Checksum,
}

/// Lower-case hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn limit_for(name: &str) -> Option<(&'static str, usize)> {
    match name {
        MANIFEST => Some((MANIFEST, MAX_MANIFEST_BYTES)),
        WASM => Some((WASM, MAX_COMPONENT_BYTES)),
        ICON => Some((ICON, MAX_ICON_BYTES)),
        README => Some((README, MAX_README_BYTES)),
        _ => None,
    }
}

/// Entry names shown in errors are untrusted: keep them short and printable.
fn shown(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(80).collect()
}

impl Archive {
    /// Parses and checks an archive. Does not run the plugin: see
    /// [`crate::registry`] for the full install check.
    pub fn parse(bytes: &[u8]) -> Result<Archive, ArchiveError> {
        if bytes.len() > MAX_ARCHIVE_BYTES {
            return Err(ArchiveError::TooLarge);
        }
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| ArchiveError::NotAnArchive(e.to_string()))?;
        if zip.len() > MAX_ENTRIES {
            return Err(ArchiveError::NotAnArchive("too many entries".into()));
        }

        let (mut manifest, mut wasm, mut icon, mut readme) = (None, None, None, None);
        for i in 0..zip.len() {
            let mut entry = zip
                .by_index(i)
                .map_err(|e| ArchiveError::Unsupported(e.to_string()))?;
            let name = entry
                .name()
                .map_err(|_| ArchiveError::Unsupported("an entry name".into()))?
                .into_owned();
            let Some((known, limit)) = limit_for(&name) else {
                return Err(ArchiveError::UnexpectedEntry(shown(&name)));
            };
            if entry.is_dir() || entry.encrypted() {
                return Err(ArchiveError::Unsupported(shown(&name)));
            }
            if entry.size() > limit as u64 {
                return Err(ArchiveError::EntryTooLarge(known));
            }
            // Read through a hard limit: the size header may lie.
            let mut data = Vec::new();
            (&mut entry)
                .take(limit as u64 + 1)
                .read_to_end(&mut data)
                .map_err(|e| ArchiveError::Unsupported(format!("{known}: {e}")))?;
            if data.len() > limit {
                return Err(ArchiveError::EntryTooLarge(known));
            }
            let slot = match known {
                MANIFEST => &mut manifest,
                WASM => &mut wasm,
                ICON => &mut icon,
                _ => &mut readme,
            };
            if slot.replace(data).is_some() {
                return Err(ArchiveError::Duplicate(known.into()));
            }
        }

        let manifest = manifest.ok_or(ArchiveError::Missing(MANIFEST))?;
        let manifest_toml =
            String::from_utf8(manifest).map_err(|_| ManifestError::Syntax("not UTF-8".into()))?;
        let manifest = Manifest::parse(&manifest_toml)?;
        let wasm = wasm.ok_or(ArchiveError::Missing(WASM))?;
        if let Some(expected) = &manifest.sha256
            && sha256_hex(&wasm) != *expected
        {
            return Err(ArchiveError::Checksum);
        }
        let readme = readme
            .map(|r| String::from_utf8(r).map_err(|_| ArchiveError::ReadmeNotText))
            .transpose()?;
        Ok(Archive {
            manifest,
            manifest_toml,
            wasm,
            icon,
            readme,
        })
    }

    /// Writes a `.locusplugin` from a manifest and a component (used by
    /// `locus-cli plugin pack` and tests). Entries are stored in a fixed
    /// order with fixed timestamps, so the same inputs give the same bytes.
    pub fn write(
        manifest_toml: &str,
        wasm: &[u8],
        icon: Option<&[u8]>,
        readme: Option<&str>,
    ) -> Result<Vec<u8>, ArchiveError> {
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default());
        let mut put = |name: &str, data: &[u8]| -> Result<(), ArchiveError> {
            out.start_file(name, opts)
                .and_then(|()| out.write_all(data).map_err(Into::into))
                .map_err(|e| ArchiveError::NotAnArchive(e.to_string()))
        };
        put(MANIFEST, manifest_toml.as_bytes())?;
        put(WASM, wasm)?;
        if let Some(icon) = icon {
            put(ICON, icon)?;
        }
        if let Some(readme) = readme {
            put(README, readme.as_bytes())?;
        }
        let bytes = out
            .finish()
            .map_err(|e| ArchiveError::NotAnArchive(e.to_string()))?
            .into_inner();
        // Round-trip through the reader so `write` never emits an archive
        // that `parse` would refuse.
        Archive::parse(&bytes)?;
        Ok(bytes)
    }
}
