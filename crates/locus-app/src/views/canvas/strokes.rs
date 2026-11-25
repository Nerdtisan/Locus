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

pub(crate) fn segs(shape: &Shape) -> Vec<Seg> {
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
        // A curve whose profile strokes alone (#527) is a group of its own.
        let alone = line.strokes_alone();
        match groups
            .iter_mut()
            .find(|(l, gw, gs, _)| !alone && *l == e.layer && *gw == w && *gs == line)
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

/// The paths the selection's extra strokes follow (#429): each extra
/// stroke's offset path, as polylines in world mm. `patch` is an Inspector
/// edit not yet committed, so the guides move live while it is typed.
pub(crate) fn guides(
    doc: &Document,
    ids: &[locus_doc::EntityId],
    patch: Option<&locus_doc::StylePatch>,
) -> Vec<Vec<Vec2>> {
    let mut out = Vec::new();
    for id in ids {
        let Some(e) = doc.entity(*id) else {
            continue;
        };
        let style = match patch {
            Some(p) => e.style.clone().patched(p),
            None => e.style.clone(),
        };
        if style.line.extra.is_empty() {
            continue;
        }
        let shape = match &e.geom {
            locus_doc::Geom::Line { .. } => doc.line(*id).map(Shape::Line),
            locus_doc::Geom::Arc { .. } => doc.arc(*id).map(Shape::Arc),
            locus_doc::Geom::Circle { .. } => doc.circle(*id).map(Shape::Circle),
            locus_doc::Geom::Bezier { .. } => doc.cubic(*id).map(Shape::Cubic),
            _ => None,
        };
        let Some(shape) = shape else {
            continue;
        };
        let segs = segs(&shape);
        let closed = matches!(shape, Shape::Circle(_));
        let chain = locus_stroke::Chain { segs, closed };
        let Ok(strokes) =
            locus_stroke::layer_strokes(&chain, style.width, &style.line, &Limits::default())
        else {
            continue;
        };
        for ls in strokes.into_iter().skip(1) {
            for c in ls.chains {
                let mut pts = Vec::new();
                for sg in &c.segs {
                    sg.flatten_into(TOL * 4.0, &mut pts);
                }
                if let Some(last) = c.segs.last() {
                    pts.push(last.end());
                }
                out.push(pts);
            }
        }
    }
    out
}

impl crate::state::AppState {
    /// Draw the selection's extra-stroke guides (#429): thin dashed lines in
    /// the selection colour along each extra stroke's path, following any
    /// Inspector edit live.
    pub(crate) fn draw_stroke_guides(&self, frame: &crate::render::Frame<'_>, ctx: &egui::Context) {
        let ids: Vec<locus_doc::EntityId> = self.selection.items.iter().collect();
        if ids.is_empty() {
            return;
        }
        let pending: Option<(Vec<locus_doc::EntityId>, locus_doc::StylePatch)> =
            ctx.data(|d| d.get_temp(egui::Id::new(crate::views::style::PENDING)));
        let patch = pending
            .as_ref()
            .filter(|(sel, _)| *sel == ids)
            .map(|(_, p)| p);
        let doc = self.doc.sketcher.document();
        let stroke = egui::Stroke::new(1.0, frame.pal.selected);
        for pts in guides(doc, &ids, patch) {
            frame.world_polyline(&pts, stroke, true);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use locus_doc::{Cap, Command, Editor, Geom, Style};
    use locus_stroke::{ProfilePreset, WidthProfile};
    use std::collections::BTreeSet;

    /// A 10 mm line, 2 mm wide, with square caps: the preview reaches 1 mm
    /// past each end (#428).
    /// #429: an extra stroke's guide sits at its offset, and a pending
    /// Inspector edit moves it before it is committed.
    #[test]
    fn guides_follow_extra_strokes_and_pending_edits() {
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
        let mut style = Style::default();
        style.line.extra = vec![locus_doc::StrokeLayer {
            offset: 2.0,
            ..locus_doc::StrokeLayer::default()
        }];
        let id = ed
            .apply(&Command::AddEntity {
                layer,
                construction: false,
                style,
                geom: Geom::Line { start: a, end: b },
            })
            .unwrap()
            .entities[0];
        let g = guides(ed.document(), &[id], None);
        assert_eq!(g.len(), 1);
        assert!(g[0].iter().all(|p| (p.y - 2.0).abs() < 1e-9));
        let patch = locus_doc::StylePatch {
            extra: Some(vec![locus_doc::StrokeLayer {
                offset: -3.0,
                ..locus_doc::StrokeLayer::default()
            }]),
            ..locus_doc::StylePatch::default()
        };
        let g = guides(ed.document(), &[id], Some(&patch));
        assert!(g[0].iter().all(|p| (p.y + 3.0).abs() < 1e-9));
    }

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

    /// A closed rectangle of four connected lines, 1 mm wide, each with a
    /// tapering profile that strokes alone or not.
    fn rect_tris(alone: bool) -> Vec<[Vec2; 3]> {
        let mut ed = Editor::new(Document::new());
        let layer = Document::default_layer();
        let mut pt = |x: f64, y: f64| {
            ed.apply(&Command::AddEntity {
                layer,
                construction: false,
                style: Style::default(),
                geom: Geom::Point {
                    pos: Vec2::new(x, y),
                },
            })
            .unwrap()
            .entities[0]
        };
        let p = [pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0), pt(0.0, 10.0)];
        let mut style = Style {
            width: 1.0,
            ..Style::default()
        };
        style.line.profile = Some(WidthProfile {
            alone,
            ..ProfilePreset::TaperBoth.profile(1)
        });
        for i in 0..4 {
            ed.apply(&Command::AddEntity {
                layer,
                construction: false,
                style: style.clone(),
                geom: Geom::Line {
                    start: p[i],
                    end: p[(i + 1) % 4],
                },
            })
            .unwrap();
        }
        let doc = ed.document();
        let scene = Scene::build(doc, &BTreeSet::new(), &BTreeSet::new());
        triangles(doc, &scene)
    }

    /// #527: sides with a "This curve" profile each taper on their own;
    /// without it the rectangle is one stroke whose profile runs round it.
    #[test]
    fn alone_profiles_stroke_each_side_separately() {
        let chained = rect_tris(false);
        let alone = rect_tris(true);
        assert!(!chained.is_empty() && !alone.is_empty());
        assert_ne!(chained, alone);
        // Four sides each taper to nothing at both ends; one closed chain
        // tapers once over its whole length, so it covers more area.
        let area = |tris: &[[Vec2; 3]]| {
            tris.iter()
                .map(|t| ((t[1] - t[0]).cross(t[2] - t[0]) / 2.0).abs())
                .sum::<f64>()
        };
        assert!(area(&alone) < area(&chained));
    }
}
