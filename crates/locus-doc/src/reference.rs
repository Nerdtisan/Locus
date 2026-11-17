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
    let mut next = doc.next_id();
    let mut alloc = || {
        let id = next;
        next += 1;
        id
    };
    let layer = LayerId::new(alloc());
    let mut commands = vec![Command::AddLayer {
        name: name.to_owned(),
        colour: REFERENCE_COLOUR,
        operation: Operation::Reference,
    }];
    let mut points: HashMap<(u64, u64), EntityId> = HashMap::new();
    let mut point = |p: Vec2, commands: &mut Vec<Command>, alloc: &mut dyn FnMut() -> u64| {
        *points
            .entry((p.x.to_bits(), p.y.to_bits()))
            .or_insert_with(|| {
                commands.push(Command::AddEntity {
                    layer,
                    construction: false,
                    style: Default::default(),
                    geom: Geom::Point { pos: p },
                });
                EntityId::new(alloc())
            })
    };
    for shape in shapes {
        let geom = match *shape {
            ImportShape::Line { a, b } => {
                let (start, end) = (
                    point(a, &mut commands, &mut alloc),
                    point(b, &mut commands, &mut alloc),
                );
                if start == end {
                    continue;
                }
                Geom::Line { start, end }
            }
            ImportShape::Arc { center, start, end } => Geom::Arc {
                center: point(center, &mut commands, &mut alloc),
                start: point(start, &mut commands, &mut alloc),
                end: point(end, &mut commands, &mut alloc),
            },
            ImportShape::Circle { center, radius } => Geom::Circle {
                center: point(center, &mut commands, &mut alloc),
                radius,
            },
            ImportShape::Cubic { a, c1, c2, b } => Geom::Bezier {
                start: point(a, &mut commands, &mut alloc),
                start_ctrl: point(c1, &mut commands, &mut alloc),
                end_ctrl: point(c2, &mut commands, &mut alloc),
                end: point(b, &mut commands, &mut alloc),
            },
        };
        commands.push(Command::AddEntity {
            layer,
            construction: false,
            style: Default::default(),
            geom,
        });
        alloc();
    }
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

/// Largest calibration factor in one step (and the inverse, the smallest).
pub const MAX_SCALE_FACTOR: f64 = 1.0e6;

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
    use crate::command::{Patch, record};
    let l = doc.layer(layer).ok_or(DocError::UnknownLayer(layer))?;
    if !matches!(l.operation, Operation::Reference) {
        return Err(DocError::Invalid(
            "only a Reference layer can be calibrated".to_owned(),
        ));
    }
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
    for (id, before) in on_layer {
        let geom = match &before.geom {
            Geom::Point { pos } => Geom::Point {
                pos: about + (*pos - about) * factor,
            },
            Geom::Circle { center, radius } => Geom::Circle {
                center: *center,
                radius: radius * factor,
            },
            Geom::Text { .. } => {
                return Err(DocError::Invalid(
                    "a reference with text cannot be calibrated".to_owned(),
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
    if let Some(before) = image {
        let p = before.placement;
        let after = crate::image::RefImage {
            placement: crate::image::ImagePlacement {
                origin: about + (p.origin - about) * factor,
                mm_per_px: p.mm_per_px * factor,
                ..p
            },
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
