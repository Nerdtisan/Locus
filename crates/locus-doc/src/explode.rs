// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exploding a plugin object while keeping its constraints (SPEC §15.3, §5;
//! #213).
//!
//! A generator returns constraints in its own, unrotated frame (ADR 0009).
//! While the object is live they are informational: the frame places it
//! rigidly. On explode they become ordinary constraints where they still
//! make sense:
//!
//! - **Orientation** constraints (horizontal, vertical) are skipped when the
//!   object is rotated: copied as they are, they would snap it back to the
//!   axes.
//! - Any constraint that would **conflict** with, or be **redundant** to, the
//!   sketch is dropped rather than refusing the explode. Each candidate is
//!   probed in turn on a scratch copy, exactly as adding it by hand would be
//!   checked (§5 Solver UX), so the kept set never over-constrains.
//!
//! The explode and every kept constraint go through the command API as one
//! [`Command::Compound`]: one undo step (§11). The caller tells the user how
//! many were dropped.

use crate::command::{Applied, Command};
use crate::constraint::{Constraint, Dimension};
use crate::error::DocError;
use crate::generated::{GenConstraint, GenEntity, frame};
use crate::ids::{EntityId, RecipeId};
use crate::recipe::RecipeKind;
use crate::solve::Sketcher;

/// What [`Sketcher::explode_plugin_object`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exploded {
    pub applied: Applied,
    /// Plugin constraints now in the sketch.
    pub kept: usize,
    /// Plugin constraints left out: orientation on a rotated object, or
    /// conflicting or redundant ones.
    pub dropped: usize,
}

/// Frame angles closer to zero than this count as unrotated (radians).
const ANGLE_TOL: f64 = locus_geom::SOLVER_TOL;

impl Sketcher {
    /// Explodes a recipe. For a generator plugin object, its constraints are
    /// kept where they still fit (see the module docs); any other recipe
    /// explodes as [`Command::ExplodeRecipe`] does.
    ///
    /// # Errors
    /// As [`Sketcher::apply`] for the explode itself. Dropping constraints
    /// is never an error.
    pub fn explode_plugin_object(&mut self, rid: RecipeId) -> Result<Exploded, DocError> {
        let plain = Command::ExplodeRecipe { id: rid };
        let candidates = candidates(self, rid)?;
        if candidates.total == 0 {
            let applied = self.apply(&plain)?;
            return Ok(Exploded {
                applied,
                kept: 0,
                dropped: 0,
            });
        }

        // Probe each constraint on a scratch copy, in the plugin's order.
        let mut scratch = Sketcher::new(self.document().clone());
        scratch.apply_unsolved(&plain)?;
        let mut kept = Vec::new();
        for c in candidates.constraints {
            let cmd = Command::AddConstraint { constraint: c };
            if scratch.precheck(&cmd).is_ok() && scratch.apply_unsolved(&cmd).is_ok() {
                kept.push(cmd);
            }
        }

        let n = kept.len();
        let mut commands = vec![plain.clone()];
        commands.extend(kept);
        let compound = Command::Compound {
            label: "Explode".to_owned(),
            commands,
        };
        match self.apply(&compound) {
            Ok(applied) => Ok(Exploded {
                applied,
                kept: n,
                dropped: candidates.total - n,
            }),
            // The kept set solved on the scratch copy, so this is unexpected;
            // still explode, without the constraints, rather than refuse.
            Err(_) => Ok(Exploded {
                applied: self.apply(&plain)?,
                kept: 0,
                dropped: candidates.total,
            }),
        }
    }
}

struct Candidates {
    /// Every constraint the plugin emitted, including those skipped here.
    total: usize,
    /// Those worth probing, mapped to document entities.
    constraints: Vec<Constraint>,
}

/// The plugin's constraints in document terms, minus orientation ones on a
/// rotated object and any whose entities can't be found.
fn candidates(s: &Sketcher, rid: RecipeId) -> Result<Candidates, DocError> {
    let doc = s.document();
    let r = doc.recipe(rid).ok_or(DocError::UnknownRecipe(rid))?;
    let RecipeKind::Generated { output, .. } = &r.kind else {
        return Ok(Candidates {
            total: 0,
            constraints: Vec::new(),
        });
    };
    let pool: &[EntityId] = r.pool.first().map_or(&[], Vec::as_slice);
    // Instances are laid out as every point, then every non-point entity in
    // order (`generated::regenerate`). A `point(i)` entity is that point.
    let n_points = output.points.len();
    let mut ids: Vec<Option<EntityId>> = Vec::with_capacity(output.entities.len());
    let mut curves = 0usize;
    for e in &output.entities {
        ids.push(match *e {
            GenEntity::Point(p) => pool.get(p as usize).copied(),
            _ => {
                let id = pool.get(n_points + curves).copied();
                curves += 1;
                id
            }
        });
    }
    let id = |i: u32| ids.get(i as usize).copied().flatten();
    let angle = frame(doc, &r.sources).map_or(0.0, |(_, a)| a);
    let rotated = angle.abs() > ANGLE_TOL;

    let mut out = Vec::new();
    for c in &output.constraints {
        let mapped = match *c {
            GenConstraint::Horizontal(_) | GenConstraint::Vertical(_) if rotated => None,
            GenConstraint::Horizontal(l) => id(l).map(|line| Constraint::Horizontal { line }),
            GenConstraint::Vertical(l) => id(l).map(|line| Constraint::Vertical { line }),
            GenConstraint::Coincident(a, b) => {
                pair(id(a), id(b), |a, b| Constraint::Coincident { a, b })
            }
            GenConstraint::Parallel(a, b) => {
                pair(id(a), id(b), |a, b| Constraint::Parallel { a, b })
            }
            GenConstraint::Perpendicular(a, b) => {
                pair(id(a), id(b), |a, b| Constraint::Perpendicular { a, b })
            }
            GenConstraint::Tangent(a, b) => pair(id(a), id(b), |a, b| Constraint::Tangent { a, b }),
            GenConstraint::Equal(a, b) => pair(id(a), id(b), |a, b| Constraint::Equal { a, b }),
            GenConstraint::Distance(a, b, d) => pair(id(a), id(b), |a, b| Constraint::Distance {
                a,
                b,
                dim: Dimension::driving(d),
            }),
            GenConstraint::Radius(c, r) => id(c).map(|curve| Constraint::Radius {
                curve,
                dim: Dimension::driving(r),
            }),
        };
        out.extend(mapped);
    }
    Ok(Candidates {
        total: output.constraints.len(),
        constraints: out,
    })
}

fn pair(
    a: Option<EntityId>,
    b: Option<EntityId>,
    f: impl FnOnce(EntityId, EntityId) -> Constraint,
) -> Option<Constraint> {
    Some(f(a?, b?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::entity::Geom;
    use crate::generated::{GeneratedGeometry, PluginRef};
    use locus_geom::Vec2;

    /// A 10 mm square: lines 0–3, then points 0 and 1 as entities 4 and 5.
    fn square(constraints: Vec<GenConstraint>) -> GeneratedGeometry {
        GeneratedGeometry {
            points: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(10.0, 0.0),
                Vec2::new(10.0, 10.0),
                Vec2::new(0.0, 10.0),
            ],
            entities: vec![
                GenEntity::Line(0, 1),
                GenEntity::Line(1, 2),
                GenEntity::Line(2, 3),
                GenEntity::Line(3, 0),
                GenEntity::Point(0),
                GenEntity::Point(1),
            ],
            constraints,
        }
    }

    fn intent() -> Vec<GenConstraint> {
        vec![
            GenConstraint::Horizontal(0),
            GenConstraint::Vertical(1),
            GenConstraint::Horizontal(2),
            GenConstraint::Vertical(3),
            GenConstraint::Equal(0, 1),
            GenConstraint::Distance(4, 5, 10.0),
        ]
    }

    /// A sketch holding one plugin object; returns it and its recipe.
    fn object(output: GeneratedGeometry) -> (Sketcher, RecipeId, Vec<EntityId>) {
        let mut s = Sketcher::new(Document::new());
        let a = s
            .apply(&Command::AddGenerated {
                layer: Document::default_layer(),
                origin: Vec2::new(5.0, 5.0),
                plugin: PluginRef {
                    id: "com.example.square".into(),
                    version: "1.0.0".into(),
                    params: Vec::new(),
                },
                output,
            })
            .unwrap();
        (s, a.recipes[0], a.entities)
    }

    fn line_angle(s: &Sketcher, line: EntityId) -> f64 {
        let doc = s.document();
        let Some(Geom::Line { start, end }) = doc.entity(line).map(|e| e.geom.clone()) else {
            panic!("not a line");
        };
        let d = doc.point_pos(end).unwrap() - doc.point_pos(start).unwrap();
        d.y.atan2(d.x)
    }

    #[test]
    fn unrotated_keeps_every_constraint_as_one_undo_step() {
        let (mut s, rid, _) = object(square(intent()));
        let constraints_before = s.document().constraints().len();
        let out = s.explode_plugin_object(rid).unwrap();
        assert_eq!((out.kept, out.dropped), (6, 0));
        assert_eq!(s.document().constraints().len(), constraints_before + 6);
        assert!(s.document().recipe(rid).is_none());
        // The square keeps only its position (2), the frame points are free (4).
        assert_eq!(s.state().dof, 6);
        assert_eq!(s.editor().undo_label(), Some("Explode"));
        s.undo().unwrap();
        assert!(s.document().recipe(rid).is_some());
        assert_eq!(s.document().constraints().len(), constraints_before);
    }

    #[test]
    fn rotated_object_keeps_its_rotation() {
        let (mut s, rid, frame_ids) = object(square(intent()));
        // Turn the object 30° by moving its direction point.
        let (o, d) = (frame_ids[0], frame_ids[1]);
        let origin = s.document().point_pos(o).unwrap();
        let t = 30f64.to_radians();
        s.apply(&Command::ModifyEntity {
            id: d,
            geom: Geom::Point {
                pos: origin + Vec2::new(10.0 * t.cos(), 10.0 * t.sin()),
            },
        })
        .unwrap();
        let first_line = s.document().recipe(rid).unwrap().pool[0][4];
        assert!((line_angle(&s, first_line) - t).abs() < 1e-9);

        let out = s.explode_plugin_object(rid).unwrap();
        // The four horizontal/vertical ones are skipped.
        assert_eq!((out.kept, out.dropped), (2, 4));
        assert!((line_angle(&s, first_line) - t).abs() < 1e-6);
    }

    #[test]
    fn conflicting_and_redundant_constraints_are_dropped() {
        let mut c = intent();
        c.push(GenConstraint::Horizontal(0)); // duplicate
        c.push(GenConstraint::Parallel(0, 2)); // implied by H(0), H(2)
        c.push(GenConstraint::Distance(4, 5, 12.0)); // fights Distance 10
        let (mut s, rid, _) = object(square(c));
        let out = s.explode_plugin_object(rid).unwrap();
        assert_eq!((out.kept, out.dropped), (6, 3));
        assert!(s.document().recipe(rid).is_none());
        assert!(s.state().conflicts.is_empty());
        assert!(s.state().redundant.is_empty());
    }

    #[test]
    fn other_recipes_explode_as_before() {
        let (mut s, rid, _) = object(square(Vec::new()));
        let out = s.explode_plugin_object(rid).unwrap();
        assert_eq!((out.kept, out.dropped), (0, 0));
        assert!(s.document().recipe(rid).is_none());
    }
}
