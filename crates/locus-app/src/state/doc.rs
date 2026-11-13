// SPDX-License-Identifier: AGPL-3.0-or-later
//! Document state (design §7.3): the `Sketcher`, its derived scene, the
//! active layer and the last refusal. Every change goes through
//! `Sketcher::apply` (SPEC §11).

use crate::scene::Scene;
use locus_doc::{Command, ConstraintId, Document, EntityId, LayerId, Sketcher, Style};

/// A constraint the solver refused (SPEC §5 Solver UX): shown with the set it
/// clashes with highlighted, and — for a dimension — the offer to add it as a
/// driven (reference) dimension instead.
#[derive(Debug, Clone)]
pub struct Refusal {
    pub constraints: Vec<ConstraintId>,
    pub entities: Vec<EntityId>,
    pub redundant: bool,
    /// The same dimension, driven, if the refused command was one.
    pub as_driven: Option<Command>,
    /// Banner text for a refusal that isn't a new constraint (a transform).
    pub message: Option<String>,
}

pub struct DocState {
    /// Every document change goes through `Sketcher::apply` (SPEC §11): the
    /// command and the re-solve are one undo step.
    pub(crate) sketcher: Sketcher,
    /// Re-runs modifier plugins (#184); kept across document loads.
    pub(crate) plugin_runner: Option<std::sync::Arc<dyn locus_doc::generated::PluginRunner>>,
    /// Render scene derived from the document after every change.
    pub(crate) scene: Scene,
    /// The layer new geometry is drawn on (#118).
    pub(crate) active_layer: LayerId,
    /// The style new geometry gets (design §6.2, §9 #5; #287), shown in the
    /// Inspector while nothing is selected. App state, not document state.
    pub(crate) current_style: Style,
    /// The palette's Linetype: drawing tools make construction geometry
    /// (design §6.10, #289). App state, not document state.
    pub(crate) construction_linetype: bool,
    /// Last refusal or error, shown in the status bar until the next action.
    pub(crate) notice: Option<String>,
    /// The last constraint the solver refused, until dismissed (#72).
    pub(crate) refusal: Option<Refusal>,
    /// Bumped whenever a different document is loaded, so views drop
    /// transient state (an expanded row, a half-typed name) that referred
    /// to the old one.
    pub(crate) generation: u64,
}

impl Default for DocState {
    fn default() -> Self {
        Self {
            sketcher: Sketcher::new(Document::new()),
            plugin_runner: None,
            scene: Scene::default(),
            active_layer: Document::default_layer(),
            current_style: Style::default(),
            construction_linetype: false,
            notice: None,
            refusal: None,
            generation: 0,
        }
    }
}
