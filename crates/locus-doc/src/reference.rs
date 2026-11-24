// SPDX-License-Identifier: AGPL-3.0-or-later

//! Imported reference geometry (SPEC §15.2 importers, §13 M4 reference import).
//!
//! An import lands as a new, locked layer with the Reference operation, built
//! as one [`Command::Compound`], so it is a single undo step and never touches
//! existing geometry. Importer plugins (#233) use it today; the SVG and DXF
//! importers (#241, #242) can share it.

use std::collections::HashMap;

use locus_geom::Vec2;

use crate::command::Command;
use crate::document::Document;
use crate::entity::Geom;
use crate::error::DocError;
use crate::generated::{GenPath, GenShape};
use crate::ids::{EntityId, LayerId};
use crate::operation::Operation;
use crate::transform::Transform;

/// Colour of imported reference layers: neutral grey, so it reads as "not mine".
pub const REFERENCE_COLOUR: [u8; 3] = [128, 128, 128];

/// One piece of imported geometry, in millimetres. Unlike [`GenShape`] it can
/// carry a cubic Bézier, which SVG and DXF imports produce (§13 M4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImportShape {
    Line {
        a: Vec2,
        b: Vec2,
    },
    /// Counter-clockwise from `start` to `end` about `center`.
    Arc {
        center: Vec2,
        start: Vec2,
        end: Vec2,
    },
    Circle {
        center: Vec2,
        radius: f64,
    },
    Cubic {
        a: Vec2,
        c1: Vec2,
        c2: Vec2,
        b: Vec2,
    },
}

impl From<GenShape> for ImportShape {
    fn from(s: GenShape) -> Self {
        match s {
            GenShape::Line { a, b } => ImportShape::Line { a, b },
            GenShape::Arc { center, start, end } => ImportShape::Arc { center, start, end },
            GenShape::Circle { center, radius } => ImportShape::Circle { center, radius },
        }
    }
}

/// The command that adds `paths` on a new locked Reference layer named `name`.
/// See [`import_shapes_command`].
pub fn import_command(doc: &Document, name: &str, paths: &[GenPath]) -> Result<Command, DocError> {
    let shapes: Vec<ImportShape> = paths
        .iter()
        .flat_map(|p| &p.segments)
        .map(|s| s.shape.into())
        .collect();
    import_shapes_command(doc, name, &shapes)
}

/// The command that adds `shapes` on a new locked Reference layer named `name`.
///
/// IDs are predicted from [`Document::next_id`]: commands in a compound
/// allocate in order, one ID each, so the layer gets the next ID and every
/// point and curve the ones after it. Apply the command to the same document
/// it was built from. End points shared bit-for-bit become one point, so
/// imported paths stay connected.
pub fn import_shapes_command(
    doc: &Document,
    name: &str,
    shapes: &[ImportShape],
) -> Result<Command, DocError> {
    if shapes.is_empty() {
        return Err(DocError::Invalid("there is no geometry to import".into()));
    }
    let mut ids = Ids(doc.next_id());
    let layer = LayerId::new(ids.next());
    let mut commands = vec![Command::AddLayer {
        name: name.to_owned(),
        colour: REFERENCE_COLOUR,
        operation: Operation::Reference,
    }];
    push_shapes(&mut commands, &mut ids, layer, shapes);
    // Locked last: a locked layer refuses new geometry (SPEC §4 rule 3).
    commands.push(Command::SetLayerLocked {
        id: layer,
        locked: true,
    });
    Ok(Command::Compound {
        label: format!("Import {name}"),
        commands,
    })
}

/// Where a group of shapes goes in [`shapes_command`].
#[derive(Debug, Clone, PartialEq)]
pub enum ShapeTarget {
    /// An existing, unlocked layer.
    Layer(LayerId),
    /// A new layer, made first.
    NewLayer {
        name: String,
        colour: [u8; 3],
        operation: Operation,
    },
}

/// Shapes for one layer.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeGroup {
    pub target: ShapeTarget,
    pub shapes: Vec<ImportShape>,
}

/// One compound command, labelled `label`, that adds every group's shapes
/// on its layer: ordinary, editable geometry (Auto trace, #461). Shared end
/// points join, as on import. IDs are predicted as in
/// [`import_shapes_command`], so apply it to the document it was built from.
///
/// # Errors
/// [`DocError::Invalid`] when there's nothing to add, or a target layer is
/// missing or locked.
pub fn shapes_command(
    doc: &Document,
    label: &str,
    groups: &[ShapeGroup],
) -> Result<Command, DocError> {
    if groups.iter().all(|g| g.shapes.is_empty()) {
        return Err(DocError::Invalid("there is no geometry to add".into()));
    }
    let mut ids = Ids(doc.next_id());
    let mut commands = Vec::new();
    for g in groups.iter().filter(|g| !g.shapes.is_empty()) {
        let layer = match &g.target {
            ShapeTarget::Layer(l) => {
                let layer = doc.layer(*l).ok_or(DocError::UnknownLayer(*l))?;
                if layer.locked {
                    return Err(DocError::Invalid("that layer is locked".into()));
                }
                *l
            }
            ShapeTarget::NewLayer {
                name,
                colour,
                operation,
            } => {
                commands.push(Command::AddLayer {
                    name: name.clone(),
                    colour: *colour,
                    operation: *operation,
                });
                LayerId::new(ids.next())
            }
        };
        push_shapes(&mut commands, &mut ids, layer, &g.shapes);
    }
    Ok(Command::Compound {
        label: label.to_owned(),
        commands,
    })
}

/// IDs as the compound's commands will allocate them, in order.
struct Ids(u64);

impl Ids {
    fn next(&mut self) -> u64 {
        let id = self.0;
        self.0 += 1;
        id
    }
}

/// `AddEntity` commands for `shapes` on `layer`: points first where new,
/// shared bit-for-bit, then each curve.
fn push_shapes(commands: &mut Vec<Command>, ids: &mut Ids, layer: LayerId, shapes: &[ImportShape]) {
    let mut points: HashMap<(u64, u64), EntityId> = HashMap::new();
    let mut point = |p: Vec2, commands: &mut Vec<Command>, ids: &mut Ids| {
        *points
            .entry((p.x.to_bits(), p.y.to_bits()))
            .or_insert_with(|| {
                commands.push(Command::AddEntity {
                    layer,
                    construction: false,
                    style: Default::default(),
                    geom: Geom::Point { pos: p },
                });
                EntityId::new(ids.next())
            })
    };
    for shape in shapes {
        let geom = match *shape {
            ImportShape::Line { a, b } => {
                let (start, end) = (point(a, commands, ids), point(b, commands, ids));
                if start == end {
                    continue;
                }
                Geom::Line { start, end }
            }
            ImportShape::Arc { center, start, end } => Geom::Arc {
                center: point(center, commands, ids),
                start: point(start, commands, ids),
                end: point(end, commands, ids),
            },
            ImportShape::Circle { center, radius } => Geom::Circle {
                center: point(center, commands, ids),
                radius,
            },
            ImportShape::Cubic { a, c1, c2, b } => Geom::Bezier {
                start: point(a, commands, ids),
                start_ctrl: point(c1, commands, ids),
                end_ctrl: point(c2, commands, ids),
                end: point(b, commands, ids),
            },
        };
        commands.push(Command::AddEntity {
            layer,
            construction: false,
            style: Default::default(),
            geom,
        });
        ids.next();
    }
}

/// Largest calibration factor in one step (and the inverse, the smallest).
pub const MAX_SCALE_FACTOR: f64 = 1.0e6;

/// A reference's anchor corner (#464): an image's placement origin (its
/// bottom-left corner); otherwise the bottom-left corner of the layer's
/// geometry. Scaling about it keeps a reference placed at 0,0 at 0,0.
#[must_use]
pub fn anchor(doc: &Document, layer: LayerId) -> Option<Vec2> {
    if let Some(img) = doc.image(layer) {
        // The image's own bottom-left, where it is drawn (turned, #463).
        return Some(img.corners()[0]);
    }
    geometry_bounds(doc, layer).map(|b| b.min)
}

/// A reference's centre (#468): the middle of everything on the layer, its
/// image and its geometry. Scaling about it keeps a reference centred where
/// it is (the default pivot).
#[must_use]
pub fn centre(doc: &Document, layer: LayerId) -> Option<Vec2> {
    let image = doc.image(layer).map(crate::image::RefImage::bounds);
    let b = match (image, geometry_bounds(doc, layer)) {
        (Some(a), Some(b)) => a.union(b),
        (a, b) => a.or(b)?,
    };
    Some((b.min + b.max) * 0.5)
}

/// The bounding box of the curves and points on `layer`.
fn geometry_bounds(doc: &Document, layer: LayerId) -> Option<locus_geom::Aabb> {
    doc.entities()
        .iter()
        .filter(|(_, e)| e.layer == layer)
        .filter_map(|(id, e)| match e.geom {
            Geom::Point { pos } => Some(locus_geom::Aabb::from_points(pos, pos)),
            Geom::Line { .. } => doc.line(id).map(|l| l.bbox()),
            Geom::Circle { .. } => doc.circle(id).map(|c| c.bbox()),
            Geom::Arc { .. } => doc.arc(id).map(|a| a.bbox()),
            Geom::Bezier { .. } => doc.cubic(id).map(|c| c.bbox()),
            Geom::Text { .. } => None,
        })
        .reduce(locus_geom::Aabb::union)
}

/// [`Command::ScaleReference`] (#391): every point on `layer` moves away
/// from `about` by `factor`, and circle radii and the image follow. Lines,
/// arcs and Béziers hang off their points, so they scale with them.
pub(crate) fn scale(
    doc: &mut Document,
    layer: LayerId,
    about: Vec2,
    factor: f64,
    patches: &mut Vec<crate::command::Patch>,
) -> Result<(), DocError> {
    if !(factor.is_finite() && factor > 1.0 / MAX_SCALE_FACTOR && factor < MAX_SCALE_FACTOR) {
        return Err(DocError::BadValue {
            what: "calibration factor",
            value: factor,
        });
    }
    if !(about.x.is_finite() && about.y.is_finite()) {
        return Err(DocError::BadValue {
            what: "calibration point",
            value: about.x,
        });
    }
    let t = Transform::Scale {
        center: about,
        factor,
    };
    transform(doc, layer, &t, patches)
}

/// [`Command::TransformReference`] (#463, generalising Calibrate's scale):
/// move, turn or scale everything on Reference layer `layer` by `t`. Points
/// go where `t` takes them, circle radii scale with it, and the image's
/// placement follows ([`crate::image::RefImage::transformed_placement`]).
/// Lines, arcs and Béziers hang off their points. Allowed while the layer is
/// locked: the lock only stops editing the reference by hand.
pub(crate) fn transform(
    doc: &mut Document,
    layer: LayerId,
    t: &Transform,
    patches: &mut Vec<crate::command::Patch>,
) -> Result<(), DocError> {
    use crate::command::{Patch, record};
    let l = doc.layer(layer).ok_or(DocError::UnknownLayer(layer))?;
    if !matches!(l.operation, Operation::Reference) {
        return Err(DocError::Invalid(
            "only a Reference layer can be placed this way".to_owned(),
        ));
    }
    let bad = |what| DocError::BadValue {
        what,
        value: f64::NAN,
    };
    if !t.valid() {
        return Err(bad("reference transform"));
    }
    let factors = match *t {
        Transform::Scale { factor, .. } => vec![factor],
        Transform::Stretch { sx, sy, .. } => vec![sx, sy],
        _ => Vec::new(),
    };
    for factor in factors {
        if !(factor > 1.0 / MAX_SCALE_FACTOR && factor < MAX_SCALE_FACTOR) {
            return Err(DocError::BadValue {
                what: "reference scale",
                value: factor,
            });
        }
    }
    let on_layer: Vec<(EntityId, crate::entity::Entity)> = doc
        .entities()
        .iter()
        .filter(|(_, e)| e.layer == layer)
        .map(|(id, e)| (id, e.clone()))
        .collect();
    let image = doc.image(layer).cloned();
    if on_layer.is_empty() && image.is_none() {
        return Err(DocError::Invalid("this reference is empty".to_owned()));
    }
    // A free X/Y scale can't keep circles and arcs: they become Béziers,
    // which the scale maps exactly (#463, owner decision 2026-11-24).
    let curved: Vec<EntityId> = if matches!(t, Transform::Stretch { .. }) {
        on_layer
            .iter()
            .filter(|(_, e)| matches!(e.geom, Geom::Circle { .. } | Geom::Arc { .. }))
            .map(|(id, _)| *id)
            .collect()
    } else {
        Vec::new()
    };
    for &id in &curved {
        if doc
            .constraints()
            .values()
            .any(|c| c.references().contains(&id))
        {
            return Err(DocError::Invalid(
                "a constraint holds a circle or arc of this reference, so it can only scale uniformly"
                    .to_owned(),
            ));
        }
    }
    let mut centres = Vec::new();
    for &id in &curved {
        curve_to_beziers(doc, id, t, patches, &mut centres)?;
    }
    for (id, before) in on_layer {
        if curved.contains(&id) {
            continue;
        }
        let geom = match &before.geom {
            Geom::Point { pos } => Geom::Point { pos: t.point(*pos) },
            Geom::Circle { center, radius } => Geom::Circle {
                center: *center,
                radius: t.length(*radius),
            },
            Geom::Text { .. } => {
                return Err(DocError::Invalid(
                    "a reference with text cannot be moved, turned or scaled".to_owned(),
                ));
            }
            g => g.clone(),
        };
        if geom == before.geom {
            continue;
        }
        let after = crate::entity::Entity {
            geom,
            ..before.clone()
        };
        doc.check_entity(&after)?;
        record(
            doc,
            patches,
            Patch::Entity {
                id,
                before: Some(before),
                after: Some(after),
            },
        );
    }
    // Centres that only placed the converted circles and arcs go too.
    for c in centres {
        let used = doc
            .entities()
            .iter()
            .any(|(_, e)| e.geom.point_refs().contains(&c))
            || doc
                .constraints()
                .values()
                .any(|k| k.references().contains(&c));
        if let (false, Some(before)) = (used, doc.entity(c).cloned()) {
            record(
                doc,
                patches,
                Patch::Entity {
                    id: c,
                    before: Some(before),
                    after: None,
                },
            );
        }
    }
    if let Some(before) = image {
        let placement = before.transformed_placement(t).ok_or_else(|| {
            DocError::Invalid("a free scale of an image goes along its own edges".to_owned())
        })?;
        let after = crate::image::RefImage {
            placement,
            ..before.clone()
        };
        after.validate()?;
        record(
            doc,
            patches,
            Patch::Image {
                layer,
                before: Some(before),
                after: Some(after),
            },
        );
    }
    Ok(())
}

/// Replace circle or arc `id` by cubic Béziers within `EXPORT_TOL`, mapped
/// by `t` (a free X/Y scale), keeping an arc's end points so its path stays
/// joined. Its centre is pushed onto `centres` for the caller to tidy.
fn curve_to_beziers(
    doc: &mut Document,
    id: EntityId,
    t: &Transform,
    patches: &mut Vec<crate::command::Patch>,
    centres: &mut Vec<EntityId>,
) -> Result<(), DocError> {
    use crate::command::{Patch, record};
    use locus_geom::{EXPORT_TOL, arc_cubics};
    let before = doc.entity(id).cloned().ok_or(DocError::UnknownEntity(id))?;
    let (pieces, ends, centre) = match before.geom {
        Geom::Circle { center, radius } => {
            let c = doc
                .point_pos(center)
                .ok_or(DocError::UnknownEntity(center))?;
            let full = arc_cubics(c, radius, 0.0, std::f64::consts::TAU, EXPORT_TOL);
            (full, None, center)
        }
        Geom::Arc { center, start, end } => {
            let arc = doc.arc(id).ok_or(DocError::UnknownEntity(id))?;
            (arc.to_cubics(EXPORT_TOL), Some((start, end)), center)
        }
        _ => return Ok(()),
    };
    if pieces.is_empty() {
        return Ok(());
    }
    centres.push(centre);
    record(
        doc,
        patches,
        Patch::Entity {
            id,
            before: Some(before.clone()),
            after: None,
        },
    );
    let point =
        |doc: &mut Document, patches: &mut Vec<Patch>, p: Vec2| -> Result<EntityId, DocError> {
            let pid = EntityId::new(doc.alloc_id()?);
            let e = crate::entity::Entity {
                layer: before.layer,
                construction: before.construction,
                geom: Geom::Point { pos: t.point(p) },
                style: Default::default(),
            };
            record(
                doc,
                patches,
                Patch::Entity {
                    id: pid,
                    before: None,
                    after: Some(e),
                },
            );
            Ok(pid)
        };
    let n = pieces.len();
    // Anchors: an arc keeps its own end points (moved by the caller's point
    // pass); a circle closes on its first new anchor.
    let first = match ends {
        Some((s, _)) => s,
        None => point(doc, patches, pieces[0].p0)?,
    };
    let mut prev = first;
    for (k, c) in pieces.iter().enumerate() {
        let last = k + 1 == n;
        let next = match (last, ends) {
            (true, Some((_, e))) => e,
            (true, None) => first,
            (false, _) => point(doc, patches, c.p1)?,
        };
        let geom = Geom::Bezier {
            start: prev,
            start_ctrl: point(doc, patches, c.c0)?,
            end_ctrl: point(doc, patches, c.c1)?,
            end: next,
        };
        let bid = EntityId::new(doc.alloc_id()?);
        let e = crate::entity::Entity {
            layer: before.layer,
            construction: before.construction,
            geom,
            style: before.style.clone(),
        };
        doc.check_entity(&e)?;
        record(
            doc,
            patches,
            Patch::Entity {
                id: bid,
                before: None,
                after: Some(e),
            },
        );
        prev = next;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Editor;
    use crate::generated::GenSegment;

    fn line(a: (f64, f64), b: (f64, f64)) -> GenSegment {
        GenSegment {
            shape: GenShape::Line {
                a: Vec2::new(a.0, a.1),
                b: Vec2::new(b.0, b.1),
            },
            reversed: false,
        }
    }

    /// #461: traced shapes land on an existing layer and on new layers as
    /// one undo step, with shared end points joined per layer.
    #[test]
    fn shapes_command_adds_groups_as_one_step() {
        let mut ed = Editor::new(Document::new());
        let sq = |o: f64| -> Vec<ImportShape> {
            let p =
                [(0.0, 0.0), (5.0, 0.0), (5.0, 5.0), (0.0, 5.0)].map(|(x, y)| Vec2::new(x + o, y));
            (0..4)
                .map(|k| ImportShape::Line {
                    a: p[k],
                    b: p[(k + 1) % 4],
                })
                .collect()
        };
        let groups = [
            ShapeGroup {
                target: ShapeTarget::Layer(Document::default_layer()),
                shapes: sq(0.0),
            },
            ShapeGroup {
                target: ShapeTarget::NewLayer {
                    name: "Trace red".into(),
                    colour: [200, 0, 0],
                    operation: Operation::Draw,
                },
                shapes: sq(10.0),
            },
        ];
        let cmd = shapes_command(ed.document(), "Auto trace", &groups).unwrap();
        ed.apply(&cmd).unwrap();
        let doc = ed.document();
        assert_eq!(doc.layers().len(), 2);
        // 4 shared points + 4 lines per square.
        assert_eq!(doc.entities().len(), 16);
        assert_eq!(ed.undo_label(), Some("Auto trace"));
        ed.undo().unwrap();
        assert!(ed.document().entities().is_empty());
        assert!(shapes_command(ed.document(), "x", &[]).is_err());
    }

    /// #463 (owner decision): a free X/Y scale stretches a vector reference;
    /// its circles and arcs become Béziers on the stretched ellipse, joined
    /// to the rest, as one undo step; a constraint on a circle refuses it.
    #[test]
    fn a_free_scale_turns_circles_and_arcs_into_curves() {
        let mut ed = Editor::new(Document::new());
        let arc = GenSegment {
            shape: GenShape::Arc {
                center: Vec2::new(10.0, 0.0),
                start: Vec2::new(15.0, 0.0),
                end: Vec2::new(10.0, 5.0),
            },
            reversed: false,
        };
        let paths = [
            GenPath {
                segments: vec![line((0.0, 0.0), (15.0, 0.0)), arc],
                closed: false,
            },
            GenPath {
                segments: vec![GenSegment {
                    shape: GenShape::Circle {
                        center: Vec2::new(0.0, 20.0),
                        radius: 4.0,
                    },
                    reversed: false,
                }],
                closed: true,
            },
        ];
        let cmd = import_command(ed.document(), "plan.dxf", &paths).unwrap();
        let layer = ed.apply(&cmd).unwrap().layers[0];
        let before = ed.document().clone();
        let stretch = Transform::Stretch {
            center: Vec2::ZERO,
            sx: 2.0,
            sy: 0.5,
            angle: 0.0,
        };
        ed.apply(&Command::TransformReference {
            layer,
            transform: stretch,
        })
        .unwrap();
        assert_eq!(ed.undo_label(), Some("Scale reference"));
        let doc = ed.document();
        let kinds: Vec<&Geom> = doc.entities().iter().map(|(_, e)| &e.geom).collect();
        assert!(
            !kinds
                .iter()
                .any(|g| matches!(g, Geom::Circle { .. } | Geom::Arc { .. }))
        );
        let curves: Vec<_> = doc
            .entities()
            .iter()
            .filter_map(|(id, _)| doc.cubic(id))
            .collect();
        assert!(curves.len() >= 5, "{}", curves.len());
        // The circle's curves lie on the ellipse x²/8² + (y-10)²/2² = 1.
        for c in &curves {
            for k in 0..=8 {
                let p = c.eval(f64::from(k) / 8.0);
                if p.y > 5.0 {
                    let e = (p.x / 8.0).powi(2) + ((p.y - 10.0) / 2.0).powi(2);
                    assert!((e - 1.0).abs() < 1e-3, "{p:?} {e}");
                }
            }
        }
        // The line still meets the arc's curves at (30, 0).
        let joined = doc.entities().iter().filter(|(_, e)| match e.geom {
            Geom::Bezier { start, .. } => doc.point_pos(start) == Some(Vec2::new(30.0, 0.0)),
            _ => false,
        });
        assert_eq!(joined.count(), 1);
        ed.undo().unwrap();
        assert_eq!(ed.document().entities(), before.entities());
        // A constraint on the circle keeps it uniform-only.
        let circle = before
            .entities()
            .iter()
            .find(|(_, e)| matches!(e.geom, Geom::Circle { .. }))
            .map(|(id, _)| id)
            .unwrap();
        ed.apply(&Command::SetLayerLocked {
            id: layer,
            locked: false,
        })
        .unwrap();
        ed.apply(&Command::AddConstraint {
            constraint: crate::constraint::Constraint::Diameter {
                curve: circle,
                dim: crate::constraint::Dimension::driving(8.0),
            },
        })
        .unwrap();
        assert!(
            ed.apply(&Command::TransformReference {
                layer,
                transform: stretch
            })
            .is_err()
        );
    }

    /// #463: a vector reference turns about a point on its locked layer,
    /// as one undo step; a drawing layer is refused.
    #[test]
    fn transform_reference_turns_a_vector_reference() {
        let mut ed = Editor::new(Document::new());
        let path = GenPath {
            segments: vec![line((0.0, 0.0), (10.0, 0.0))],
            closed: false,
        };
        let cmd = import_command(ed.document(), "plan.dxf", &[path]).unwrap();
        let layer = ed.apply(&cmd).unwrap().layers[0];
        let before = ed.document().clone();
        ed.apply(&Command::TransformReference {
            layer,
            transform: Transform::Rotate {
                center: Vec2::ZERO,
                angle: std::f64::consts::FRAC_PI_2,
            },
        })
        .unwrap();
        let doc = ed.document();
        let pos: Vec<Vec2> = doc
            .entities()
            .iter()
            .filter_map(|(id, _)| doc.point_pos(id))
            .collect();
        assert!(
            pos.iter().any(|p| p.distance(Vec2::new(0.0, 10.0)) < 1e-12),
            "{pos:?}"
        );
        assert_eq!(ed.undo_label(), Some("Rotate reference"));
        ed.undo().unwrap();
        assert_eq!(ed.document().entities(), before.entities());
        let refused = ed.apply(&Command::TransformReference {
            layer: Document::default_layer(),
            transform: Transform::Move(Vec2::new(1.0, 0.0)),
        });
        assert!(refused.is_err());
        let bad = ed.apply(&Command::TransformReference {
            layer,
            transform: Transform::Scale {
                center: Vec2::ZERO,
                factor: f64::NAN,
            },
        });
        assert!(bad.is_err());
    }

    /// #391: calibrating scales the reference's points, radii and image
    /// about the first picked point, on the locked layer, as one undo step,
    /// and is stored in the document.
    #[test]
    fn calibrate_scales_the_reference_about_a_point() {
        let mut ed = Editor::new(Document::new());
        let corner = GenPath {
            segments: vec![
                line((0.0, 0.0), (10.0, 0.0)),
                line((10.0, 0.0), (10.0, 10.0)),
            ],
            closed: false,
        };
        let circle = GenPath {
            segments: vec![GenSegment {
                shape: GenShape::Circle {
                    center: Vec2::new(5.0, 5.0),
                    radius: 2.0,
                },
                reversed: false,
            }],
            closed: true,
        };
        let cmd = import_command(ed.document(), "plan.dxf", &[corner, circle]).unwrap();
        let layer = ed.apply(&cmd).unwrap().layers[0];
        assert!(ed.document().layer(layer).unwrap().locked);
        let before = ed.document().clone();
        // The 10 mm edge is really 25 mm: factor 2.5 about (0, 0).
        ed.apply(&Command::ScaleReference {
            layer,
            about: Vec2::ZERO,
            factor: 2.5,
        })
        .unwrap();
        let doc = ed.document();
        let pos: Vec<Vec2> = doc
            .entities()
            .iter()
            .filter_map(|(id, _)| doc.point_pos(id))
            .collect();
        assert!(pos.contains(&Vec2::new(25.0, 0.0)), "{pos:?}");
        assert!(pos.contains(&Vec2::new(12.5, 12.5)), "the circle's centre");
        let radius = doc
            .entities()
            .iter()
            .find_map(|(_, e)| match e.geom {
                Geom::Circle { radius, .. } => Some(radius),
                _ => None,
            })
            .unwrap();
        assert!((radius - 5.0).abs() < 1e-12);
        assert_eq!(ed.undo_label(), Some("Calibrate reference"));
        ed.undo().unwrap();
        assert_eq!(ed.document().entities(), before.entities());
        // #464: about its anchor (bottom-left, here 0,0) the reference
        // stays put, ×2 and ×0.5.
        let at = anchor(ed.document(), layer).unwrap();
        assert_eq!(at, Vec2::ZERO);
        for f in [2.0, 0.5] {
            ed.apply(&Command::ScaleReference {
                layer,
                about: anchor(ed.document(), layer).unwrap(),
                factor: f,
            })
            .unwrap();
            assert_eq!(anchor(ed.document(), layer).unwrap(), at, "×{f}");
        }
        ed.undo().unwrap();
        ed.undo().unwrap();
        // #468: about its centre, ×0.0875 then ×2, the centre stays put.
        let mid = centre(ed.document(), layer).unwrap();
        for f in [0.0875, 2.0] {
            ed.apply(&Command::ScaleReference {
                layer,
                about: centre(ed.document(), layer).unwrap(),
                factor: f,
            })
            .unwrap();
            let now = centre(ed.document(), layer).unwrap();
            assert!(now.distance(mid) < 1e-9, "×{f}: {now:?} vs {mid:?}");
        }
        // Only Reference layers, and only sane factors.
        for (l, f) in [
            (Document::default_layer(), 2.0),
            (layer, 0.0),
            (layer, f64::NAN),
            (layer, -1.0),
        ] {
            assert!(
                ed.apply(&Command::ScaleReference {
                    layer: l,
                    about: Vec2::ZERO,
                    factor: f,
                })
                .is_err()
            );
        }
    }

    #[test]
    fn import_is_one_locked_reference_layer_and_one_undo_step() {
        let mut ed = Editor::new(Document::new());
        let square = GenPath {
            segments: vec![
                line((0.0, 0.0), (10.0, 0.0)),
                line((10.0, 0.0), (10.0, 10.0)),
                line((10.0, 10.0), (0.0, 0.0)),
            ],
            closed: true,
        };
        let circle = GenPath {
            segments: vec![GenSegment {
                shape: GenShape::Circle {
                    center: Vec2::new(5.0, 5.0),
                    radius: 2.0,
                },
                reversed: false,
            }],
            closed: true,
        };
        let before = ed.document().clone();
        let cmd = import_command(ed.document(), "drawing.plt", &[square, circle]).unwrap();
        let applied = ed.apply(&cmd).unwrap();

        let layer = applied.layers[0];
        let l = ed.document().layer(layer).unwrap();
        assert!(l.locked);
        assert_eq!(l.operation, Operation::Reference);
        // 3 shared corners + 3 lines + circle centre + circle.
        assert_eq!(applied.entities.len(), 8);
        for id in &applied.entities {
            assert_eq!(ed.document().entities().get(*id).unwrap().layer, layer);
        }
        ed.undo().unwrap();
        assert_eq!(ed.document().layers(), before.layers());
        assert_eq!(ed.document().entities().len(), before.entities().len());
    }

    #[test]
    fn nothing_to_import_is_refused() {
        let doc = Document::new();
        assert!(import_command(&doc, "x", &[]).is_err());
        assert!(import_command(&doc, "x", &[GenPath::default()]).is_err());
    }
}
