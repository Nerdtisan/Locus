// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Inspector's Style section (design §2.1, §6.2; #287): stroke colour,
//! stroke width, pen and opacity of the selection, or the current style new
//! geometry gets while nothing is selected. Objects have no fill: a selected
//! region's Fill section sets its own fill and opacity (#451).
//!
//! Edits to the selection are held until the pointer is released and no
//! field has focus, then applied as one `SetStyle` command, so dragging a
//! value or a colour is one undo step (SPEC §11). A mixed selection shows
//! "—" for a field its objects don't share; editing it sets that field on
//! all of them and leaves the others alone.

use crate::state::AppState;
use egui::{DragValue, Ui};
use locus_doc::gradient::Gradient;
use locus_doc::region_fill::RegionFill;
use locus_doc::{Align, Cap, Dash, DashAlign, Join, StrokeLayer, StrokeStyles};
use locus_doc::{
    Command, Document, EntityId, Fill, Geom, OperationKind, Stroke, Style, StylePatch,
    operation::MAX_PEN,
};
use locus_region::RegionKey;
use locus_stroke::{DEFAULT_MITER, MAX_MITER, MIN_MITER};

/// One field across the selection: `None` when the objects differ.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Shared {
    pub stroke: Option<Stroke>,
    pub width: Option<f64>,
    pub pen: Option<u32>,
    pub opacity: Option<f64>,
    pub cap: Option<Cap>,
    pub join: Option<Join>,
    pub align: Option<Align>,
    /// `None` when the objects differ; `Some(None)` is solid.
    pub dash: Option<Option<Dash>>,
    /// Extra strokes (#429); `None` when the objects differ.
    pub extra: Option<Vec<StrokeLayer>>,
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
            opacity: same(styles.iter().map(|s| s.opacity)),
            cap: same(styles.iter().map(|s| s.line.cap)),
            join: same(styles.iter().map(|s| s.line.join)),
            align: same(styles.iter().map(|s| s.line.align)),
            dash: same(styles.iter().map(|s| s.line.dash)),
            extra: {
                let mut it = styles.iter().map(|s| &s.line.extra);
                let first = it.next();
                first.filter(|f| it.all(|x| x == *f)).cloned()
            },
        }
    }

    fn with(mut self, p: &StylePatch) -> Self {
        self.stroke = p.stroke.or(self.stroke);
        self.width = p.width.or(self.width);
        self.pen = p.pen.or(self.pen);
        self.opacity = p.opacity.or(self.opacity);
        self.cap = p.cap.or(self.cap);
        self.join = p.join.or(self.join);
        self.align = p.align.or(self.align);
        self.dash = p.dash.or(self.dash);
        if let Some(x) = &p.extra {
            self.extra = Some(x.clone());
        }
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
        let picking = self.tools.eyedropper;
        let mut toggle = false;
        crate::views::properties::section(ui, "Style", |ui| {
            // Painted: no bundled font has a drop or pipette (#385).
            toggle = crate::views::nav_bar::icon_button(ui, "ui.eyedropper", "Eyedropper", picking)
                .on_hover_text(if nothing {
                    "Eyedropper: click an object to make its style the current style"
                } else {
                    "Eyedropper: click an object to copy its style to the selection"
                })
                .clicked();
        });
        if toggle {
            self.tools.eyedropper = !picking;
        }
        if nothing {
            let mut s = self.doc.current_style.clone();
            let shared = Shared::of(&[s.clone()]);
            let patch = fields(ui, shared, [0, 0, 0]);
            s = s.patched(&patch);
            if s.validate().is_ok() {
                self.doc.current_style = s;
            }
            ui.weak("Current style: new geometry is drawn with it. Pick a style with the eyedropper to change it.");
            return;
        }
        if ids.is_empty() {
            ui.weak("Points have no style.");
            return;
        }
        self.edit_styles(ui, ids);
    }

    /// Edit the style of `ids`, held until the pointer is released and no
    /// field has focus, then applied as one `SetStyle` (one undo step).
    fn edit_styles(&mut self, ui: &mut Ui, ids: Vec<EntityId>) {
        let doc = self.doc.sketcher.document();
        let styles: Vec<Style> = ids
            .iter()
            .filter_map(|id| doc.entity(*id).map(|e| e.style.clone()))
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
        if let Some(sentence) = machine_row(doc, &ids) {
            ui.weak(sentence);
        }
        // The layer's Stroke styles, switchable right here (#502).
        let mut switch = None;
        if let Some((layer, cur)) = machine_layer(doc, &ids) {
            ui.horizontal(|ui| {
                ui.label("Stroke styles");
                for (v, label) in [
                    (StrokeStyles::Centreline, "Centreline"),
                    (StrokeStyles::Styled, "Styled"),
                ] {
                    if ui.selectable_label(cur == v, label).clicked() && cur != v {
                        switch = Some((layer, v));
                    }
                }
            })
            .response
            .on_hover_text("For the whole layer: cut the path itself, or the styled strokes (dashes perforate, extra strokes cut in parallel, wide strokes cut their outline)");
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
        if let Some((id, v)) = switch {
            self.apply_reporting(&Command::SetLayerProps {
                id,
                props: locus_doc::LayerProps {
                    stroke_styles: Some(v),
                    ..locus_doc::LayerProps::default()
                },
            });
        }
    }

    /// Selected regions (#370, #451, design §2.1): their own Fill and
    /// Opacity. Each region is filled on its own; holes stay open and
    /// strokes always draw above every fill. An edit is held like a style
    /// edit and applied as one command (one undo step).
    pub(crate) fn region_style_ui(&mut self, ui: &mut Ui, fills_shown: bool) {
        let keys: Vec<RegionKey> = self.selection.regions.selected.iter().cloned().collect();
        crate::views::properties::section(ui, "Fill", |_| {});
        if keys.is_empty() {
            return;
        }
        let doc = self.doc.sketcher.document();
        // What each region shows (#457): stored, or kept from a split.
        let fills: Vec<Option<RegionFill>> = keys.iter().map(|k| self.resolved_fill(k)).collect();
        let stored: Vec<Option<RegionFill>> =
            keys.iter().map(|k| doc.region_fill(k).cloned()).collect();
        let fallback = self.region_layer_colour(&keys[0]);
        let key = egui::Id::new(REGION_PENDING);
        let mut pending: FillPatch = ui
            .data(|d| d.get_temp::<(Vec<RegionKey>, FillPatch)>(key))
            .filter(|(sel, _)| *sel == keys)
            .map(|(_, p)| p)
            .unwrap_or_default();
        let shown: Vec<Option<RegionFill>> = fills
            .iter()
            .map(|f| pending.apply(f.clone(), fallback))
            .collect();
        let mut edit = region_fields(ui, &shown, fallback);
        // The gradient (#436): one edit applies to every selected region,
        // each keeping its own frame.
        if let Some(g) = gradient_fields(ui, &shown, fallback) {
            if g.is_some() {
                edit.on = edit.on.or(Some(true));
            }
            edit.gradient = Some(g);
        }
        pending.merge(edit);
        let settled = !ui.input(|i| i.pointer.any_down()) && ui.memory(|m| m.focused().is_none());
        if pending != FillPatch::default() && settled {
            ui.data_mut(|d| d.remove::<(Vec<RegionKey>, FillPatch)>(key));
            let commands: Vec<Command> = keys
                .iter()
                .zip(fills.iter().zip(&stored))
                .filter_map(|(k, (shown, stored))| {
                    let mut next = pending.apply(shown.clone(), fallback);
                    // Removing an inherited fill stores "no fill", or the
                    // split region's fill would show through again (#457).
                    if next.is_none() && shown.is_some() && stored.is_none() {
                        next = Some(RegionFill {
                            fill: Fill::None,
                            opacity: 1.0,
                            gradient: None,
                        });
                    }
                    (next != *stored).then(|| Command::SetRegionFill {
                        regions: vec![k.clone()],
                        fill: next,
                    })
                })
                .collect();
            if !commands.is_empty() {
                self.apply_reporting(&Command::Compound {
                    label: "Fill region".to_owned(),
                    commands,
                });
            }
        } else if pending == FillPatch::default() {
            ui.data_mut(|d| d.remove::<(Vec<RegionKey>, FillPatch)>(key));
        } else {
            ui.data_mut(|d| d.insert_temp(key, (keys, pending)));
        }
        ui.weak("Each region has its own fill; lines always draw above fills.");
        if !fills_shown {
            ui.weak("Fills are hidden on the canvas: turn on Show fills in the Sketch palette.");
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
            .map(|e| e.style.clone())
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

const REGION_PENDING: &str = "locus.region_fill.pending";

/// A held edit of selected regions' fills (#451): fill on or off, a colour,
/// an opacity. `None` leaves that part as each region has it.
#[derive(Debug, Clone, PartialEq, Default)]
struct FillPatch {
    on: Option<bool>,
    colour: Option<[u8; 3]>,
    opacity: Option<f64>,
    /// A gradient set (`Some(Some)`) or turned back to solid (`Some(None)`),
    /// #436.
    gradient: Option<Option<Box<Gradient>>>,
}

impl FillPatch {
    /// The region's fill with this edit. Turning a fill on keeps its own
    /// colour (#388); one that never had a colour gets `fallback`. A fill
    /// that is off and opaque is no fill at all.
    fn apply(&self, f: Option<RegionFill>, fallback: [u8; 3]) -> Option<RegionFill> {
        let mut r = f.unwrap_or(RegionFill {
            fill: Fill::None,
            opacity: 1.0,
            gradient: None,
        });
        if let Some(g) = &self.gradient {
            r.gradient.clone_from(g);
        }
        if let Some(on) = self.on {
            r.fill = r.fill.switched(on, Some(fallback));
        }
        if let Some(c) = self.colour {
            r.fill = Fill::Colour(c);
        }
        if let Some(o) = self.opacity {
            r.opacity = o.clamp(0.0, 1.0);
        }
        (r.fill != Fill::None || r.opacity < 1.0).then_some(r)
    }

    /// Keep what was edited before and add this frame's edits; a colour
    /// picked after a switch is the colour.
    fn merge(&mut self, edit: FillPatch) {
        if edit.on.is_some() {
            self.on = edit.on;
            self.colour = None;
        }
        self.colour = edit.colour.or(self.colour);
        self.opacity = edit.opacity.or(self.opacity);
        if edit.gradient.is_some() {
            self.gradient = edit.gradient;
        }
    }
}

/// The region Fill fields: on/off, colour and opacity, "—" where the
/// selected regions differ. Returns this frame's edits.
fn region_fields(ui: &mut Ui, shown: &[Option<RegionFill>], fallback: [u8; 3]) -> FillPatch {
    fn same<T: PartialEq + Copy>(v: impl Iterator<Item = T>) -> Option<T> {
        let mut v = v.peekable();
        let first = *v.peek()?;
        v.all(|x| x == first).then_some(first)
    }
    let painted = |f: &Option<RegionFill>| f.as_ref().and_then(RegionFill::painted);
    let on = same(shown.iter().map(|f| painted(f).is_some()));
    let colour = same(
        shown
            .iter()
            .map(|f| f.as_ref().and_then(|f| f.fill.colour())),
    );
    let opacity = same(shown.iter().map(|f| f.as_ref().map_or(1.0, |f| f.opacity)));
    let mut out = FillPatch::default();
    egui::Grid::new("region-fill-fields")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Fill");
            ui.horizontal(|ui| {
                let mut v = on.unwrap_or(false);
                if crate::theme::checkbox(ui, &mut v, "")
                    .on_hover_text("Fill this region (lines always draw above it)")
                    .changed()
                {
                    out.on = Some(v);
                }
                let mut c = colour.flatten().unwrap_or(fallback);
                if v && crate::theme::swatch(ui, &mut c).changed() {
                    out.colour = Some(c);
                }
                if on.is_none() || colour.is_none() {
                    ui.weak("—").on_hover_text("The selected regions differ");
                }
            });
            ui.end_row();
            ui.label("Opacity");
            ui.horizontal(|ui| {
                let mut o = opacity.unwrap_or(1.0) * 100.0;
                if ui
                    .add(DragValue::new(&mut o).range(0.0..=100.0).suffix(" %"))
                    .on_hover_text("The fill's opacity")
                    .changed()
                {
                    out.opacity = Some((o / 100.0).clamp(0.0, 1.0));
                }
                if opacity.is_none() {
                    ui.weak("—").on_hover_text("The selected regions differ");
                }
            });
            ui.end_row();
        });
    out
}

/// The gradient editor under the region Fill fields (#436, M6 design §6):
/// Type, Rotate with shape, Reverse, Angle, Ratio and the stops. The
/// selected regions' first gradient is the one shown; an edit makes the
/// whole gradient, applied to every selected region. `Some(None)` turns the
/// fill back to solid.
fn gradient_fields(
    ui: &mut Ui,
    shown: &[Option<RegionFill>],
    fallback: [u8; 3],
) -> Option<Option<Box<Gradient>>> {
    use locus_doc::gradient::{GradientKind, MAX_STOPS, Stop};
    let first = shown.iter().flatten().next()?;
    first.painted()?;
    let current: Option<Gradient> = first.gradient.as_deref().cloned();
    let base = first.fill.colour().unwrap_or(fallback);
    let mut g = current.clone();
    #[derive(Clone, Copy, PartialEq)]
    enum T {
        Solid,
        Linear,
        Radial,
        Elliptical,
        Conic,
    }
    let kind = match g.as_ref().map(|g| g.kind) {
        None => T::Solid,
        Some(GradientKind::Linear) => T::Linear,
        Some(GradientKind::Radial) => T::Radial,
        Some(GradientKind::Elliptical { .. }) => T::Elliptical,
        Some(GradientKind::Conic) => T::Conic,
    };
    ui.horizontal(|ui| {
        ui.label("Type");
        if let Some(t) = crate::theme::segmented(
            ui,
            kind,
            &[
                (T::Solid, "Solid"),
                (T::Linear, "Linear"),
                (T::Radial, "Radial"),
                (T::Elliptical, "Elliptical"),
                (T::Conic, "Conic"),
            ],
        ) {
            g = match t {
                T::Solid => None,
                t => {
                    let mut next = g.clone().unwrap_or_else(|| Gradient::linear_from(base));
                    next.kind = match t {
                        T::Radial => GradientKind::Radial,
                        T::Elliptical => GradientKind::Elliptical { ratio: 0.6 },
                        T::Conic => GradientKind::Conic,
                        _ => GradientKind::Linear,
                    };
                    // Round kinds start from the middle.
                    if t != T::Linear && kind == T::Linear || kind == T::Solid && t != T::Linear {
                        next.from = locus_geom::Vec2::new(0.5, 0.5);
                        next.to = locus_geom::Vec2::new(1.0, 0.5);
                    }
                    Some(next)
                }
            };
        }
    });
    if let Some(gr) = g.as_mut() {
        ui.horizontal(|ui| {
            crate::theme::checkbox(ui, &mut gr.rotate_with_shape, "Rotate with shape").on_hover_text(
                "Follows Rotate and Mirror. Reshaping by dragging points refits the gradient to the new outline.",
            );
            if ui.button("Reverse").on_hover_text("Swap the gradient's ends").clicked() {
                for s in &mut gr.stops {
                    s.pos = 1.0 - s.pos;
                }
                gr.stops.reverse();
            }
        });
        // The angle turns from → to about their middle.
        let d = gr.to - gr.from;
        let mut deg = d.y.atan2(d.x).to_degrees();
        ui.horizontal(|ui| {
            ui.label("Angle");
            if ui
                .add(DragValue::new(&mut deg).speed(1.0).suffix("°"))
                .on_hover_text("The gradient's direction in its frame")
                .changed()
            {
                let mid = gr.from.lerp(gr.to, 0.5);
                let half = d.length() / 2.0;
                let u = locus_geom::Vec2::from_angle(deg.to_radians());
                if matches!(gr.kind, GradientKind::Linear) {
                    gr.from = mid - u * half;
                    gr.to = mid + u * half;
                } else {
                    gr.to = gr.from + u * d.length();
                }
            }
            if let GradientKind::Elliptical { ratio } = &mut gr.kind {
                ui.label("Ratio");
                let mut pct = *ratio * 100.0;
                if ui
                    .add(DragValue::new(&mut pct).range(1.0..=100.0).suffix(" %"))
                    .changed()
                {
                    *ratio = (pct / 100.0).clamp(0.01, 1.0);
                }
            }
        });
        let mut remove = None;
        for (i, s) in gr.stops.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                crate::theme::swatch(ui, &mut s.colour);
                let mut pos = s.pos * 100.0;
                if ui
                    .add(DragValue::new(&mut pos).range(0.0..=100.0).suffix(" %"))
                    .on_hover_text("Where the stop sits along the gradient")
                    .changed()
                {
                    s.pos = (pos / 100.0).clamp(0.0, 1.0);
                }
                let mut op = s.opacity * 100.0;
                if ui
                    .add(
                        DragValue::new(&mut op)
                            .range(0.0..=100.0)
                            .prefix("α ")
                            .suffix(" %"),
                    )
                    .on_hover_text("The stop's opacity")
                    .changed()
                {
                    s.opacity = (op / 100.0).clamp(0.0, 1.0);
                }
                if ui
                    .small_button("✕")
                    .on_hover_text("Delete this stop")
                    .clicked()
                {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove.filter(|_| gr.stops.len() > 2) {
            gr.stops.remove(i);
        }
        gr.stops.sort_by(|a, b| a.pos.total_cmp(&b.pos));
        if gr.stops.len() < MAX_STOPS
            && ui
                .button("+ Add stop")
                .on_hover_text("A stop halfway along the widest gap")
                .clicked()
        {
            let (i, _) = gr
                .stops
                .windows(2)
                .enumerate()
                .map(|(i, w)| (i, w[1].pos - w[0].pos))
                .fold((0, -1.0), |acc, x| if x.1 > acc.1 { x } else { acc });
            let pos = (gr.stops[i].pos + gr.stops[i + 1].pos) / 2.0;
            let (c, a) = gr.colour(pos);
            gr.stops.insert(
                i + 1,
                Stop {
                    pos,
                    colour: c.map(|v| v.round().clamp(0.0, 255.0) as u8),
                    opacity: a,
                },
            );
        }
    }
    (g != current).then(|| g.map(Box::new))
}

/// Keep what was edited before and add this frame's edits.
fn merge(into: &mut StylePatch, edit: &StylePatch) {
    *into = StylePatch {
        stroke: edit.stroke.or(into.stroke),
        width: edit.width.or(into.width),
        pen: edit.pen.or(into.pen),
        fill: edit.fill.or(into.fill),
        opacity: edit.opacity.or(into.opacity),
        // A colour picked after a switch is the colour: the switch's
        // keep-each-colour rule no longer applies (#388).
        fill_on: if edit.fill.is_some() && edit.fill_on.is_none() {
            None
        } else {
            edit.fill_on.or(into.fill_on)
        },
        align: edit.align.or(into.align),
        cap: edit.cap.or(into.cap),
        join: edit.join.or(into.join),
        dash: edit.dash.or(into.dash),
        extra: edit.extra.clone().or_else(|| into.extra.clone()),
    };
}

/// The selection's one machine layer (Cut, Score, Engrave, Drag knife) and
/// its Stroke styles, or `None` for any other or a mixed selection (#502).
fn machine_layer(doc: &Document, ids: &[EntityId]) -> Option<(locus_doc::LayerId, StrokeStyles)> {
    let mut layers = ids.iter().filter_map(|id| doc.entity(*id).map(|e| e.layer));
    let first = layers.next()?;
    if layers.any(|l| l != first) {
        return None;
    }
    let l = doc.layer(first)?;
    matches!(
        l.operation.kind(),
        OperationKind::Cut
            | OperationKind::Score
            | OperationKind::Engrave
            | OperationKind::DragKnife
    )
    .then_some((first, l.stroke_styles))
}

/// The Machine row (design §3): what the object's layer does with its line
/// style (#428). `None` for a mixed-layer selection.
fn machine_row(doc: &Document, ids: &[EntityId]) -> Option<String> {
    let layer = ids
        .iter()
        .filter_map(|id| doc.entity(*id).map(|e| e.layer))
        .try_fold(None, |acc: Option<locus_doc::LayerId>, l| match acc {
            Some(a) if a != l => Err(()),
            _ => Ok(Some(l)),
        })
        .ok()
        .flatten()?;
    let l = doc.layer(layer)?;
    let kind = l.operation.kind();
    Some(match kind {
        OperationKind::Draw => {
            "Draw layer: SVG, PDF and PNG draw the caps and joins; a plotter draws the centreline."
                .to_owned()
        }
        OperationKind::Reference => return None,
        _ => match l.stroke_styles {
            StrokeStyles::Centreline => format!(
                "{} layer: cuts the centreline (Stroke styles: Centreline, in the layer's Inspector).",
                kind.name()
            ),
            StrokeStyles::Styled => format!(
                "{} layer: cuts the stroke's outline (Stroke styles: Styled).",
                kind.name()
            ),
        },
    })
}

/// The Caps & joins row (design §3, #428): cap, join, miter limit, align.
fn caps_joins(ui: &mut Ui, s: &Shared, out: &mut StylePatch, mixed: &dyn Fn(&mut Ui, bool)) {
    ui.label("Cap");
    ui.horizontal(|ui| {
        for (c, name, tip) in [
            (Cap::Butt, "Butt", "Flat, at the end point"),
            (Cap::Round, "Round", "A half disc, as a round pen draws"),
            (Cap::Square, "Square", "Flat, half the width past the end"),
        ] {
            if ui
                .selectable_label(s.cap == Some(c), name)
                .on_hover_text(tip)
                .clicked()
            {
                out.cap = Some(c);
            }
        }
        mixed(ui, s.cap.is_none());
    });
    ui.end_row();

    ui.label("Join");
    ui.horizontal(|ui| {
        let miter = match s.join {
            Some(Join::Miter { limit }) => Some(limit),
            _ => None,
        };
        if ui
            .selectable_label(miter.is_some(), "Miter")
            .on_hover_text("Sharp corners, bevelled past the miter limit")
            .clicked()
        {
            out.join = Some(Join::Miter {
                limit: miter.unwrap_or(DEFAULT_MITER),
            });
        }
        if ui
            .selectable_label(s.join == Some(Join::Round), "Round")
            .on_hover_text("Rounded corners")
            .clicked()
        {
            out.join = Some(Join::Round);
        }
        if ui
            .selectable_label(s.join == Some(Join::Bevel), "Bevel")
            .on_hover_text("Corners cut straight across")
            .clicked()
        {
            out.join = Some(Join::Bevel);
        }
        if let Some(mut limit) = miter
            && ui
                .add(
                    DragValue::new(&mut limit)
                        .range(MIN_MITER..=MAX_MITER)
                        .speed(0.1),
                )
                .on_hover_text("Miter limit: the longest miter, in stroke widths")
                .changed()
        {
            out.join = Some(Join::Miter { limit });
        }
        mixed(ui, s.join.is_none());
    });
    ui.end_row();

    ui.label("Align");
    ui.horizontal(|ui| {
        for (a, name, tip) in [
            (Align::Centre, "Centre", "The stroke straddles the path"),
            (
                Align::Inside,
                "Inside",
                "Closed paths: the stroke lies inside",
            ),
            (
                Align::Outside,
                "Outside",
                "Closed paths: the stroke lies outside",
            ),
        ] {
            if ui
                .selectable_label(s.align == Some(a), name)
                .on_hover_text(tip)
                .clicked()
            {
                out.align = Some(a);
            }
        }
        mixed(ui, s.align.is_none());
    });
    ui.end_row();
}

/// Dash presets (design §3): name and dash/gap pairs, mm.
const DASH_PRESETS: [(&str, &[f64]); 5] = [
    ("Dash 3/2", &[3.0, 2.0]),
    ("Dot", &[0.0, 1.5]),
    ("Dash-dot", &[3.0, 1.5, 0.0, 1.5]),
    ("Long dash", &[8.0, 3.0]),
    ("Fine 1/1", &[1.0, 1.0]),
];

/// The Dashes row (design §3, #427): Solid or a preset, then the dash/gap
/// pairs, the phase and Align to corners.
fn dashes_row(ui: &mut Ui, s: &Shared, out: &mut StylePatch, mixed: &dyn Fn(&mut Ui, bool)) {
    ui.label("Dashes");
    ui.vertical(|ui| {
        let current = s.dash.flatten();
        ui.horizontal(|ui| {
            let name = match (s.dash, current) {
                (None, _) => "—",
                (Some(None), _) => "Solid",
                (Some(Some(d)), _) => DASH_PRESETS
                    .iter()
                    .find(|(_, p)| *p == d.pattern())
                    .map_or("Custom", |(n, _)| *n),
            };
            egui::ComboBox::from_id_salt("style-dash-preset")
                .selected_text(name)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(s.dash == Some(None), "Solid").clicked() {
                        out.dash = Some(None);
                    }
                    for (n, p) in DASH_PRESETS {
                        if ui.selectable_label(name == n, n).clicked() {
                            let (phase, align) =
                                current.map_or((0.0, DashAlign::Exact), |d| (d.phase, d.align));
                            out.dash = Some(Dash::new(p, phase, align).ok());
                        }
                    }
                });
            mixed(ui, s.dash.is_none());
        });
        let Some(d) = current else {
            return;
        };
        // Dash/gap pairs: edit in place; a blank pair at the end adds one.
        let mut vals: Vec<f64> = d.pattern().to_vec();
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            for (i, v) in vals.iter_mut().enumerate() {
                let tip = if i % 2 == 0 { "Dash, mm" } else { "Gap, mm" };
                changed |= ui
                    .add(
                        DragValue::new(v)
                            .range(0.0..=locus_stroke::MAX_DASH_LEN)
                            .speed(0.05)
                            .suffix(" mm"),
                    )
                    .on_hover_text(tip)
                    .changed();
            }
            if vals.len() < 2 * locus_stroke::MAX_DASH_PAIRS
                && ui.small_button("+").on_hover_text("Add a dash/gap pair").clicked()
            {
                vals.extend([1.0, 1.0]);
                changed = true;
            }
            if vals.len() > 2 && ui.small_button("−").on_hover_text("Remove the last pair").clicked() {
                vals.truncate(vals.len() - 2);
                changed = true;
            }
        });
        let mut phase = d.phase;
        let mut corners = d.align == DashAlign::Corners;
        ui.horizontal(|ui| {
            ui.label("Phase");
            changed |= ui
                .add_enabled(
                    !corners,
                    DragValue::new(&mut phase)
                        .range(-locus_stroke::MAX_DASH_LEN..=locus_stroke::MAX_DASH_LEN)
                        .speed(0.05)
                        .suffix(" mm"),
                )
                .on_hover_text("Shifts the pattern along the path")
                .changed();
            changed |= crate::theme::checkbox(ui, &mut corners, "Align to corners")
                .on_hover_text("Fit the pattern between corners, with a dash centred on every corner and path end")
                .changed();
        });
        if changed {
            let align = if corners {
                DashAlign::Corners
            } else {
                DashAlign::Exact
            };
            // An invalid pattern (all zero) is simply not applied.
            if let Ok(nd) = Dash::new(&vals, phase, align) {
                out.dash = Some(Some(nd));
            }
        }
    });
    ui.end_row();
}

/// The Strokes list (design §3, #429): the object's own stroke, then each
/// extra stroke with its offset, width, colour, pen and dash. Rows paint in
/// list order; Up and Down reorder, Remove drops a row, + adds a copy of the last row offset
/// by its width. At most 15 extra strokes.
fn strokes_row(ui: &mut Ui, s: &Shared, out: &mut StylePatch, mixed: &dyn Fn(&mut Ui, bool)) {
    ui.label("Strokes");
    ui.vertical(|ui| {
        let Some(mut rows) = s.extra.clone() else {
            mixed(ui, true);
            return;
        };
        ui.weak("● The object's own stroke (width and colour above)");
        let mut changed = false;
        let mut action: Option<(usize, i32)> = None; // (row, -1 up / +1 down / 0 remove)
        for (i, r) in rows.iter_mut().enumerate() {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{}.", i + 1));
                changed |= ui
                    .add(
                        DragValue::new(&mut r.offset)
                            .range(-locus_stroke::MAX_WIDTH..=locus_stroke::MAX_WIDTH)
                            .speed(0.05)
                            .prefix("offset ")
                            .suffix(" mm"),
                    )
                    .on_hover_text("Distance from the path: + is left of travel (outside on a clockwise shape)")
                    .changed();
                changed |= ui
                    .add(
                        DragValue::new(&mut r.width)
                            .range(0.001..=locus_stroke::MAX_WIDTH)
                            .speed(0.01)
                            .prefix("w ")
                            .suffix(" mm"),
                    )
                    .on_hover_text("This stroke's width")
                    .changed();
                let mut own = r.colour.is_none();
                if crate::theme::checkbox(ui, &mut own, "Object colour").changed() {
                    r.colour = if own { None } else { Some([0, 0, 0]) };
                    changed = true;
                }
                if let Some(c) = &mut r.colour {
                    changed |= crate::theme::swatch(ui, c).changed();
                }
                let mut pen = r.pen.unwrap_or(0);
                if ui
                    .add(DragValue::new(&mut pen).range(0..=MAX_PEN).prefix("pen "))
                    .on_hover_text("Plotter pen; 0 uses the object's pen")
                    .changed()
                {
                    r.pen = (pen > 0).then_some(pen);
                    changed = true;
                }
                let name = r.dash.map_or("Solid", |d| {
                    DASH_PRESETS
                        .iter()
                        .find(|(_, p)| *p == d.pattern())
                        .map_or("Custom", |(n, _)| *n)
                });
                egui::ComboBox::from_id_salt(("stroke-dash", i))
                    .selected_text(name)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(r.dash.is_none(), "Solid").clicked() {
                            r.dash = None;
                            changed = true;
                        }
                        for (n, p) in DASH_PRESETS {
                            if ui.selectable_label(name == n, n).clicked() {
                                r.dash = Dash::new(p, 0.0, DashAlign::Exact).ok();
                                changed = true;
                            }
                        }
                    });
                if ui.small_button("Up").on_hover_text("Paint earlier").clicked() {
                    action = Some((i, -1));
                }
                if ui.small_button("Down").on_hover_text("Paint later").clicked() {
                    action = Some((i, 1));
                }
                if ui.small_button("Remove").on_hover_text("Remove this stroke").clicked() {
                    action = Some((i, 0));
                }
            });
        }
        if let Some((i, d)) = action {
            match d {
                0 => {
                    rows.remove(i);
                }
                -1 if i > 0 => rows.swap(i, i - 1),
                1 if i + 1 < rows.len() => rows.swap(i, i + 1),
                _ => {}
            }
            changed = true;
        }
        if rows.len() < locus_stroke::MAX_STROKE_LAYERS
            && ui
                .small_button("+ Add stroke")
                .on_hover_text("A copy of the last stroke, offset by its width")
                .clicked()
        {
            let next = rows.last().map_or(
                StrokeLayer {
                    offset: 1.0,
                    ..StrokeLayer::default()
                },
                |l| StrokeLayer {
                    // Next to the last stroke, on the same side, a width further.
                    offset: if l.offset >= 0.0 {
                        l.offset + l.width
                    } else {
                        l.offset - l.width
                    },
                    ..*l
                },
            );
            rows.push(next);
            changed = true;
        }
        if changed && rows.iter().all(|r| r.validate().is_ok()) {
            out.extra = Some(rows);
        }
    });
    ui.end_row();
}

/// Draw the fields; returns what the user changed this frame. Objects
/// have no Fill: regions do (#451).
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
            {
                ui.label("Stroke");
                ui.horizontal(|ui| {
                    let mut by_layer = s.stroke == Some(Stroke::ByLayer);
                    if crate::theme::checkbox(ui, &mut by_layer, "Layer")
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
                    if crate::theme::swatch(ui, &mut c).changed() {
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
            }

            caps_joins(ui, &s, &mut out, &mixed);
            dashes_row(ui, &s, &mut out, &mixed);
            strokes_row(ui, &s, &mut out, &mixed);

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

    /// #451, #388: a region's fill goes off and on keeping its colour; one
    /// that never had a colour gets the fallback; a colour picked after a
    /// switch is the colour; opacity alone makes a fill entry.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn region_fill_edits_keep_each_colour() {
        let red = Some(RegionFill {
            fill: Fill::Colour([200, 0, 0]),
            opacity: 0.5,
            gradient: None,
        });
        let off = FillPatch {
            on: Some(false),
            ..FillPatch::default()
        };
        let on = FillPatch {
            on: Some(true),
            ..FillPatch::default()
        };
        let hidden = off.apply(red.clone(), [9, 9, 9]);
        assert_eq!(hidden.clone().unwrap().fill, Fill::Off([200, 0, 0]));
        assert_eq!(on.apply(hidden, [9, 9, 9]), red);
        assert_eq!(
            on.apply(None, [9, 9, 9]).unwrap().fill,
            Fill::Colour([9, 9, 9])
        );
        let mut p = on;
        p.merge(FillPatch {
            colour: Some([0, 0, 255]),
            ..FillPatch::default()
        });
        assert_eq!(
            p.apply(None, [9, 9, 9]).unwrap().fill,
            Fill::Colour([0, 0, 255])
        );
        let faint = FillPatch {
            opacity: Some(0.3),
            ..FillPatch::default()
        };
        assert_eq!(
            faint.apply(None, [9, 9, 9]),
            Some(RegionFill {
                fill: Fill::None,
                opacity: 0.3,
                gradient: None,
            })
        );
        assert_eq!(off.apply(None, [9, 9, 9]), None);
    }
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
        c.doc.current_style = st.clone();
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
            .map(|(_, e)| e.style.clone());
        assert_eq!(line, Some(st));
    }

    /// #502: an object on a Cut layer offers its layer's Stroke styles in
    /// the Style section; on a Draw layer it doesn't.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn stroke_styles_are_offered_for_machine_layer_objects() {
        let mut c = AppState::default();
        let id = line(&mut c, 0.0, Style::default());
        let doc = c.document();
        assert!(machine_layer(doc, &[id]).is_none(), "Draw layer");
        let layer = doc.entity(id).unwrap().layer;
        c.apply_reporting(&Command::SetLayerProps {
            id: layer,
            props: locus_doc::LayerProps {
                operation: Some(locus_doc::Operation::Cut(Default::default())),
                ..locus_doc::LayerProps::default()
            },
        });
        assert_eq!(
            machine_layer(c.document(), &[id]),
            Some((layer, StrokeStyles::Centreline))
        );
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
        let src = line(&mut c, 0.0, red.clone());
        let a = line(&mut c, 5.0, Style::default());
        let b = line(&mut c, 10.0, Style::default());
        c.selection.items.set_all([a, b]);
        c.tools.eyedropper = true;
        c.eyedrop(src);
        assert!(!c.tools.eyedropper);
        for id in [a, b] {
            assert_eq!(
                c.document().entity(id).map(|e| e.style.clone()),
                Some(red.clone())
            );
        }
        c.undo();
        for id in [a, b] {
            assert_eq!(
                c.document().entity(id).map(|e| e.style.clone()),
                Some(Style::default()),
                "one undo restores both"
            );
        }
        c.selection.items.clear();
        c.eyedrop(src);
        assert_eq!(c.doc.current_style, red);
    }
}
