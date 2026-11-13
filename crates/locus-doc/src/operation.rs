// SPDX-License-Identifier: AGPL-3.0-or-later

//! Layer operations and their parameters (SPEC §7).
//!
//! Lengths are millimetres, angles degrees (SPEC §4). Every value is validated
//! as finite and in range; both the command API and the file loader go
//! through [`Operation::validate`].

use crate::error::DocError;
use serde::{Deserialize, Serialize};

/// Kerf compensation side for [`Operation::Cut`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Compensation {
    None,
    Inside,
    Outside,
}

/// Which operation a layer has, without its parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum OperationKind {
    Cut,
    Score,
    Engrave,
    Draw,
    DragKnife,
    Reference,
}

impl OperationKind {
    /// Every operation, in SPEC §7 order.
    pub const ALL: [OperationKind; 6] = [
        OperationKind::Cut,
        OperationKind::Score,
        OperationKind::Engrave,
        OperationKind::Draw,
        OperationKind::DragKnife,
        OperationKind::Reference,
    ];

    /// Display name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            OperationKind::Cut => "Cut",
            OperationKind::Score => "Score",
            OperationKind::Engrave => "Engrave",
            OperationKind::Draw => "Draw",
            OperationKind::DragKnife => "Drag knife",
            OperationKind::Reference => "Reference",
        }
    }

    /// An operation of this kind with default parameters.
    #[must_use]
    pub fn default_operation(self) -> Operation {
        match self {
            OperationKind::Cut => Operation::Cut(CutParams::default()),
            OperationKind::Score => Operation::Score,
            OperationKind::Engrave => Operation::Engrave(EngraveParams::default()),
            OperationKind::Draw => Operation::Draw,
            OperationKind::DragKnife => Operation::DragKnife(DragKnifeParams::default()),
            OperationKind::Reference => Operation::Reference,
        }
    }
}

/// Cut parameters. `passes` is metadata for the machine operator.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CutParams {
    /// Kerf width, mm, `0..=MAX_LENGTH`.
    pub kerf_width: f64,
    pub compensation: Compensation,
    /// `1..=MAX_PASSES`.
    pub passes: u32,
}

impl Default for CutParams {
    fn default() -> Self {
        Self {
            kerf_width: 0.0,
            compensation: Compensation::None,
            passes: 1,
        }
    }
}

/// Engrave (fill) parameters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EngraveParams {
    /// Hatch line spacing, mm, `>0..=MAX_LENGTH`.
    pub hatch_spacing: f64,
    /// Hatch angle, degrees, `-360..=360`.
    pub hatch_angle: f64,
    /// Also hatch at `hatch_angle + 90°` (file format v7; absent = off).
    #[serde(default)]
    pub crosshatch: bool,
}

impl Default for EngraveParams {
    fn default() -> Self {
        Self {
            hatch_spacing: 0.1,
            hatch_angle: 0.0,
            crosshatch: false,
        }
    }
}

/// Drag-knife (vinyl) parameters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DragKnifeParams {
    /// Blade offset, mm, `0..=MAX_LENGTH`.
    pub blade_offset: f64,
}

impl Default for DragKnifeParams {
    fn default() -> Self {
        Self { blade_offset: 0.25 }
    }
}

/// Upper bound for any length parameter, mm.
pub const MAX_LENGTH: f64 = 1000.0;
pub const MAX_PASSES: u32 = 1000;
pub const MAX_PEN: u32 = 256;

/// What a layer does on the machine, with its parameters (SPEC §7).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Operation {
    Cut(CutParams),
    Score,
    Engrave(EngraveParams),
    /// Pen plotter. Pen number and width are each object's style since M5
    /// (SPEC §7, #287); file format v13 moved them off the layer.
    Draw,
    DragKnife(DragKnifeParams),
    /// Never exported.
    Reference,
}

fn check(what: &'static str, v: f64, lo: f64, hi: f64, lo_open: bool) -> Result<(), DocError> {
    let ok = v.is_finite() && v <= hi && if lo_open { v > lo } else { v >= lo };
    if ok {
        Ok(())
    } else {
        Err(DocError::BadValue { what, value: v })
    }
}

fn check_int(what: &'static str, v: u32, hi: u32) -> Result<(), DocError> {
    if (1..=hi).contains(&v) {
        Ok(())
    } else {
        Err(DocError::BadValue {
            what,
            value: f64::from(v),
        })
    }
}

impl Operation {
    #[must_use]
    pub const fn kind(&self) -> OperationKind {
        match self {
            Operation::Cut(_) => OperationKind::Cut,
            Operation::Score => OperationKind::Score,
            Operation::Engrave(_) => OperationKind::Engrave,
            Operation::Draw => OperationKind::Draw,
            Operation::DragKnife(_) => OperationKind::DragKnife,
            Operation::Reference => OperationKind::Reference,
        }
    }

    /// Reference layers are never exported (SPEC §7).
    #[must_use]
    pub const fn is_exportable(&self) -> bool {
        !matches!(self, Operation::Reference)
    }

    /// # Errors
    /// [`DocError::BadValue`] naming the offending parameter.
    pub fn validate(&self) -> Result<(), DocError> {
        match self {
            Operation::Cut(p) => {
                check("kerf width", p.kerf_width, 0.0, MAX_LENGTH, false)?;
                check_int("pass count", p.passes, MAX_PASSES)
            }
            Operation::Engrave(p) => {
                check("hatch spacing", p.hatch_spacing, 0.0, MAX_LENGTH, true)?;
                check("hatch angle", p.hatch_angle, -360.0, 360.0, false)
            }
            Operation::DragKnife(p) => {
                check("blade offset", p.blade_offset, 0.0, MAX_LENGTH, false)
            }
            Operation::Score | Operation::Draw | Operation::Reference => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_kinds_match() {
        for k in OperationKind::ALL {
            let op = k.default_operation();
            assert_eq!(op.kind(), k);
            op.validate().unwrap();
        }
        assert!(!Operation::Reference.is_exportable());
        assert!(Operation::Score.is_exportable());
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        let cut = |kerf_width, passes| {
            Operation::Cut(CutParams {
                kerf_width,
                passes,
                ..CutParams::default()
            })
        };
        let bad = [
            cut(-0.1, 1),
            cut(f64::NAN, 1),
            cut(0.0, 0),
            cut(0.0, MAX_PASSES + 1),
            Operation::Engrave(EngraveParams {
                hatch_spacing: 0.0,
                hatch_angle: 0.0,
                crosshatch: false,
            }),
            Operation::Engrave(EngraveParams {
                hatch_spacing: 1.0,
                hatch_angle: 400.0,
                crosshatch: false,
            }),
            Operation::DragKnife(DragKnifeParams { blade_offset: -1.0 }),
        ];
        for op in bad {
            assert!(
                matches!(op.validate(), Err(DocError::BadValue { .. })),
                "{op:?}"
            );
        }
    }
}
