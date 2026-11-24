// SPDX-License-Identifier: AGPL-3.0-or-later

//! Multi-strokes (#429, ADR 0019 decision 3, design §3): extra strokes
//! drawn alongside an object's own, each the exact ADR 0010 offset of the
//! path, then dashed and widened like the base stroke.

use crate::dash::Dash;
use crate::style::{Align, Cap, LineStyle};
use crate::{Chain, Limits, MAX_WIDTH, StrokeError};
use locus_region::{Area, BoolOp, Edge, Join, Loop, SNAP, Seg, boolean, offset, offset_open};
use serde::{Deserialize, Serialize};

/// Most extra strokes on one object (design §3: 16 rows with the base).
pub const MAX_STROKE_LAYERS: usize = 15;
/// Largest plotter pen number (as `locus_doc`'s `MAX_PEN`).
pub const MAX_PEN: u32 = 256;

/// One extra stroke. `None` fields follow the object's own stroke.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StrokeLayer {
    /// Distance from the path, mm: + is left of travel (outside on a
    /// clockwise closed shape).
    pub offset: f64,
    /// Stroke width, mm.
    pub width: f64,
    /// `[r, g, b]`; `None` is the object's stroke colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colour: Option<[u8; 3]>,
    /// Plotter pen; `None` is the object's pen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pen: Option<u32>,
    /// This stroke's own dash pattern; `None` is solid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dash: Option<Dash>,
    /// This stroke's cap; `None` is the object's cap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cap: Option<Cap>,
}

impl Default for StrokeLayer {
    fn default() -> Self {
        Self {
            offset: 0.0,
            width: 0.4,
            colour: None,
            pen: None,
            dash: None,
            cap: None,
        }
    }
}

impl StrokeLayer {
    /// # Errors
    /// [`StrokeError::BadValue`] naming the bad field.
    pub fn validate(&self) -> Result<(), StrokeError> {
        let bad = |what, value| Err(StrokeError::BadValue { what, value });
        if !(self.offset.is_finite() && self.offset.abs() <= MAX_WIDTH) {
            return bad("stroke offset", self.offset);
        }
        if !(self.width.is_finite() && self.width > 0.0 && self.width <= MAX_WIDTH) {
            return bad("stroke width", self.width);
        }
        if let Some(p) = self.pen
            && !(1..=MAX_PEN).contains(&p)
        {
            return bad("pen number", f64::from(p));
        }
        if let Some(d) = &self.dash {
            d.validate()?;
        }
        Ok(())
    }
}

/// One stroke of an object, ready to draw: its pen paths, width and style.
/// Index 0 is the object's own stroke.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerStroke {
    pub index: usize,
    pub chains: Vec<Chain>,
    pub width: f64,
    /// Cap, join and dash for this stroke (no extras of its own).
    pub line: LineStyle,
    pub colour: Option<[u8; 3]>,
    pub pen: Option<u32>,
}

/// `chain` offset by `d` mm (+ left of travel), as the pen paths of the
/// offset: exact lines and arcs (ADR 0010), runs broken where the offset
/// folds away (tight curves), round joins.
#[must_use]
pub fn offset_chain(chain: &Chain, d: f64) -> Vec<Chain> {
    if chain.segs.is_empty() || !d.is_finite() {
        return Vec::new();
    }
    if d.abs() <= SNAP {
        return vec![chain.clone()];
    }
    let edges: Vec<Edge> = chain
        .segs
        .iter()
        .map(|s| Edge { seg: *s, source: 0 })
        .collect();
    if !chain.closed {
        return offset_open(&edges, d, Join::Round)
            .into_iter()
            .map(|run| Chain {
                segs: run.into_iter().map(|e| e.seg).collect(),
                closed: false,
            })
            .collect();
    }
    // Closed: left of a counter-clockwise loop is its inside.
    let signed: f64 = chain.segs.iter().map(Seg::area_term).sum();
    let area = boolean(
        &Area {
            loops: vec![Loop { edges }],
        },
        &Area::default(),
        BoolOp::Union,
    );
    let grow = if signed >= 0.0 { -d } else { d };
    offset(&area, grow, Join::Round)
        .loops
        .into_iter()
        .map(|l| Chain {
            segs: l.edges.into_iter().map(|e| e.seg).collect(),
            closed: true,
        })
        .collect()
}

/// Every stroke of `chain` under `style`: the object's own (index 0, at
/// `width`), then each extra stroke at its offset.
///
/// # Errors
/// [`StrokeError`] for a bad extra stroke or too many of them.
pub fn layer_strokes(
    chain: &Chain,
    width: f64,
    style: &LineStyle,
    limits: &Limits,
) -> Result<Vec<LayerStroke>, StrokeError> {
    if style.extra.len() > MAX_STROKE_LAYERS {
        return Err(StrokeError::Limit {
            what: "extra strokes",
            count: style.extra.len(),
        });
    }
    let base = LineStyle {
        extra: Vec::new(),
        ..style.clone()
    };
    let mut out = vec![LayerStroke {
        index: 0,
        chains: vec![chain.clone()],
        width,
        line: base,
        colour: None,
        pen: None,
    }];
    let mut pieces = chain.segs.len();
    for (i, l) in style.extra.iter().enumerate() {
        l.validate()?;
        let chains = offset_chain(chain, l.offset);
        pieces += chains.iter().map(|c| c.segs.len()).sum::<usize>();
        if pieces > limits.pieces {
            return Err(StrokeError::Limit {
                what: "path pieces",
                count: pieces,
            });
        }
        out.push(LayerStroke {
            index: i + 1,
            chains,
            width: l.width,
            line: LineStyle {
                cap: l.cap.unwrap_or(style.cap),
                join: style.join,
                align: Align::Centre,
                dash: l.dash,
                extra: Vec::new(),
            },
            colour: l.colour,
            pen: l.pen,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use locus_geom::Vec2;

    fn rect(ccw: bool) -> Chain {
        let mut p = [
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 0.0),
            Vec2::new(20.0, 10.0),
            Vec2::new(0.0, 10.0),
        ];
        if !ccw {
            p.reverse();
        }
        Chain {
            segs: (0..4)
                .map(|i| Seg::Line {
                    a: p[i],
                    b: p[(i + 1) % 4],
                })
                .collect(),
            closed: true,
        }
    }

    fn area(c: &Chain) -> f64 {
        c.segs.iter().map(Seg::area_term).sum::<f64>().abs()
    }

    #[test]
    fn plus_is_left_inside_ccw_and_outside_cw() {
        let inside = offset_chain(&rect(true), 1.0);
        assert!((area(&inside[0]) - 18.0 * 8.0).abs() < 1e-9);
        let outside = offset_chain(&rect(false), 1.0);
        // Grown with round corners: 22 × 12 less the corner squares plus
        // quarter discs.
        let want = 22.0 * 12.0 - 4.0 + core::f64::consts::PI;
        assert!((area(&outside[0]) - want).abs() < 1e-9);
    }

    #[test]
    fn an_open_line_offsets_to_a_parallel_line() {
        let c = Chain {
            segs: vec![Seg::Line {
                a: Vec2::new(0.0, 0.0),
                b: Vec2::new(10.0, 0.0),
            }],
            closed: false,
        };
        let o = offset_chain(&c, 2.0);
        assert_eq!(o.len(), 1);
        assert!(o[0].segs.iter().all(|s| (s.start().y - 2.0).abs() < 1e-9));
    }

    #[test]
    fn bad_and_too_many_layers_are_refused() {
        let c = rect(true);
        let bad = LineStyle {
            extra: vec![StrokeLayer {
                width: f64::NAN,
                ..StrokeLayer::default()
            }],
            ..LineStyle::default()
        };
        assert!(layer_strokes(&c, 1.0, &bad, &Limits::default()).is_err());
        let many = LineStyle {
            extra: vec![StrokeLayer::default(); MAX_STROKE_LAYERS + 1],
            ..LineStyle::default()
        };
        assert!(layer_strokes(&c, 1.0, &many, &Limits::default()).is_err());
    }
}
