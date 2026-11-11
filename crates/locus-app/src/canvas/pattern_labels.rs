// SPDX-License-Identifier: AGPL-3.0-or-later
//! On-canvas pattern labels (#168, #169; SPEC §6.2.1 "pattern parameters
//! appear as editable dimensions on canvas"). Each pattern shows its count
//! and spacing (or angle) next to its first copy; a double-click opens the
//! same inline box as an offset's distance, taking `count, spacing` (or
//! `count, angle`) as numbers or expressions over the parameters.

use super::patterns_panel::with_field;
use super::{Canvas, DimEdit, DimTarget, to_display, to_stored};
use crate::render::Palette;
use crate::settings::Units;
use egui::{Align2, FontId, Painter, Rect, Stroke};
use locus_doc::{Command, Document, RecipeId, RecipeKind};
use locus_geom::Vec2;

/// The label text for a pattern: its expressions as typed, in display units.
fn label_text(kind: &RecipeKind, units: Units) -> Option<String> {
    Some(match kind {
        RecipeKind::Linear { dir1, .. } => {
            format!("{} × {}", dir1.count, to_display(&dir1.spacing, units))
        }
        RecipeKind::Circular { count, angle, .. } => format!("{count} × {angle}°"),
        _ => return None,
    })
}

/// Split `a, b` at the one comma outside any brackets (`min(1, 2), 5`).
fn split_top(text: &str) -> Option<(String, String)> {
    let mut depth = 0_i32;
    let mut at = None;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                if at.is_some() {
                    return None;
                }
                at = Some(i);
            }
            _ => {}
        }
    }
    let i = at?;
    Some((text[..i].trim().to_owned(), text[i + 1..].trim().to_owned()))
}

/// Where a pattern's label sits: its first copy's first point, or the
/// source's if no copy exists.
fn anchor(doc: &Document, rid: RecipeId) -> Option<Vec2> {
    let r = doc.recipe(rid)?;
    r.instances()
        .chain(r.sources.iter().copied())
        .find_map(|e| {
            doc.point_pos(e).or_else(|| {
                doc.entity(e)?
                    .geom
                    .point_refs()
                    .first()
                    .and_then(|p| doc.point_pos(*p))
            })
        })
}

impl Canvas {
    /// Draw each pattern's label; remembered (with the offsets') for
    /// double-click editing.
    pub(super) fn draw_pattern_labels(
        &mut self,
        painter: &Painter,
        rect: Rect,
        dark: bool,
        units: Units,
    ) {
        if self.dragging {
            return;
        }
        let pal = Palette::new(dark);
        let doc = self.sketcher.document();
        let mut hits = Vec::new();
        for (rid, r) in doc.recipes() {
            let (Some(text), Some(at)) = (label_text(&r.kind, units), anchor(doc, *rid)) else {
                continue;
            };
            let p = self.view.to_screen(at, rect) + egui::vec2(0.0, -16.0);
            let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), pal.dim);
            let r = Align2::CENTER_CENTER
                .anchor_size(p, galley.size())
                .expand(3.0);
            painter.rect(
                r,
                3.0,
                pal.background,
                Stroke::new(1.0, pal.dim),
                egui::StrokeKind::Outside,
            );
            painter.galley(r.shrink(3.0).min, galley, pal.dim);
            hits.push((*rid, r));
        }
        self.offset_rects.extend(hits);
    }

    /// Open the inline box on a pattern: `count, spacing` (or `count, angle`).
    pub(super) fn edit_pattern(&mut self, rid: RecipeId, units: Units) {
        let doc = self.sketcher.document();
        let Some(r) = doc.recipe(rid) else {
            return;
        };
        let Some(text) =
            label_text(&r.kind, units).map(|t| t.replace(" × ", ", ").replace('°', ""))
        else {
            return;
        };
        self.dim_edit = Some(DimEdit {
            target: DimTarget::Pattern(rid),
            what: if matches!(r.kind, RecipeKind::Linear { .. }) {
                "count, spacing"
            } else {
                "count, angle"
            },
            angle: false,
            text,
            at: anchor(doc, rid).unwrap_or(Vec2::ZERO),
            fresh: true,
        });
    }

    /// The command for an edited pattern label.
    pub(super) fn pattern_command(
        &self,
        rid: RecipeId,
        text: &str,
        units: Units,
    ) -> Result<Command, String> {
        let r = self
            .sketcher
            .document()
            .recipe(rid)
            .ok_or("that pattern is gone")?;
        let (count, second) = split_top(text).ok_or("enter \"count, value\"")?;
        let kind = with_field(&r.kind, 0, count);
        let second = if matches!(r.kind, RecipeKind::Linear { .. }) {
            to_stored(&second, units)
        } else {
            second
        };
        Ok(Command::EditRecipe {
            id: rid,
            kind: with_field(&kind, 1, second),
        })
    }
}
