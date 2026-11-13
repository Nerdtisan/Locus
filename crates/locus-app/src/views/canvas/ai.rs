// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the MCP server needs from the canvas (SPEC §16.2, #229): the sketch
//! to read, and a way to apply an AI command exactly like a user one, so it
//! is one undo step and the canvas updates live. Desktop-only.

use crate::state::AppState;
use locus_doc::{Applied, Command, DocError, LayerId, Sketcher};

impl AppState {
    pub fn sketcher(&self) -> &Sketcher {
        &self.doc.sketcher
    }

    /// Apply a command from the AI client (one undo step, §16.4).
    pub fn apply_ai(&mut self, cmd: &Command) -> Result<Applied, DocError> {
        // A half-drawn shape may refer to what the AI changes.
        self.tools.draft.cancel();
        let out = self.doc.sketcher.apply(cmd)?;
        self.rebuild();
        Ok(out)
    }

    /// Where the AI's new geometry goes by default.
    pub fn ai_layer(&self) -> LayerId {
        self.active_layer()
    }
}
