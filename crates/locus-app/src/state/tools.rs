// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tool state (design §7.3): the active drawing or modify tool and its
//! in-progress input. The `Tool` trait and `ToolHost` (§7.2) replace this
//! with one boxed tool.

use crate::constrain::DimSpec;
use crate::modify_tool::{CornerSettings, Modify, TransformSettings};
use crate::tools::{Draft, Pick, Tool};
use crate::views::text_ui::{FontCatalog, TextSettings};
use locus_doc::{ConstraintId, EntityId, RecipeId};
use locus_geom::Vec2;

/// What an inline dimension editor will do on Enter (#70).
#[derive(Clone, Copy, Debug)]
pub enum DimTarget {
    New(DimSpec),
    Existing(ConstraintId),
    /// A live offset's distance (#164).
    Offset(RecipeId),
    /// A pattern's count and spacing / angle (#168, #169).
    Pattern(RecipeId),
}

/// The inline value box shown over the canvas while a dimension is typed.
#[derive(Debug)]
pub struct DimEdit {
    pub target: DimTarget,
    pub what: &'static str,
    pub angle: bool,
    pub text: String,
    /// World position the box hangs from.
    pub at: Vec2,
    /// First frame: take focus and select the text.
    pub fresh: bool,
}

pub struct ToolState {
    /// The active drawing tool and its clicks so far (#68).
    pub(crate) draft: Draft,
    /// Pen tool: where the button went down and where the pointer is now
    /// while dragging out a handle (#160).
    pub(crate) pen: Option<(Pick, Vec2)>,
    /// Digits typed while drawing: a length or radius (SPEC §6.1).
    pub(crate) typed: String,
    /// Last cursor position over the canvas, snapped to a point if near one.
    pub(crate) cursor: Option<Pick>,
    /// The inline dimension editor, while open (#70).
    pub(crate) dim_edit: Option<DimEdit>,
    /// A point drag is in progress (#71): frames re-solve, release records.
    pub(crate) dragging: bool,
    /// The active modify tool, if any (SPEC §6.2): clicks go to it.
    pub(crate) modify: Option<Modify>,
    /// The offset picked with the Offset tool (#164).
    pub(crate) offset_pick: Option<RecipeId>,
    /// The first curve picked for a fillet or chamfer.
    pub(crate) corner_first: Option<EntityId>,
    /// Fillet radius and chamfer sizes, edited in the tool options.
    pub(crate) corner: CornerSettings,
    /// Move/rotate/scale values, edited in the tool options.
    pub(crate) transform: TransformSettings,
    /// Settings for the next text the Text tool places, and the fonts it can use (#180).
    pub(crate) text_settings: TextSettings,
    pub(crate) fonts: FontCatalog,
    /// The selected text being edited: its id and the not-yet-committed settings.
    pub(crate) text_edit: Option<(EntityId, TextSettings)>,
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            draft: Draft::new(Tool::Select),
            pen: None,
            typed: String::new(),
            cursor: None,
            dim_edit: None,
            dragging: false,
            modify: None,
            offset_pick: None,
            corner_first: None,
            corner: CornerSettings::default(),
            transform: TransformSettings::default(),
            text_settings: TextSettings::default(),
            fonts: FontCatalog::headless(),
            text_edit: None,
        }
    }
}
