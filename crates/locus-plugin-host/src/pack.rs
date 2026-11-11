// SPDX-License-Identifier: AGPL-3.0-or-later

//! Packing a plugin build into a `.locusplugin` (#185, §15.7).
//!
//! A Rust plugin is built for `wasm32-unknown-unknown`, which gives a *core*
//! module carrying `wit-bindgen`'s component-type metadata. [`componentize`]
//! turns that into a component with `wit-component`, as `cargo component`
//! would; [`pack`] then checks the result in the sandbox against its manifest
//! and writes the archive. Packing never accepts what installing would refuse.

use crate::archive::{Archive, ArchiveError};
use crate::manifest::Manifest;
use crate::registry::{InstallError, verify};
use crate::runtime::{Host, MAX_COMPONENT_BYTES};

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("the build output is larger than {MAX_COMPONENT_BYTES} bytes")]
    TooLarge,
    #[error("not a WebAssembly module or component")]
    NotWasm,
    #[error("couldn't make a component from the module: {0}")]
    Componentize(String),
    #[error(transparent)]
    Archive(#[from] ArchiveError),
    #[error(transparent)]
    Check(#[from] InstallError),
}

/// Wraps a core module built with `wit-bindgen` into a component. A
/// component is returned unchanged.
pub fn componentize(wasm: &[u8]) -> Result<Vec<u8>, PackError> {
    if wasm.len() > MAX_COMPONENT_BYTES {
        return Err(PackError::TooLarge);
    }
    // Preamble: magic, then version 1 (core module) or a component layer.
    match wasm.get(..8) {
        Some([0, b'a', b's', b'm', 1, 0, 0, 0]) => {}
        Some([0, b'a', b's', b'm', _, _, 1, 0]) => return Ok(wasm.to_vec()),
        _ => return Err(PackError::NotWasm),
    }
    wit_component::ComponentEncoder::default()
        .module(wasm)
        .and_then(|e| e.validate(true).encode())
        .map_err(|e| PackError::Componentize(format!("{e:#}")))
}

/// Builds a `.locusplugin` from a manifest and a build output (module or
/// component), after loading it in the sandbox and checking it against the
/// manifest exactly as an install would.
pub fn pack(
    host: &Host,
    manifest_toml: &str,
    wasm: &[u8],
    readme: Option<&str>,
    icon: Option<&[u8]>,
) -> Result<Vec<u8>, PackError> {
    let manifest = Manifest::parse(manifest_toml).map_err(ArchiveError::from)?;
    let component = componentize(wasm)?;
    verify(
        host,
        &Archive {
            manifest,
            manifest_toml: manifest_toml.to_owned(),
            wasm: component.clone(),
            icon: None,
            readme: None,
        },
    )?;
    Ok(Archive::write(manifest_toml, &component, icon, readme)?)
}
