// SPDX-License-Identifier: AGPL-3.0-or-later
//! Views (design §7.3): one module per pane. A view keeps only transient
//! widget state (a half-typed name, which row is expanded); everything else
//! lives in [`crate::state::AppState`]. Views never mutate the document
//! directly: they run a `locus_doc::Command` through
//! `AppState::apply_reporting` or run a registry action (SPEC §11).

pub mod browser;
pub mod canvas;
pub mod constraints;
pub mod icons;
pub mod layers;
pub mod marking_menu;
pub mod nav_bar;
pub mod palette;
pub mod parameters;
pub mod properties;
pub mod recipes;
pub mod ribbon;
pub mod shortcuts;
pub mod style;
pub mod text_ui;
pub mod tool_options;
