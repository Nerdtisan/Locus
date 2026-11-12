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

/// The command that adds `paths` on a new locked Reference layer named `name`.
///
/// IDs are predicted from [`Document::next_id`]: commands in a compound
/// allocate in order, one ID each, so the layer gets the next ID and every
/// point and curve the ones after it. Apply the command to the same document
/// it was built from. End points shared bit-for-bit become one point, so
/// imported paths stay connected.
pub fn import_command(doc: &Document, name: &str, paths: &[GenPath]) -> Result<Command, DocError> {
    if paths.iter().all(|p| p.segments.is_empty()) {
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
                    geom: Geom::Point { pos: p },
                });
                EntityId::new(alloc())
            })
    };
    for seg in paths.iter().flat_map(|p| &p.segments) {
        let geom = match seg.shape {
            GenShape::Line { a, b } => {
                let (start, end) = (
                    point(a, &mut commands, &mut alloc),
                    point(b, &mut commands, &mut alloc),
                );
                if start == end {
                    continue;
                }
                Geom::Line { start, end }
            }
            GenShape::Arc { center, start, end } => Geom::Arc {
                center: point(center, &mut commands, &mut alloc),
                start: point(start, &mut commands, &mut alloc),
                end: point(end, &mut commands, &mut alloc),
            },
            GenShape::Circle { center, radius } => Geom::Circle {
                center: point(center, &mut commands, &mut alloc),
                radius,
            },
        };
        commands.push(Command::AddEntity {
            layer,
            construction: false,
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
