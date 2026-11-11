// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fonts shipped inside the binary (SPEC §9): Hershey single-line fonts
//! (public domain, notice in `fonts/hershey/LICENSE-HERSHEY.txt`) and two SIL
//! OFL outline fonts (licences beside each file; see THIRD-PARTY-NOTICES.md).
//!
//! A document references a bundled font by its stable [`BundledFont::id`], never
//! by embedding it (SPEC §10; #179).

use crate::Font;
use crate::error::TextError;
use crate::outline::OutlineFont;
use crate::stroke::StrokeFont;
use std::sync::Arc;

const FUTURAL: &str = include_str!("../fonts/hershey/futural.jhf");
const ROWMANS: &str = include_str!("../fonts/hershey/rowmans.jhf");
const FIGTREE: &[u8] = include_bytes!("../fonts/figtree/Figtree.ttf");
const BASKERVILLE: &[u8] = include_bytes!("../fonts/librebaskerville/LibreBaskerville.ttf");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BundledFont {
    /// Hershey "Futura Light": single-line sans.
    HersheySans,
    /// Hershey "Roman Simplex": single-line serif.
    HersheySerif,
    /// Figtree (OFL): outline sans.
    Figtree,
    /// Libre Baskerville (OFL): outline serif.
    LibreBaskerville,
}

impl BundledFont {
    pub const ALL: [BundledFont; 4] = [
        BundledFont::HersheySans,
        BundledFont::HersheySerif,
        BundledFont::Figtree,
        BundledFont::LibreBaskerville,
    ];

    /// Stable identifier stored in `.locus` files. Never change an existing one.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            BundledFont::HersheySans => "hershey-sans",
            BundledFont::HersheySerif => "hershey-serif",
            BundledFont::Figtree => "figtree",
            BundledFont::LibreBaskerville => "libre-baskerville",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            BundledFont::HersheySans => "Hershey Sans (single-line)",
            BundledFont::HersheySerif => "Hershey Serif (single-line)",
            BundledFont::Figtree => "Figtree",
            BundledFont::LibreBaskerville => "Libre Baskerville",
        }
    }

    #[must_use]
    pub const fn is_single_line(self) -> bool {
        matches!(self, BundledFont::HersheySans | BundledFont::HersheySerif)
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.id() == id)
    }

    /// Parses the font. Outline fonts borrow the baked-in bytes; parsing is
    /// cheap, but callers laying out many strings should keep the result.
    ///
    /// # Errors
    /// Only if the bundled data were corrupt (guarded by tests).
    pub fn load(self) -> Result<Font, TextError> {
        Ok(match self {
            BundledFont::HersheySans => Font::Stroke(Arc::new(StrokeFont::from_jhf(
                self.display_name(),
                FUTURAL,
            )?)),
            BundledFont::HersheySerif => Font::Stroke(Arc::new(StrokeFont::from_jhf(
                self.display_name(),
                ROWMANS,
            )?)),
            BundledFont::Figtree => Font::Outline(OutlineFont::from_static("Figtree", FIGTREE)?),
            BundledFont::LibreBaskerville => {
                Font::Outline(OutlineFont::from_static("Libre Baskerville", BASKERVILLE)?)
            }
        })
    }
}
