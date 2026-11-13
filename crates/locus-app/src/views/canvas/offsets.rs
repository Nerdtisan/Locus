// SPDX-License-Identifier: AGPL-3.0-or-later
//! Live offsets on the canvas (#164, SPEC §6.2): the Offset tool turns a
//! clicked curve's chain into an offset recipe, each offset shows its
//! distance as an editable label, and the tool strip edits, explodes or
//! removes the picked one.

use crate::render::Palette;
use crate::settings::Units;
use crate::state::{AppState, DimEdit, DimTarget};
use egui::{Align2, FontId, Painter, Rect, Stroke};
use locus_doc::recipe::RecipeKind;
use locus_doc::{Command, Geom, RecipeId};
use locus_geom::Vec2;

impl AppState {
    /// The offset recipe picked with the Offset tool, for the tool strip.
    pub fn offset_selected(&self) -> Option<RecipeId> {
        self.tools
            .offset_pick
            .filter(|r| self.doc.sketcher.document().recipe(*r).is_some())
    }

    /// Change the picked offset's distance (or corners) from its dialog.
    pub fn set_offset_params(&mut self, rid: RecipeId, distance: f64, sharp: bool) {
        let Some(RecipeKind::Offset { expr, .. }) = self
            .doc
            .sketcher
            .document()
            .recipe(rid)
            .map(|r| r.kind.clone())
        else {
            return;
        };
        self.apply_reporting(&Command::EditRecipe {
            id: rid,
            kind: RecipeKind::Offset {
                distance,
                sharp,
                expr,
            },
        });
    }

    /// Turn the picked offset into plain geometry, or delete it.
    pub fn finish_offset(&mut self, rid: RecipeId, explode: bool) {
        let cmd = if explode {
            Command::ExplodeRecipe { id: rid }
        } else {
            Command::RemoveRecipe { id: rid }
        };
        if self.apply_reporting(&cmd) {
            self.tools.offset_pick = None;
        }
    }

    /// The distance an offset uses now (its expression evaluated).
    pub fn offset_params(&self, rid: RecipeId) -> Option<(f64, bool)> {
        let doc = self.doc.sketcher.document();
        let r = doc.recipe(rid)?;
        let RecipeKind::Offset { sharp, .. } = r.kind else {
            return None;
        };
        Some((locus_doc::offset::distance(doc, &r.kind)?, sharp))
    }

    /// Open the inline editor on an offset's distance: a number or an
    /// expression over the parameters, like any dimension.
    pub(crate) fn edit_offset(&mut self, rid: RecipeId, units: Units) {
        let doc = self.doc.sketcher.document();
        let Some(r) = doc.recipe(rid) else {
            return;
        };
        let RecipeKind::Offset { expr, .. } = &r.kind else {
            return;
        };
        let d = locus_doc::offset::distance(doc, &r.kind).unwrap_or(0.0);
        self.tools.dim_edit = Some(DimEdit {
            target: DimTarget::Offset(rid),
            what: "offset",
            angle: false,
            text: expr.as_ref().map_or_else(
                || crate::state::format_value(d, false, units),
                |s| crate::state::to_display(s, units),
            ),
            at: label_anchor(doc, rid).unwrap_or(Vec2::ZERO),
            fresh: true,
        });
    }

    /// The command for an edited offset distance: a value (mm) or an
    /// expression.
    pub(crate) fn offset_command(
        &self,
        rid: RecipeId,
        value: Option<f64>,
        text: &str,
    ) -> Option<Command> {
        let r = self.doc.sketcher.document().recipe(rid)?;
        let RecipeKind::Offset {
            distance, sharp, ..
        } = r.kind
        else {
            return None;
        };
        let kind = match value {
            Some(v) => RecipeKind::Offset {
                distance: v,
                sharp,
                expr: None,
            },
            None => RecipeKind::Offset {
                distance,
                sharp,
                expr: Some(text.trim().to_owned()),
            },
        };
        Some(Command::EditRecipe { id: rid, kind })
    }

    /// Each offset's distance as a label on its first curve; remembered for
    /// double-click editing.
    pub(crate) fn draw_offset_labels(
        &mut self,
        painter: &Painter,
        rect: Rect,
        dark: bool,
        units: Units,
    ) {
        self.view.offset_rects.clear();
        if self.tools.dragging {
            return;
        }
        let pal = Palette::new(dark);
        let doc = self.doc.sketcher.document();
        for (rid, r) in doc.recipes() {
            if !matches!(r.kind, RecipeKind::Offset { .. }) {
                continue;
            }
            let (Some(at), Some(d)) = (
                label_anchor(doc, *rid),
                locus_doc::offset::distance(doc, &r.kind),
            ) else {
                continue;
            };
            let p = self.view.camera.to_screen(at, rect);
            let text = format!("offset {}", units.format_len(d));
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
            self.view.offset_rects.push((*rid, r));
        }
    }
}

/// Where an offset's label sits: the middle of its first curve.
fn label_anchor(doc: &locus_doc::Document, rid: RecipeId) -> Option<Vec2> {
    let r = doc.recipe(rid)?;
    r.instances().find_map(|i| match doc.entity(i)?.geom {
        Geom::Line { .. } => doc.line(i).map(|l| l.eval(0.5)),
        Geom::Arc { .. } => doc.arc(i).map(|a| a.eval(0.5)),
        Geom::Circle { .. } => doc.circle(i).map(|c| c.center + Vec2::new(c.radius, 0.0)),
        _ => None,
    })
}
