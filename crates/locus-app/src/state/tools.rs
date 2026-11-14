// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tool state (design §7.3): the active tool, held by the [`ToolHost`]
//! (§7.2), and the canvas-level input around it: the typed heads-up value,
//! the cursor, a point drag, and the tool options.

use crate::constrain::DimSpec;
use crate::modify_tool::{CornerSettings, TransformSettings};
use crate::tool::ToolHost;
use crate::tools::Pick;
use crate::views::canvas::{MirrorDraft, OffsetDraft, PatternDraft};
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
    /// The active tool (design §7.2): Select, a drawing tool or a modify tool.
    pub(crate) host: ToolHost,
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
    /// The offset picked with the Offset tool (#164).
    pub(crate) offset_pick: Option<RecipeId>,
    /// An offset being set up with the Offset tool, before Enter (#218).
    pub(crate) offset_draft: Option<OffsetDraft>,
    /// The Pattern tools' values (#221), kept between uses.
    pub(crate) pattern: PatternDraft,
    /// The Mirror tool's line and options (#215).
    pub(crate) mirror: MirrorDraft,
    /// Fillet radius and chamfer sizes, edited in the command dialog.
    pub(crate) corner: CornerSettings,
    /// The corner picked for a fillet or chamfer, waiting for OK / Enter
    /// (#217), and whether its size field should take focus.
    pub(crate) corner_pick: Option<(EntityId, EntityId)>,
    pub(crate) corner_focus: bool,
    /// Move/rotate/scale values, edited in the tool options.
    pub(crate) transform: TransformSettings,
    /// Settings for the next text the Text tool places, and the fonts it can use (#180).
    pub(crate) text_settings: TextSettings,
    pub(crate) fonts: FontCatalog,
    /// The selected text being edited: its id and the not-yet-committed settings.
    pub(crate) text_edit: Option<(EntityId, TextSettings)>,
    /// The Style eyedropper is waiting for a click on an object (design §6.2).
    pub(crate) eyedropper: bool,
    /// The last tool or command run from the registry, for Repeat (design §4).
    pub(crate) last_action: Option<String>,
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            host: ToolHost::default(),
            pen: None,
            typed: String::new(),
            cursor: None,
            dim_edit: None,
            dragging: false,
            offset_pick: None,
            offset_draft: None,
            pattern: PatternDraft::default(),
            mirror: MirrorDraft::default(),
            corner: CornerSettings::default(),
            corner_pick: None,
            corner_focus: false,
            transform: TransformSettings::default(),
            text_settings: TextSettings::default(),
            fonts: FontCatalog::headless(),
            text_edit: None,
            eyedropper: false,
            last_action: None,
        }
    }
}
