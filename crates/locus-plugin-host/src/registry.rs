// SPDX-License-Identifier: AGPL-3.0-or-later

//! Installed plugins (§15.7, #182).
//!
//! A registry is a folder the app owns (the GUI picks it, e.g. under the
//! user's data directory). Each plugin lives in `<folder>/<id>/` with the
//! archive's files. Installing checks everything first: the archive, the
//! manifest, the component (by loading it in the sandbox) and that the
//! component agrees with its manifest. Only then is anything written, and the
//! swap into place is a rename, so a failed install never leaves a half
//! plugin behind or damages the one already installed.
//!
//! Plugins are never auto-updated (§15.7); installing a new version is an
//! explicit act. Signing is later (§15.4): today every plugin is unsigned and
//! labelled so.
//!
//! A plugin can be disabled without uninstalling it (#235): a `disabled`
//! marker file in its folder, kept across reinstalls. A disabled plugin is
//! listed but never loaded, so documents that use it open from their cached
//! geometry with the "plugin missing" badge (§15.5).

use std::fs;
use std::path::{Path, PathBuf};

use crate::archive::{self, Archive, ArchiveError, sha256_hex};
use crate::manifest::{Manifest, api_major};
use crate::runtime::{Host, LoadError, Plugin};
use crate::validate::is_reverse_dns;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Archive(#[from] ArchiveError),
    #[error(transparent)]
    Load(#[from] LoadError),
    /// The component and its manifest disagree.
    #[error("the plugin doesn't match its manifest: {0}")]
    Mismatch(String),
    #[error("no plugin with that id is installed")]
    NotInstalled,
    #[error("the plugin is disabled")]
    Disabled,
    #[error("couldn't update the plugin folder: {0}")]
    Io(String),
}

fn io(e: std::io::Error) -> InstallError {
    // The error kind, not the path: paths are never shown or logged.
    InstallError::Io(e.kind().to_string())
}

/// An installed plugin, as listed (not loaded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub manifest: Manifest,
    pub dir: PathBuf,
    /// Always false until signing lands (§15.4); the UI labels these.
    pub signed: bool,
    /// False once disabled in the plugin manager (#235).
    pub enabled: bool,
}

impl Installed {
    /// What the UI shows next to the plugin's name.
    pub fn trust_label(&self) -> &'static str {
        if self.signed { "Signed" } else { "Unsigned" }
    }

    /// The capabilities granted at install, for the plugin manager. Locus v1
    /// grants none (§15.4), and the manifest check refuses any request.
    pub fn capabilities(&self) -> &'static [&'static str] {
        &[]
    }
}

/// Marker file of a disabled plugin.
const DISABLED: &str = "disabled";
/// Largest README shown (the archive limit).
const MAX_README_BYTES: usize = 256 * 1024;

/// Loads an archive's component and checks it against the manifest.
pub fn verify(host: &Host, archive: &Archive) -> Result<Plugin, InstallError> {
    let m = &archive.manifest;
    let plugin = host.load(&archive.wasm)?;
    let info = plugin.info();
    if info.id != m.id {
        return Err(InstallError::Mismatch(
            "the plugin reports a different id".into(),
        ));
    }
    if info.version != m.version {
        return Err(InstallError::Mismatch(
            "the plugin reports a different version".into(),
        ));
    }
    if api_major(plugin.api_version()) != api_major(&m.api) {
        return Err(InstallError::Mismatch(
            "the plugin was built against a different API major".into(),
        ));
    }
    if m.kinds != plugin.kinds() {
        return Err(InstallError::Mismatch(
            "the kinds in plugin.toml aren't the ones the plugin provides".into(),
        ));
    }
    Ok(plugin)
}

#[derive(Debug, Clone)]
pub struct Registry {
    dir: PathBuf,
}

impl Registry {
    /// A registry in `dir`, which is created when the first plugin is installed.
    pub fn new(dir: impl Into<PathBuf>) -> Registry {
        Registry { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn plugin_dir(&self, id: &str) -> Result<PathBuf, InstallError> {
        // Ids are reverse-DNS (no separators, no `..`), so this stays inside.
        if !is_reverse_dns(id) {
            return Err(InstallError::NotInstalled);
        }
        Ok(self.dir.join(id))
    }

    /// Checks and installs a `.locusplugin`, replacing any installed version.
    pub fn install(&self, host: &Host, bytes: &[u8]) -> Result<Installed, InstallError> {
        let archive = Archive::parse(bytes)?;
        verify(host, &archive)?;
        let id = archive.manifest.id.clone();
        let target = self.plugin_dir(&id)?;
        let staging = self.dir.join(format!(".staging-{id}"));
        let old = self.dir.join(format!(".old-{id}"));

        fs::create_dir_all(&self.dir).map_err(io)?;
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_dir_all(&old);
        let written = (|| {
            fs::create_dir(&staging)?;
            fs::write(staging.join(archive::WASM), &archive.wasm)?;
            if let Some(icon) = &archive.icon {
                fs::write(staging.join(archive::ICON), icon)?;
            }
            if let Some(readme) = &archive.readme {
                fs::write(staging.join(archive::README), readme)?;
            }
            // Reinstalling keeps a plugin disabled if it was.
            if target.join(DISABLED).exists() {
                fs::write(staging.join(DISABLED), b"")?;
            }
            fs::write(staging.join(archive::MANIFEST), &archive.manifest_toml)
        })();
        if let Err(e) = written {
            let _ = fs::remove_dir_all(&staging);
            return Err(io(e));
        }
        if target.exists() {
            fs::rename(&target, &old).map_err(io)?;
        }
        if let Err(e) = fs::rename(&staging, &target) {
            // Put the previous version back.
            let _ = fs::rename(&old, &target);
            let _ = fs::remove_dir_all(&staging);
            return Err(io(e));
        }
        let _ = fs::remove_dir_all(&old);
        let enabled = !target.join(DISABLED).exists();
        Ok(Installed {
            manifest: archive.manifest,
            dir: target,
            signed: false,
            enabled,
        })
    }

    /// Enables or disables an installed plugin without uninstalling it.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), InstallError> {
        let dir = self.plugin_dir(id)?;
        if !dir.is_dir() {
            return Err(InstallError::NotInstalled);
        }
        let marker = dir.join(DISABLED);
        if enabled {
            match fs::remove_file(marker) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io(e)),
                _ => Ok(()),
            }
        } else {
            fs::write(marker, b"").map_err(io)
        }
    }

    /// The plugin's README as text, if it has one (at most 256 KiB, UTF-8).
    /// Shown as plain text: never rendered as HTML or followed as links.
    pub fn readme(&self, id: &str) -> Option<String> {
        let dir = self.plugin_dir(id).ok()?;
        let bytes = read_limited(&dir.join(archive::README), MAX_README_BYTES)?;
        String::from_utf8(bytes).ok()
    }

    /// The installed plugins, sorted by id. Folders that don't hold a valid
    /// plugin are skipped (a half-deleted folder must not break the list).
    pub fn list(&self) -> Vec<Installed> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<Installed> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                is_reverse_dns(&name).then_some(())?;
                let installed = self.get(&name).ok()?;
                (installed.manifest.id == name).then_some(installed)
            })
            .collect();
        out.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        out
    }

    /// Reads one installed plugin's manifest.
    pub fn get(&self, id: &str) -> Result<Installed, InstallError> {
        let dir = self.plugin_dir(id)?;
        let src = read_limited(
            &dir.join(archive::MANIFEST),
            crate::manifest::MAX_MANIFEST_BYTES,
        )
        .ok_or(InstallError::NotInstalled)?;
        let src = String::from_utf8(src).map_err(|_| InstallError::NotInstalled)?;
        let manifest = Manifest::parse(&src).map_err(ArchiveError::from)?;
        let enabled = !dir.join(DISABLED).exists();
        Ok(Installed {
            manifest,
            dir,
            signed: false,
            enabled,
        })
    }

    fn manifest_toml(&self, id: &str) -> Option<String> {
        let dir = self.plugin_dir(id).ok()?;
        let src = read_limited(
            &dir.join(archive::MANIFEST),
            crate::manifest::MAX_MANIFEST_BYTES,
        )?;
        String::from_utf8(src).ok()
    }

    /// Loads an installed, enabled plugin, re-checking it: the files on disk
    /// are as untrusted as the archive was.
    pub fn load(&self, host: &Host, id: &str) -> Result<(Installed, Plugin), InstallError> {
        let installed = self.get(id)?;
        if !installed.enabled {
            return Err(InstallError::Disabled);
        }
        let wasm = read_limited(
            &installed.dir.join(archive::WASM),
            crate::runtime::MAX_COMPONENT_BYTES,
        )
        .ok_or(InstallError::NotInstalled)?;
        if let Some(expected) = &installed.manifest.sha256
            && sha256_hex(&wasm) != *expected
        {
            return Err(ArchiveError::Checksum.into());
        }
        let archive = Archive {
            manifest: installed.manifest.clone(),
            manifest_toml: self.manifest_toml(id).unwrap_or_default(),
            wasm,
            icon: None,
            readme: None,
        };
        let plugin = verify(host, &archive)?;
        Ok((installed, plugin))
    }

    pub fn uninstall(&self, id: &str) -> Result<(), InstallError> {
        let dir = self.plugin_dir(id)?;
        if !dir.is_dir() {
            return Err(InstallError::NotInstalled);
        }
        fs::remove_dir_all(dir).map_err(io)
    }
}

/// Reads at most `limit` bytes; `None` if missing, unreadable or larger.
fn read_limited(path: &Path, limit: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut data = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(limit as u64 + 1)
        .read_to_end(&mut data)
        .ok()?;
    (data.len() <= limit).then_some(data)
}
