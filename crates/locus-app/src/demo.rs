// SPDX-License-Identifier: AGPL-3.0-or-later
//! Built-in sample documents, so the canvas has something to draw until file
//! open (M1) lands. Built only through the command API (SPEC §11).

use locus_doc::{Command, Constraint, Dimension, Document, Editor, EntityId, Geom};
use locus_geom::Vec2;

struct Builder {
    ed: Editor,
}

impl Builder {
    fn new() -> Self {
        Self {
            ed: Editor::new(Document::new()),
        }
    }

    fn add(&mut self, construction: bool, geom: Geom) -> Option<EntityId> {
        let cmd = Command::AddEntity {
            layer: Document::default_layer(),
            construction,
            style: Default::default(),
            geom,
        };
        self.ed.apply(&cmd).ok()?.entities.first().copied()
    }

    fn point(&mut self, x: f64, y: f64) -> Option<EntityId> {
        self.add(
            false,
            Geom::Point {
                pos: Vec2::new(x, y),
            },
        )
    }

    fn line(&mut self, a: EntityId, b: EntityId, construction: bool) -> Option<EntityId> {
        self.add(construction, Geom::Line { start: a, end: b })
    }

    fn constrain(&mut self, constraint: Constraint) {
        // A rejected constraint only means a sparser demo.
        let _ = self.ed.apply(&Command::AddConstraint { constraint });
    }

    fn finish(self) -> Document {
        self.ed.into_document()
    }
}

/// A plate with a hole, an arc, a construction diagonal and a few constraints.
pub fn sketch() -> Document {
    let mut b = Builder::new();
    build_sketch(&mut b);
    b.finish()
}

fn build_sketch(b: &mut Builder) -> Option<()> {
    let p00 = b.point(20.0, 20.0)?;
    let p10 = b.point(220.0, 20.0)?;
    let p11 = b.point(220.0, 140.0)?;
    let p01 = b.point(20.0, 140.0)?;
    let bottom = b.line(p00, p10, false)?;
    let right = b.line(p10, p11, false)?;
    let top = b.line(p11, p01, false)?;
    let left = b.line(p01, p00, false)?;
    let diag = b.line(p00, p11, true)?;

    let hc = b.point(80.0, 80.0)?;
    let hole = b.add(
        false,
        Geom::Circle {
            center: hc,
            radius: 20.0,
        },
    )?;

    // A quarter arc centred on the middle of the construction diagonal.
    let ac = b.point(120.0, 80.0)?;
    let a0 = b.point(140.0, 80.0)?;
    let a1 = b.point(120.0, 100.0)?;
    let arc = b.add(
        false,
        Geom::Arc {
            center: ac,
            start: a0,
            end: a1,
        },
    )?;
    let _free = b.point(130.0, 110.0)?;

    b.constrain(Constraint::Fix { point: p00 });
    b.constrain(Constraint::Horizontal { line: bottom });
    b.constrain(Constraint::Horizontal { line: top });
    b.constrain(Constraint::Vertical { line: right });
    b.constrain(Constraint::Vertical { line: left });
    b.constrain(Constraint::HorizontalDistance {
        a: p00,
        b: p10,
        dim: Dimension::driving(200.0),
    });
    b.constrain(Constraint::VerticalDistance {
        a: p10,
        b: p11,
        dim: Dimension::driving(120.0),
    });
    b.constrain(Constraint::Fix { point: hc });
    b.constrain(Constraint::Diameter {
        curve: hole,
        dim: Dimension::driving(40.0),
    });
    b.constrain(Constraint::Radius {
        curve: arc,
        dim: Dimension::driving(20.0),
    });
    b.constrain(Constraint::Midpoint {
        point: ac,
        line: diag,
    });
    Some(())
}

/// Deterministic scatter of about `n` lines, circles and arcs (plus their
/// points) over a 600 × 400 mm area, for the 5 000-entity smoothness check.
pub fn stress(n: usize) -> Document {
    let mut b = Builder::new();
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut rnd = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    for i in 0..n {
        let (x, y) = (rnd() * 600.0, rnd() * 400.0);
        let construction = i % 11 == 0;
        let _ = match i % 4 {
            0 | 1 => {
                let (dx, dy) = (rnd() * 30.0 - 15.0, rnd() * 30.0 - 15.0);
                b.point(x, y)
                    .zip(b.point(x + dx, y + dy))
                    .and_then(|(p, q)| b.line(p, q, construction))
            }
            2 => b.point(x, y).and_then(|c| {
                b.add(
                    construction,
                    Geom::Circle {
                        center: c,
                        radius: 1.0 + rnd() * 8.0,
                    },
                )
            }),
            _ => {
                let r = 2.0 + rnd() * 8.0;
                let a0 = rnd() * std::f64::consts::TAU;
                let a1 = a0 + 0.5 + rnd() * 2.5;
                let at = |a: f64| Vec2::new(x + r * a.cos(), y + r * a.sin());
                let (s, e) = (at(a0), at(a1));
                b.point(x, y).zip(b.point(s.x, s.y)).and_then(|(c, ps)| {
                    let pe = b.point(e.x, e.y)?;
                    b.add(
                        construction,
                        Geom::Arc {
                            center: c,
                            start: ps,
                            end: pe,
                        },
                    )
                })
            }
        };
    }
    b.finish()
}
