// SPDX-License-Identifier: AGPL-3.0-or-later

//! Expand to geometry (#428, ADR 0019 decision 4): a styled stroke becomes
//! the plain curves of its outline, as one undo step.
//!
//! The selection's curves are grouped by layer and style and joined into
//! chains, as the canvas joins them. Each chain is expanded by
//! `locus-stroke` into an exact line/arc outline, written as new entities on
//! the same layer with the same colour, pen and opacity, and the source
//! curves are deleted as a user delete would. Béziers are lowered to biarcs
//! within the Expand tolerance (owner-approved default 0.01 mm).

use crate::document::Document;
use crate::entity::Geom;
use crate::error::DocError;
use crate::ids::{EntityId, LayerId};
use crate::offset::{Out, emit, pieces_within};
use crate::recipe::MAX_INSTANCE_ENTITIES;
use crate::style::{DEFAULT_WIDTH, Style};
use locus_region::Seg;
use locus_stroke::{Limits, LineStyle, OutPiece, chains, dashes, layer_strokes, outline};
use serde::{Deserialize, Serialize};

/// The Expand tolerance's default, mm (owner decision 2026-11-24).
pub const DEFAULT_EXPAND_TOL: f64 = 0.01;
/// Accepted Expand tolerances, mm.
pub const MIN_EXPAND_TOL: f64 = 1e-4;
pub const MAX_EXPAND_TOL: f64 = 1.0;

/// What Expand to geometry writes (design §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExpandMode {
    /// The stroke's filled outline as closed curves.
    #[default]
    Outline,
    /// The pen paths: each dash as an open curve (a solid stroke gives its
    /// path back as plain curves).
    Centrelines,
}

/// One stroke's expanded output: its colour and pen overrides, and entities.
type StrokeOut = (Option<[u8; 3]>, Option<u32>, Vec<Out>);

/// One group's output: the entities to add on `layer` in `style`.
pub(crate) struct Expanded {
    pub layer: LayerId,
    pub style: Style,
    pub out: Vec<Out>,
}

/// The outlines of `ids`' strokes, grouped by layer and style, and the
/// sources they replace.
///
/// # Errors
/// [`DocError`] for nothing to expand, an unknown or locked entity, a recipe
/// instance (explode it first, ADR 0011), a bad tolerance, or a stroke over
/// the bounds of SPEC §6.2.1.
pub(crate) fn plan(
    doc: &Document,
    ids: &[EntityId],
    tol: f64,
    mode: ExpandMode,
) -> Result<(Vec<Expanded>, Vec<EntityId>), DocError> {
    if !(tol.is_finite() && (MIN_EXPAND_TOL..=MAX_EXPAND_TOL).contains(&tol)) {
        return Err(DocError::BadValue {
            what: "expand tolerance",
            value: tol,
        });
    }
    // Group in first-seen order, so the output is deterministic.
    let mut groups: Vec<(LayerId, Style, Vec<Vec<Seg>>)> = Vec::new();
    let mut sources = Vec::new();
    for &id in ids {
        let e = doc.entity(id).ok_or(DocError::UnknownEntity(id))?;
        let curve = matches!(
            e.geom,
            Geom::Line { .. } | Geom::Arc { .. } | Geom::Circle { .. } | Geom::Bezier { .. }
        );
        if !curve || e.construction {
            continue;
        }
        doc.check_unlocked(e.layer)?;
        if doc.driven_by(id).is_some() {
            return Err(DocError::Invalid(
                "expand: this curve belongs to a pattern, offset or plugin object; explode it first"
                    .to_owned(),
            ));
        }
        let segs = pieces_within(doc, id, false, tol * 0.5);
        if segs.is_empty() {
            continue;
        }
        sources.push(id);
        // A curve whose profile strokes alone (#527) is a group of its own.
        let alone = e.style.line.strokes_alone();
        match groups
            .iter_mut()
            .find(|(l, s, _)| !alone && *l == e.layer && *s == e.style)
        {
            Some((_, _, v)) => v.push(segs),
            None => groups.push((e.layer, e.style.clone(), vec![segs])),
        }
    }
    if sources.is_empty() {
        return Err(DocError::NothingToDo("expand to geometry"));
    }
    let limits = Limits::default();
    let mut result = Vec::new();
    let mut total = 0usize;
    let err = |e: locus_stroke::StrokeError| DocError::Invalid(format!("expand: {e}"));
    for (layer, style, items) in groups {
        // One output group per stroke (#429): the object's own, then each
        // extra stroke in its own colour and pen.
        let mut per_stroke: Vec<StrokeOut> = Vec::new();
        for chain in chains(items) {
            let strokes = layer_strokes(&chain, style.width, &style.line, &limits).map_err(err)?;
            for ls in strokes {
                let mut out = Vec::new();
                for c in &ls.chains {
                    match mode {
                        ExpandMode::Outline => {
                            // Width profiles keep their fitted cubics within
                            // the Expand tolerance (ADR 0019 option (b)).
                            let o = outline(c, ls.width, &ls.line, tol, &limits).map_err(err)?;
                            for l in &o.loops {
                                emit_outline(l, &mut out);
                            }
                        }
                        ExpandMode::Centrelines => {
                            let runs = match &ls.line.dash {
                                Some(d) => dashes(c, d, &limits).map_err(err)?,
                                None => vec![c.clone()],
                            };
                            for run in runs {
                                let edges: Vec<locus_region::Edge> = run
                                    .segs
                                    .iter()
                                    // A dot has no length to draw as a curve.
                                    .filter(|s| s.length() > locus_region::SNAP)
                                    .map(|s| locus_region::Edge { seg: *s, source: 0 })
                                    .collect();
                                emit(&edges, run.closed, &mut out);
                            }
                        }
                    }
                }
                match per_stroke.get_mut(ls.index) {
                    Some((_, _, o)) => o.extend(out),
                    None => per_stroke.push((ls.colour, ls.pen, out)),
                }
            }
        }
        for (colour, pen, out) in per_stroke {
            if out.is_empty() {
                continue;
            }
            total += out.len();
            if total > MAX_INSTANCE_ENTITIES {
                return Err(DocError::InstanceLimit {
                    requested: total,
                    limit: MAX_INSTANCE_ENTITIES,
                });
            }
            // The outline keeps the stroke's colour, pen and opacity, drawn
            // as a plain line (its width is now geometry).
            let plain = Style {
                width: DEFAULT_WIDTH,
                line: LineStyle::default(),
                stroke: colour.map_or(style.stroke, crate::style::Stroke::Colour),
                pen: pen.unwrap_or(style.pen),
                ..style.clone()
            };
            result.push(Expanded {
                layer,
                style: plain,
                out,
            });
        }
    }
    Ok((result, sources))
}

/// One closed outline loop as entities: lines and arcs as [`emit`] writes
/// them, cubics as Béziers with their control points.
fn emit_outline(l: &[OutPiece], out: &mut Vec<Out>) {
    let segs: Option<Vec<locus_region::Edge>> = l
        .iter()
        .map(|p| match p {
            OutPiece::Seg(seg) => Some(locus_region::Edge {
                seg: *seg,
                source: 0,
            }),
            OutPiece::Cubic(_) => None,
        })
        .collect();
    if let Some(edges) = segs {
        emit(&edges, true, out);
        return;
    }
    let base = out.len();
    for p in l {
        out.push(Out::Point(p.start()));
    }
    let n = l.len();
    for (i, p) in l.iter().enumerate() {
        let (a, b) = (base + i, base + (i + 1) % n);
        match p {
            OutPiece::Seg(Seg::Line { .. }) => out.push(Out::Line(a, b)),
            OutPiece::Seg(Seg::Arc { center, sweep, .. }) => {
                out.push(Out::Point(*center));
                let c = out.len() - 1;
                out.push(if *sweep >= 0.0 {
                    Out::Arc(c, a, b)
                } else {
                    Out::Arc(c, b, a)
                });
            }
            OutPiece::Cubic(cu) => {
                out.push(Out::Point(cu.c0));
                out.push(Out::Point(cu.c1));
                let c1 = out.len() - 1;
                out.push(Out::Bezier(a, c1 - 1, c1, b));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::command::{Command, Editor};
    use crate::document::Document;
    use crate::entity::Geom;
    use crate::style::{Style, StylePatch};
    use locus_geom::Vec2;
    use locus_stroke::{Cap, Join};

    fn l_doc(w: f64) -> (Editor, Vec<crate::ids::EntityId>) {
        let mut ed = Editor::new(Document::new());
        let layer = Document::default_layer();
        let mut pts = Vec::new();
        for (x, y) in [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)] {
            pts.push(
                ed.apply(&Command::AddEntity {
                    layer,
                    construction: false,
                    style: Style::default(),
                    geom: Geom::Point {
                        pos: Vec2::new(x, y),
                    },
                })
                .unwrap()
                .entities[0],
            );
        }
        let style = Style {
            width: w,
            ..Style::default()
        };
        let mut ids = Vec::new();
        for (s, e) in [(pts[0], pts[1]), (pts[1], pts[2])] {
            ids.push(
                ed.apply(&Command::AddEntity {
                    layer,
                    construction: false,
                    style: style.clone(),
                    geom: Geom::Line { start: s, end: e },
                })
                .unwrap()
                .entities[0],
            );
        }
        (ed, ids)
    }

    fn outline_area(ed: &Editor) -> f64 {
        let doc = ed.document();
        let segs: Vec<Vec<locus_region::Seg>> = doc
            .entities()
            .iter()
            .filter(|(_, e)| !matches!(e.geom, Geom::Point { .. }))
            .map(|(id, _)| crate::offset::pieces(doc, id, false))
            .collect();
        locus_stroke::chains(segs)
            .iter()
            .map(|c| {
                c.segs
                    .iter()
                    .map(locus_region::Seg::area_term)
                    .sum::<f64>()
                    .abs()
            })
            .sum()
    }

    #[test]
    fn expanding_an_l_gives_its_outline_in_one_undo_step() {
        let (mut ed, ids) = l_doc(2.0);
        ed.apply(&Command::SetStyle {
            entities: ids.clone(),
            patch: StylePatch {
                cap: Some(Cap::Butt),
                join: Some(Join::Miter { limit: 4.0 }),
                ..StylePatch::default()
            },
        })
        .unwrap();
        let before = ed.document().clone();
        ed.apply(&Command::ExpandStyle {
            entities: ids.clone(),
            tol: super::DEFAULT_EXPAND_TOL,
            mode: super::ExpandMode::Outline,
        })
        .unwrap();
        let doc = ed.document();
        assert!(ids.iter().all(|i| doc.entity(*i).is_none()));
        // An L of two 10 mm arms, 2 mm wide, butt caps, mitred: 6 lines.
        let lines = doc
            .entities()
            .iter()
            .filter(|(_, e)| matches!(e.geom, Geom::Line { .. }))
            .count();
        assert_eq!(lines, 6);
        assert!((outline_area(&ed) - 40.0).abs() < 1e-6);
        assert!(
            doc.entities()
                .iter()
                .all(|(_, e)| e.style.line.is_default())
        );
        ed.undo().unwrap();
        // IDs are never reused (ADR 0003), so compare the content.
        assert_eq!(ed.document().entities(), before.entities());
    }

    #[test]
    fn a_width_profile_expands_to_beziers() {
        let (mut ed, ids) = l_doc(2.0);
        ed.apply(&Command::SetStyle {
            entities: ids.clone(),
            patch: StylePatch {
                profile: Some(Some(locus_stroke::ProfilePreset::TaperEnd.profile(0))),
                ..StylePatch::default()
            },
        })
        .unwrap();
        let style = ed.document().entity(ids[0]).unwrap().style.clone();
        assert!(style.line.varying().is_some());
        // The exact cleaned outline's area, to compare with.
        let doc = ed.document();
        let segs: Vec<Vec<locus_region::Seg>> = ids
            .iter()
            .map(|id| crate::offset::pieces(doc, *id, false))
            .collect();
        let chain = &locus_stroke::chains(segs)[0];
        let exact = locus_stroke::expand(chain, 2.0, &style.line, &locus_stroke::Limits::default())
            .unwrap()
            .area();
        let before = ed.document().clone();
        ed.apply(&Command::ExpandStyle {
            entities: ids,
            tol: super::DEFAULT_EXPAND_TOL,
            mode: super::ExpandMode::Outline,
        })
        .unwrap();
        let doc = ed.document();
        assert!(
            doc.entities()
                .iter()
                .any(|(_, e)| matches!(e.geom, Geom::Bezier { .. }))
        );
        assert!((outline_area(&ed) - exact).abs() < 0.05, "{exact}");
        ed.undo().unwrap();
        assert_eq!(ed.document().entities(), before.entities());
    }

    #[test]
    fn centrelines_give_one_open_curve_per_dash() {
        let (mut ed, ids) = l_doc(1.0);
        ed.apply(&Command::SetStyle {
            entities: ids.clone(),
            patch: StylePatch {
                dash: Some(Some(
                    locus_stroke::Dash::new(&[4.0, 1.0], 2.0, locus_stroke::DashAlign::Exact)
                        .unwrap(),
                )),
                ..StylePatch::default()
            },
        })
        .unwrap();
        ed.apply(&Command::ExpandStyle {
            entities: ids,
            tol: super::DEFAULT_EXPAND_TOL,
            mode: super::ExpandMode::Centrelines,
        })
        .unwrap();
        // 20 mm of path, dash 4 gap 1, phase 2: dashes 0–2, 3–7, 8–12
        // (across the corner: two lines), 13–17, 18–20 → six lines.
        let lines = ed
            .document()
            .entities()
            .iter()
            .filter(|(_, e)| matches!(e.geom, Geom::Line { .. }))
            .count();
        assert_eq!(lines, 6);
    }

    #[test]
    fn an_extra_stroke_expands_in_its_own_colour() {
        let (mut ed, ids) = l_doc(1.0);
        ed.apply(&Command::SetStyle {
            entities: ids.clone(),
            patch: StylePatch {
                extra: Some(vec![locus_stroke::StrokeLayer {
                    offset: 2.0,
                    width: 0.5,
                    colour: Some([255, 0, 0]),
                    ..locus_stroke::StrokeLayer::default()
                }]),
                ..StylePatch::default()
            },
        })
        .unwrap();
        ed.apply(&Command::ExpandStyle {
            entities: ids,
            tol: super::DEFAULT_EXPAND_TOL,
            mode: super::ExpandMode::Centrelines,
        })
        .unwrap();
        let red = ed
            .document()
            .entities()
            .iter()
            .filter(|(_, e)| {
                !matches!(e.geom, Geom::Point { .. })
                    && e.style.stroke == crate::style::Stroke::Colour([255, 0, 0])
            })
            .count();
        // The L offset 2 mm to its left: two lines and a round joint.
        assert!(red >= 2, "{red}");
    }

    #[test]
    fn nothing_to_expand_and_bad_tolerances_are_refused() {
        let (mut ed, ids) = l_doc(1.0);
        for tol in [0.0, f64::NAN, 5.0] {
            assert!(
                ed.apply(&Command::ExpandStyle {
                    entities: ids.clone(),
                    tol,
                    mode: super::ExpandMode::Outline,
                })
                .is_err()
            );
        }
        assert!(
            ed.apply(&Command::ExpandStyle {
                entities: Vec::new(),
                tol: 0.01,
                mode: super::ExpandMode::Outline,
            })
            .is_err()
        );
    }
}
