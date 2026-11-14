// SPDX-License-Identifier: AGPL-3.0-or-later
//! View state (design §7.3): zoom and pan, auto-fit, the canvas rectangle,
//! preview toggles and their caches, label rectangles from the last frame,
//! and touch input. Never part of the document.

use crate::view::View;
use crate::views::canvas::MachinePath;
use egui::Pos2;
use locus_doc::{ConstraintId, RecipeId};

pub struct ViewState {
    /// World ↔ screen transform.
    pub(crate) camera: View,
    /// Keep the view fitted to the content, re-fitting when the canvas
    /// resizes, until the user pans or zooms (the web canvas is not at its
    /// final size on the first frame).
    pub(crate) auto_fit: bool,
    pub(crate) fit_size: egui::Vec2,
    /// Items drawn last frame (status bar).
    pub(crate) drawn: usize,
    /// Pick radius in points for this frame: coarse only for finger input.
    pub(crate) tol_pt: f64,
    /// The canvas rectangle and whether the pointer was over it, last frame.
    pub(crate) rect: egui::Rect,
    pub(crate) pointer_over: bool,
    /// Machine preview on, and its polylines (#176).
    pub(crate) machine_on: bool,
    pub(crate) machine: Vec<MachinePath>,
    /// Draw pen width / kerf at true scale under the sketch (#240).
    pub(crate) stroke_on: bool,
    /// Screen rectangles of the dimension labels drawn last frame.
    pub(crate) dim_rects: Vec<(ConstraintId, egui::Rect)>,
    /// Screen rectangles of the offset / pattern labels drawn last frame (#164).
    pub(crate) offset_rects: Vec<(RecipeId, egui::Rect)>,
    /// Screen position where a rubber-band box started.
    pub(crate) rubber: Option<Pos2>,
    /// Live touch/pen contacts (#250).
    pub(crate) touches: crate::touch::Touches,
    /// On-screen Shift (add) and Ctrl (toggle) for touch selection (#251).
    pub(crate) touch_add: bool,
    pub(crate) touch_toggle: bool,
    /// The open marking menu (design §4, #290).
    pub(crate) marking: Option<Marking>,
    /// Requests for the shell (show the shortcuts pane, reset the layout):
    /// it owns the dock, so it takes these each frame.
    pub(crate) shell: Vec<crate::actions::Shell>,
}

/// A marking menu opened by right-click or a long press (design §4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marking {
    /// Centre of the ring, in screen points.
    pub at: Pos2,
    /// The right button is still down: releasing away from the centre runs
    /// the slot in that direction (a flick).
    pub held: bool,
    /// When it opened (egui time, s): a flick doesn't draw the menu.
    pub since: f64,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            camera: View::default(),
            auto_fit: true,
            fit_size: egui::Vec2::ZERO,
            drawn: 0,
            tol_pt: super::HIT_TOL_PT,
            rect: egui::Rect::NOTHING,
            pointer_over: false,
            machine_on: false,
            machine: Vec::new(),
            stroke_on: false,
            dim_rects: Vec::new(),
            offset_rects: Vec::new(),
            rubber: None,
            touches: crate::touch::Touches::default(),
            touch_add: false,
            touch_toggle: false,
            marking: None,
            shell: Vec::new(),
        }
    }
}
