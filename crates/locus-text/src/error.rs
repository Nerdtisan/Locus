// SPDX-License-Identifier: AGPL-3.0-or-later

use thiserror::Error;

/// Why a font could not be loaded or text could not be laid out. Font files
/// are untrusted input (CLAUDE.md, Secure Coding): every failure is a value.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TextError {
    #[error("not a usable TrueType/OpenType font")]
    BadOutlineFont,
    #[error("font file is larger than the {0}-byte limit")]
    FontTooLarge(usize),
    #[error("invalid single-line font: {0}")]
    BadStrokeFont(String),
    #[error("text is longer than the {0}-character limit")]
    TextTooLong(usize),
    #[error("unknown bundled font `{0}`")]
    UnknownBundledFont(String),
}
