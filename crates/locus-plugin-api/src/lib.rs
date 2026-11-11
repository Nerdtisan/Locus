// SPDX-License-Identifier: MIT OR Apache-2.0

//! WIT interface and Rust guest SDK for Locus plugins (SPEC §15, ADR 0009).
//!
//! The interface is `wit/plugin.wit`, package `locus:plugin@1.0.0`: one world
//! per plugin kind, none importing anything. This crate is MIT OR Apache-2.0
//! so plugins don't inherit AGPL obligations (§15.6, ADR 0006).
//!
//! # Writing a plugin
//!
//! A plugin is a `cdylib` built for `wasm32-unknown-unknown` (not
//! `wasm32-wasip2`: WASI imports are refused by the sandbox, §15.4), then
//! packed into a `.locusplugin` with `locus-cli plugin pack`. See
//! `examples/plugin-template/` for a ready-to-copy project.
//!
//! ```ignore
//! use locus_plugin_api::prelude::*;
//!
//! struct Square;
//!
//! impl InfoGuest for Square {
//!     fn describe() -> PluginInfo {
//!         PluginInfo { id: "com.example.square".into(), name: "Square".into(), version: "1.0.0".into() }
//!     }
//! }
//!
//! impl GeneratorGuest for Square {
//!     fn schema() -> Vec<ParamSpec> {
//!         vec![number("size", "Size", "mm", 1.0, 500.0, 20.0)]
//!     }
//!     fn generate(params: Vec<Param>) -> Result<Generated, PluginError> {
//!         let s = Params::new(&params).number("size")?;
//!         let mut sk = Sketch::new();
//!         sk.polygon(&[(0.0, 0.0), (s, 0.0), (s, s), (0.0, s)]);
//!         Ok(sk.finish())
//!     }
//! }
//!
//! locus_plugin_api::export_plugin!(Square: generator);
//! ```
//!
//! The SDK only exists when building for `wasm32`; on other targets this
//! crate is just the WIT files.

#[cfg(target_arch = "wasm32")]
mod bindings;
#[cfg(target_arch = "wasm32")]
pub use bindings::*;

#[cfg(target_arch = "wasm32")]
mod sdk;
#[cfg(target_arch = "wasm32")]
pub use sdk::{Params, Sketch, choice, flag, integer, number};

/// Everything a plugin usually needs.
#[cfg(target_arch = "wasm32")]
pub mod prelude {
    pub use crate::exporter::exports::locus::plugin::exporter::{
        FormatInfo, Guest as ExporterGuest,
    };
    pub use crate::generator::exports::locus::plugin::generator::{
        Constraint, Entity, Generated, Guest as GeneratorGuest,
    };
    pub use crate::info::exports::locus::plugin::info::{Guest as InfoGuest, PluginInfo};
    pub use crate::modifier::exports::locus::plugin::modifier::Guest as ModifierGuest;
    pub use crate::shared::locus::plugin::params::{
        ChoiceSpec, IntegerSpec, NumberSpec, Param, ParamKind, ParamSpec, ParamValue, TextSpec,
    };
    pub use crate::shared::locus::plugin::types::{
        Arc, Circle, Compensation, CutParams, DragKnifeParams, DrawParams, EngraveParams,
        ExportModel, Layer, Line, Operation, Path, PluginError, Segment, Shape, Vec2,
    };
    pub use crate::{Params, Sketch, choice, flag, integer, number};
}

/// Exports a plugin type for the kinds it implements, plus `info`:
/// `export_plugin!(MyPlugin: generator, exporter);`. The type must implement
/// `InfoGuest` and the `…Guest` trait of every kind listed.
#[cfg(target_arch = "wasm32")]
#[macro_export]
macro_rules! export_plugin {
    ($ty:ident : $($kind:ident),+ $(,)?) => {
        $crate::info::export_info!($ty);
        $( $crate::export_plugin!(@kind $ty $kind); )+
    };
    (@kind $ty:ident generator) => { $crate::generator::export_generator!($ty); };
    (@kind $ty:ident modifier) => { $crate::modifier::export_modifier!($ty); };
    (@kind $ty:ident exporter) => { $crate::exporter::export_exporter!($ty); };
}
