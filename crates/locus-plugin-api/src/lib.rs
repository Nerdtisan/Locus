// SPDX-License-Identifier: MIT OR Apache-2.0

//! WIT interface definitions and Rust guest SDK for plugin authors (SPEC §15).
//!
//! The interface is `wit/plugin.wit`, package `locus:plugin@1.0.0` (ADR 0009):
//! one world per plugin kind, none importing anything. Draft until the M3
//! plugin host ships. Licensed MIT OR Apache-2.0 so plugins don't inherit AGPL
//! obligations (§15.6). Must build for wasm32.
