// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Inspector's Style section (design §2.1, §6.2; #287): stroke colour,
//! stroke width, pen, fill and opacity of the selection, or the current
//! style new geometry gets while nothing is selected.
//!
//! Edits to the selection are held until the pointer is released and no
//! field has focus, then applied as one `SetStyle` command, so dragging a
//! value or a colour is one undo step (SPEC §11). A mixed selection shows
//! "—" for a field its objects don't share; editing it sets that field on
//! all of them and leaves the others alone.

use crate::state::AppState;
use egui::{DragValue, Ui};
use locus_doc::{
    Command, Document, EntityId, Fill, Geom, OperationKind, Stroke, Style, StylePatch,
    operation::MAX_PEN,
};

/// One field across the selection: `None` when the objects differ.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Shared {
    pub stroke: Option<Stroke>,
    pub width: Option<f64>,
    pub pen: Option<u32>,
    pub fill: Option<Fill>,
    pub opacity: Option<f64>,
}

impl Shared {
    fn of(styles: &[Style]) -> Self {
        fn same<T: PartialEq + Copy>(v: impl Iterator<Item = T>) -> Option<T> {
            let mut v = v.peekable();
            let first = *v.peek()?;
            v.all(|x| x == first).then_some(first)
        }
        Self {
            stroke: same(styles.iter().map(|s| s.stroke)),
            width: same(styles.iter().map(|s| s.width)),
            pen: same(styles.iter().map(|s| s.pen)),
            fill: same(styles.iter().map(|s| s.fill)),
            opacity: same(styles.iter().map(|s| s.opacity)),
        }
    }

    fn with(mut self, p: &StylePatch) -> Self {
        self.stroke = p.stroke.or(self.stroke);
        self.width = p.width.or(self.width);
        self.pen = p.pen.or(self.pen);
        self.fill = p.fill.or(self.fill);
        self.opacity = p.opacity.or(self.opacity);
        self
    }
}

/// The entities a style edit applies to: the selection without bare points,
/// which are never drawn or exported.
pub(crate) fn styled(doc: &Document, ids: &[EntityId]) -> Vec<EntityId> {
    ids.iter()
        .copied()
        .filter(|id| {
            doc.entity(*id)
                .is_some_and(|e| !matches!(e.geom, Geom::Point { .. }))
        })
        .collect()
}

/// Is any of `ids` on a layer whose export ignores object style (SPEC §7:
/// machine layers export in the profile's colour and hairline)?
fn on_machine_layer(doc: &Document, ids: &[EntityId]) -> bool {
    ids.iter().any(|id| {
        doc.entity(*id)
            .and_then(|e| doc.layer(e.layer))
            .is_some_and(|l| {
                !matches!(
                    l.operation.kind(),
                    OperationKind::Draw | OperationKind::Reference
                )
            })
    })
}

const PENDING: &str = "locus.style.pending";

impl AppState {
    /// The Style section: the selection's style, or the current style.
    pub(crate) fn style_ui(&mut self, ui: &mut Ui) {
        let doc = self.doc.sketcher.document();
        let ids = styled(doc, &self.selected());
        let nothing = self.selection.items.is_empty();
        ui.horizontal(|ui| {
            ui.strong(if nothing { "Current style" } else { "Style" });
            let picking = self.tools.eyedropper;
            if ui
                .selectable_label(picking, "💧")
                .on_hover_text(if nothing {
                    "Eyedropper: click an object to make its style the current style"
                } else {
                    "Eyedropper: click an object to copy its style to the selection"
                })
                .clicked()
            {
                self.tools.eyedropper = !picking;
            }
        });
        if nothing {
            let mut s = self.doc.current_style;
            let shared = Shared::of(&[s]);
            let patch = fields(ui, shared, [0, 0, 0]);
            s = s.patched(&patch);
            if s.validate().is_ok() {
                self.doc.current_style = s;
            }
            ui.weak("New geometry gets this style.");
            return;
        }
        if ids.is_empty() {
            ui.weak("Points have no style.");
            return;
        }
        let styles: Vec<Style> = ids
            .iter()
            .filter_map(|id| doc.entity(*id).map(|e| e.style))
            .collect();
        let layer_colour = ids
            .first()
            .and_then(|id| doc.entity(*id))
            .and_then(|e| doc.layer(e.layer))
            .map_or([0, 0, 0], |l| l.colour);
        let machine = on_machine_layer(doc, &ids);
        let key = egui::Id::new(PENDING);
        // A held edit only belongs to the selection it was made on.
        let mut pending: StylePatch = ui
            .data(|d| d.get_temp::<(Vec<EntityId>, StylePatch)>(key))
            .filter(|(sel, _)| *sel == ids)
            .map(|(_, p)| p)
            .unwrap_or_default();
        let shown = Shared::of(&styles).with(&pending);
        let edit = fields(ui, shown, layer_colour);
        merge(&mut pending, &edit);
        if machine {
            ui.weak("Cut, Score, Engrave and Drag-knife layers export in the machine profile's colour and hairline.");
        }
        let settled = !ui.input(|i| i.pointer.any_down()) && ui.memory(|m| m.focused().is_none());
        if !pending.is_empty() && settled {
            ui.data_mut(|d| d.remove::<(Vec<EntityId>, StylePatch)>(key));
            self.apply_reporting(&Command::SetStyle {
                entities: ids,
                patch: pending,
            });
        } else if pending.is_empty() {
            ui.data_mut(|d| d.remove::<(Vec<EntityId>, StylePatch)>(key));
        } else {
            ui.data_mut(|d| d.insert_temp(key, (ids, pending)));
        }
    }

    /// The eyedropper's click (design §6.2): copy `from`'s style to the
    /// selection as one command, or make it the current style when nothing
    /// is selected. Points have no style and are ignored.
    pub(crate) fn eyedrop(&mut self, from: EntityId) {
        self.tools.eyedropper = false;
        let doc = self.doc.sketcher.document();
        let Some(style) = doc
            .entity(from)
            .filter(|e| !matches!(e.geom, Geom::Point { .. }))
            .map(|e| e.style)
        else {
            return;
        };
        let ids: Vec<EntityId> = styled(doc, &self.selected())
            .into_iter()
            .filter(|id| *id != from)
            .collect();
        if self.selection.items.is_empty() {
            self.doc.current_style = style;
        } else if !ids.is_empty() {
            self.apply_reporting(&Command::SetStyle {
                entities: ids,
                patch: StylePatch::all(style),
            });
        }
    }
}

/// Keep what was edited before and add this frame's edits.
fn merge(into: &mut StylePatch, edit: &StylePatch) {
    *into = StylePatch {
        stroke: edit.stroke.or(into.stroke),
        width: edit.width.or(into.width),
        pen: edit.pen.or(into.pen),
        fill: edit.fill.or(into.fill),
        opacity: edit.opacity.or(into.opacity),
    };
}

/// Draw the fields; returns what the user changed this frame.
fn fields(ui: &mut Ui, s: Shared, layer_colour: [u8; 3]) -> StylePatch {
    let mut out = StylePatch::default();
    let mixed = |ui: &mut Ui, m: bool| {
        if m {
            ui.weak("—").on_hover_text("The selected objects differ");
        }
    };
    egui::Grid::new("style-fields")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Stroke");
            ui.horizontal(|ui| {
                let mut by_layer = s.stroke == Some(Stroke::ByLayer);
                if ui
                    .checkbox(&mut by_layer, "Layer")
                    .on_hover_text("Use the layer's colour")
                    .changed()
                {
                    out.stroke = Some(if by_layer {
                        Stroke::ByLayer
                    } else {
                        Stroke::Colour(layer_colour)
                    });
                }
                let mut c = match s.stroke {
                    Some(Stroke::Colour(c)) => c,
                    _ => layer_colour,
                };
                if ui.color_edit_button_srgb(&mut c).changed() {
                    out.stroke = Some(Stroke::Colour(c));
                }
                mixed(ui, s.stroke.is_none());
            });
            ui.end_row();

            ui.label("Width");
            ui.horizontal(|ui| {
                let mut w = s.width.unwrap_or(locus_doc::style::DEFAULT_WIDTH);
                if ui
                    .add(
                        DragValue::new(&mut w)
                            .range(0.001..=1000.0)
                            .speed(0.01)
                            .suffix(" mm"),
                    )
                    .changed()
                {
                    out.width = Some(w);
                }
                mixed(ui, s.width.is_none());
            });
            ui.end_row();

            ui.label("Pen");
            ui.horizontal(|ui| {
                let mut p = s.pen.unwrap_or(1);
                if ui
                    .add(DragValue::new(&mut p).range(1..=MAX_PEN))
                    .on_hover_text("Plotter pen number")
                    .changed()
                {
                    out.pen = Some(p);
                }
                mixed(ui, s.pen.is_none());
            });
            ui.end_row();

            ui.label("Fill");
            ui.horizontal(|ui| {
                let mut on = matches!(s.fill, Some(Fill::Colour(_)));
                let mut c = match s.fill {
                    Some(Fill::Colour(c)) => c,
                    _ => layer_colour,
                };
                if ui
                    .checkbox(&mut on, "")
                    .on_hover_text("Fill closed shapes (Engrave layers hatch instead)")
                    .changed()
                {
                    out.fill = Some(if on { Fill::Colour(c) } else { Fill::None });
                }
                if on && ui.color_edit_button_srgb(&mut c).changed() {
                    out.fill = Some(Fill::Colour(c));
                }
                mixed(ui, s.fill.is_none());
            });
            ui.end_row();

            ui.label("Opacity");
            ui.horizontal(|ui| {
                let mut o = s.opacity.unwrap_or(1.0) * 100.0;
                if ui
                    .add(DragValue::new(&mut o).range(0.0..=100.0).suffix(" %"))
                    .changed()
                {
                    out.opacity = Some((o / 100.0).clamp(0.0, 1.0));
                }
                mixed(ui, s.opacity.is_none());
            });
            ui.end_row();
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use locus_geom::Vec2;

    fn line(c: &mut AppState, y: f64, style: Style) -> EntityId {
        let layer = c.active_layer();
        let pt = |c: &mut AppState, x: f64| {
            c.apply_reporting(&Command::AddEntity {
                layer,
                construction: false,
                geom: Geom::Point {
                    pos: Vec2::new(x, y),
                },
                style: Style::default(),
            });
            last(c)
        };
        let (a, b) = (pt(c, 0.0), pt(c, 10.0));
        c.apply_reporting(&Command::AddEntity {
            layer,
            construction: false,
            geom: Geom::Line { start: a, end: b },
            style,
        });
        last(c)
    }

    fn last(c: &AppState) -> EntityId {
        c.document()
            .entities()
            .iter()
            .map(|(i, _)| i)
            .last()
            .unwrap_or(EntityId::new(0))
    }

    #[test]
    fn shared_fields_are_none_where_objects_differ() {
        let red = Style {
            stroke: Stroke::Colour([255, 0, 0]),
            ..Style::default()
        };
        let s = Shared::of(&[red, Style::default()]);
        assert_eq!(s.stroke, None);
        assert_eq!(s.width, Some(0.4));
        let s = s.with(&StylePatch {
            stroke: Some(Stroke::ByLayer),
            ..StylePatch::default()
        });
        assert_eq!(s.stroke, Some(Stroke::ByLayer));
    }

    /// New geometry takes the current style (design §6.2, §9 #5).
    #[test]
    fn drawn_geometry_takes_the_current_style() {
        let mut c = AppState::default();
        let st = Style {
            width: 1.2,
            pen: 3,
            ..Style::default()
        };
        c.doc.current_style = st;
        c.set_tool(crate::tools::Tool::Line);
        for x in [0.0, 20.0] {
            c.tool_event(
                crate::tool::ToolEvent::Click(crate::tools::Pick::free(Vec2::new(x, 300.0))),
                None,
                crate::settings::Units::Mm,
            );
        }
        let line = c
            .document()
            .entities()
            .iter()
            .filter(|(_, e)| matches!(e.geom, Geom::Line { .. }))
            .last()
            .map(|(_, e)| e.style);
        assert_eq!(line, Some(st));
    }

    /// The eyedropper copies a style to the selection as one undo step, or
    /// to the current style with nothing selected.
    #[test]
    fn the_eyedropper_copies_a_style() {
        let mut c = AppState::default();
        let red = Style {
            stroke: Stroke::Colour([255, 0, 0]),
            width: 0.9,
            ..Style::default()
        };
        let src = line(&mut c, 0.0, red);
        let a = line(&mut c, 5.0, Style::default());
        let b = line(&mut c, 10.0, Style::default());
        c.selection.items.set_all([a, b]);
        c.tools.eyedropper = true;
        c.eyedrop(src);
        assert!(!c.tools.eyedropper);
        for id in [a, b] {
            assert_eq!(c.document().entity(id).map(|e| e.style), Some(red));
        }
        c.undo();
        for id in [a, b] {
            assert_eq!(
                c.document().entity(id).map(|e| e.style),
                Some(Style::default()),
                "one undo restores both"
            );
        }
        c.selection.items.clear();
        c.eyedrop(src);
        assert_eq!(c.doc.current_style, red);
    }
}
