// SPDX-License-Identifier: AGPL-3.0-or-later

//! Validation pipeline and SVG/PDF/DXF writers, kerf/tool offsets (SPEC §8). Core crate: returns bytes, never touches the filesystem; must build for wasm32.

// Tests may unwrap; library code may not (CLAUDE.md, Code Style).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod svg;

pub use svg::to_svg;
