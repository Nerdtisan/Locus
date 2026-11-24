// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Stroke width view with caps and joins (#428, ADR 0019 S3): each
//! object's stroke as triangles from `locus-stroke`, curves joined end to
//! end as export joins them. Built in world millimetres once per scene and
//! kept until the document changes, so panning and zooming only map points.

use crate::scene::{Scene, Shape};
use locus_doc::{Document, LayerId, LineStyle, Operation};
use locus_geom::{Piece, Vec2};
use locus_region::Seg;
use locus_stroke::{Limits, chains, mesh};

/// World tolerance of the canvas mesh, mm: under half a pixel up to
/// 100 px/mm, and cheap (an arc of 50 mm radius is ~220 steps a turn).
const TOL: f64 = 0.005;

/// Triangles of the stroke preview, cached per scene.
#[derive(Debug, Default)]
pub(crate) struct StrokeMesh {
    /// The scene stamp the triangles were built for.
    pub stamp: u64,
    pub tris: Vec<[Vec2; 3]>,
}

fn segs(shape: &Shape) -> Vec<Seg> {
    let arc = |a: &locus_geom::Arc| Seg::Arc {
        center: a.center,
        radius: a.radius(),
        start: a.start_angle(),
        sweep: a.sweep(),
    };
    match shape {
        Shape::Point(_) => Vec::new(),
        Shape::Line(l) => vec![Seg::Line { a: l.a, b: l.b }],
        Shape::Arc(a) => vec![arc(a)],
        Shape::Circle(c) => vec![Seg::Arc {
            center: c.center,
            radius: c.radius,
            start: 0.0,
            sweep: core::f64::consts::TAU,
        }],
        Shape::Cubic(c) => c
            .to_pieces_oriented(TOL)
            .into_iter()
            .map(|(p, back)| match p {
                Piece::Line(l) => Seg::Line { a: l.a, b: l.b },
                Piece::Arc(a) => {
                    if back {
                        arc(&a).reversed()
                    } else {
                        arc(&a)
                    }
                }
            })
            .collect(),
    }
}

/// The preview's triangles: Draw layers with each object's width and line
/// style; Cut layers with their kerf as a round beam (as before M6).
pub(crate) fn triangles(doc: &Document, scene: &Scene) -> Vec<[Vec2; 3]> {
    // Group by layer, width and style in first-seen order (deterministic).
    let mut groups: Vec<(LayerId, f64, LineStyle, Vec<Vec<Seg>>)> = Vec::new();
    for item in &scene.items {
        if item.construction {
            continue;
        }
        let Some(e) = doc.entity(item.id) else {
            continue;
        };
        let w = doc.stroke_width(item.id);
        if w <= 0.0 {
            continue;
        }
        let line = match doc.layer(e.layer).map(|l| &l.operation) {
            Some(Operation::Draw) => e.style.line.clone(),
            _ => LineStyle::default(),
        };
        let s = segs(&item.shape);
        if s.is_empty() {
            continue;
        }
        match groups
            .iter_mut()
            .find(|(l, gw, gs, _)| *l == e.layer && *gw == w && *gs == line)
        {
            Some((_, _, _, v)) => v.push(s),
            None => groups.push((e.layer, w, line, vec![s])),
        }
    }
    let limits = Limits::default();
    let mut tris = Vec::new();
    for (_, w, line, items) in groups {
        for chain in chains(items) {
            // A stroke over the bounds is left out of the preview; export
            // reports it (§6.2.1).
            let mark = tris.len();
            if mesh(&chain, w, &line, TOL, &limits, &mut tris).is_err() {
                tris.truncate(mark);
            }
        }
    }
    tris
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use locus_doc::{Cap, Command, Editor, Geom, Style};
    use std::collections::BTreeSet;

    /// A 10 mm line, 2 mm wide, with square caps: the preview reaches 1 mm
    /// past each end (#428).
    #[test]
    fn square_caps_reach_past_the_ends() {
        let mut ed = Editor::new(Document::new());
        let layer = Document::default_layer();
        let mut pt = |x: f64| {
            ed.apply(&Command::AddEntity {
                layer,
                construction: false,
                style: Style::default(),
                geom: Geom::Point {
                    pos: Vec2::new(x, 0.0),
                },
            })
            .unwrap()
            .entities[0]
        };
        let (a, b) = (pt(0.0), pt(10.0));
        let mut style = Style {
            width: 2.0,
            ..Style::default()
        };
        style.line.cap = Cap::Square;
        ed.apply(&Command::AddEntity {
            layer,
            construction: false,
            style,
            geom: Geom::Line { start: a, end: b },
        })
        .unwrap();
        let doc = ed.document();
        let scene = Scene::build(doc, &BTreeSet::new(), &BTreeSet::new());
        let tris = triangles(doc, &scene);
        let xs = tris.iter().flatten().map(|p| p.x);
        let (lo, hi) = xs.fold((f64::MAX, f64::MIN), |(l, h), x| (l.min(x), h.max(x)));
        assert!(
            (lo + 1.0).abs() < 1e-9 && (hi - 11.0).abs() < 1e-9,
            "{lo} {hi}"
        );
    }
}
