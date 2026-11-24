// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Machine preview pane (#443, SPEC §7): the document's machine bed at
//! true proportion with the job drawn on it as the machine will make it, and
//! the job's placement. Drag the design (it snaps to the bed's edges, centre
//! and corners) or type X / Y from the machine origin. Each placement is one
//! undoable `SetPlacement`, saved with the document and used by export. The
//! export dialog shows the same bed view ([`placement_ui`]).

use crate::settings::{Settings, Units};
use crate::state::AppState;
use crate::views::canvas::MachinePath;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, vec2};
use locus_doc::{Command, OperationKind};
use locus_export::profile::{Bed, Origin};
use locus_geom::{Aabb, Vec2};

/// Snap distance, screen points.
const SNAP_PT: f64 = 8.0;

/// Transient state of one bed view (never part of the document, §11).
#[derive(Debug, Default)]
pub struct BedView {
    /// A drag in progress: the offset it started from and the pointer's
    /// travel so far, mm.
    drag: Option<(Vec2, Vec2)>,
    /// X / Y typed or dragged but not yet committed (one undo step per edit).
    pending: Option<Vec2>,
}

/// The pane's state: its bed view.
#[derive(Debug, Default)]
pub struct MachinePanel {
    view: BedView,
}

/// The design's bounds on the bed: every machine path, as placed.
pub(crate) fn job_bounds(paths: &[MachinePath]) -> Option<Aabb> {
    paths
        .iter()
        .flat_map(|p| p.points.iter().copied())
        .fold(None, |acc: Option<Aabb>, p| {
            Some(acc.map_or_else(
                || Aabb::from_points(p, p),
                |b| b.union(Aabb::from_points(p, p)),
            ))
        })
}

/// What the machine marks: each path's bounds grown by half its width (the
/// pen on Draw, the kerf on Cut), for the fit check (#450): a stroke past
/// the bed edge doesn't fit even when its centre line does.
pub(crate) fn visual_bounds(paths: &[MachinePath]) -> Option<Aabb> {
    paths
        .iter()
        .filter_map(|p| {
            let b = job_bounds(std::slice::from_ref(p))?;
            let r = Vec2::new(1.0, 1.0) * (p.band.max(0.0) / 2.0);
            Some(Aabb::from_points(b.min - r, b.max + r))
        })
        .reduce(Aabb::union)
}

/// Snap `b` moved by `d` to the bed's edges and centre on each axis, within
/// `tol` mm. Returns the snapped move and the bed lines it snapped to.
pub(crate) fn snap(b: Aabb, d: Vec2, bed: &Bed, tol: f64) -> (Vec2, Option<f64>, Option<f64>) {
    let axis = |lo: f64, hi: f64, size: f64, d: f64| -> (f64, Option<f64>) {
        let mut best: Option<(f64, f64)> = None;
        for edge in [lo + d, (lo + hi) / 2.0 + d, hi + d] {
            for target in [0.0, size / 2.0, size] {
                let gap = target - edge;
                if gap.abs() <= tol && best.is_none_or(|(g, _)| gap.abs() < g.abs()) {
                    best = Some((gap, target));
                }
            }
        }
        best.map_or((d, None), |(gap, t)| (d + gap, Some(t)))
    };
    let (x, gx) = axis(b.min.x, b.max.x, bed.width_mm, d.x);
    let (y, gy) = axis(b.min.y, b.max.y, bed.height_mm, d.y);
    (Vec2::new(x, y), gx, gy)
}

/// The job's X / Y as the machine counts them: from the origin corner to the
/// design's nearest corner, positive into the bed.
pub(crate) fn job_xy(b: Aabb, bed: &Bed) -> Vec2 {
    let (left, bottom) = corner(bed.origin);
    Vec2::new(
        if left {
            b.min.x
        } else {
            bed.width_mm - b.max.x
        },
        if bottom {
            b.min.y
        } else {
            bed.height_mm - b.max.y
        },
    )
}

/// The move that puts the job at machine X / Y `xy`.
pub(crate) fn move_to(b: Aabb, bed: &Bed, xy: Vec2) -> Vec2 {
    let now = job_xy(b, bed);
    let (left, bottom) = corner(bed.origin);
    Vec2::new(
        if left { xy.x - now.x } else { now.x - xy.x },
        if bottom { xy.y - now.y } else { now.y - xy.y },
    )
}

const fn corner(o: Origin) -> (bool, bool) {
    match o {
        Origin::TopLeft => (true, false),
        Origin::TopRight => (false, false),
        Origin::BottomLeft => (true, true),
        Origin::BottomRight => (false, true),
    }
}

/// Does the placed design fit, and if not, why (SPEC §8 bed check).
pub(crate) fn fit_message(b: Aabb, bed: &Bed, units: Units) -> Option<String> {
    let (w, h) = (b.max.x - b.min.x, b.max.y - b.min.y);
    let tol = locus_geom::EXPORT_TOL;
    if w > bed.width_mm + tol || h > bed.height_mm + tol {
        return Some(format!(
            "The design is {} × {}, larger than the {} × {} bed: it doesn't fit.",
            units.format_len(w),
            units.format_len(h),
            units.format_len(bed.width_mm),
            units.format_len(bed.height_mm)
        ));
    }
    let past = [
        (-b.min.x, "left"),
        (b.max.x - bed.width_mm, "right"),
        (-b.min.y, "bottom"),
        (b.max.y - bed.height_mm, "top"),
    ]
    .into_iter()
    .filter(|(d, _)| *d > tol)
    .max_by(|a, b| a.0.total_cmp(&b.0))?;
    Some(format!(
        "The design reaches {} past the bed's {} edge: drag it back or press Centre.",
        units.format_len(past.0),
        past.1
    ))
}

/// A length field in the display units; the value is millimetres.
fn len_drag(ui: &mut Ui, mm: &mut f64, units: Units) -> egui::Response {
    let k = match units {
        Units::Mm => 1.0,
        Units::Inch => 25.4,
    };
    let mut v = *mm / k;
    let r = ui.add(
        egui::DragValue::new(&mut v)
            .speed(0.5 / k)
            .max_decimals(3)
            .suffix(format!(" {}", units.label())),
    );
    if r.changed() {
        *mm = v * k;
    }
    r
}

/// The bed at true proportion with the job on it, draggable. `paths` are
/// the machine paths as placed (bed coordinates), `offset` the placement
/// they were made with. Returns the offset to commit when a drag ends.
fn bed_canvas(
    ui: &mut Ui,
    st: &mut BedView,
    bed: &Bed,
    paths: &[MachinePath],
    offset: Vec2,
    max_h: f32,
) -> Option<Vec2> {
    let dark = ui.visuals().dark_mode;
    let w = ui.available_width().max(120.0);
    let h = (w * (bed.height_mm / bed.width_mm) as f32).clamp(80.0, max_h.max(80.0));
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click_and_drag());
    let margin = 10.0;
    let scale = ((rect.width() - 2.0 * margin) as f64 / bed.width_mm)
        .min((rect.height() - 2.0 * margin) as f64 / bed.height_mm);
    let centre = rect.center();
    let mid = Vec2::new(bed.width_mm / 2.0, bed.height_mm / 2.0);
    let to = |p: Vec2| -> Pos2 {
        let q = (p - mid) * scale;
        Pos2::new(centre.x + q.x as f32, centre.y - q.y as f32)
    };
    let painter = ui.painter_at(rect);
    let bed_rect = Rect::from_two_pos(to(Vec2::ZERO), to(Vec2::new(bed.width_mm, bed.height_mm)));
    let (fill, grid, edge) = if dark {
        (
            Color32::from_gray(38),
            Color32::from_gray(52),
            Color32::from_rgb(120, 200, 255),
        )
    } else {
        (
            Color32::from_gray(250),
            Color32::from_gray(228),
            Color32::from_rgb(0, 110, 190),
        )
    };
    painter.rect_filled(bed_rect, 2.0, fill);
    // A 10 mm grid when it is at least 6 points apart, else 50 mm.
    let step = if 10.0 * scale >= 6.0 { 10.0 } else { 50.0 };
    let mut x = step;
    while x < bed.width_mm {
        painter.vline(
            to(Vec2::new(x, 0.0)).x,
            bed_rect.y_range(),
            Stroke::new(1.0, grid),
        );
        x += step;
    }
    let mut y = step;
    while y < bed.height_mm {
        painter.hline(
            bed_rect.x_range(),
            to(Vec2::new(0.0, y)).y,
            Stroke::new(1.0, grid),
        );
        y += step;
    }
    painter.rect_stroke(
        bed_rect,
        2.0,
        Stroke::new(1.5, edge),
        egui::StrokeKind::Middle,
    );
    painter.rect_filled(
        Rect::from_center_size(to(bed.origin_point()), vec2(7.0, 7.0)),
        0.0,
        edge,
    );

    // The drag: the design follows the pointer, snapping to the bed.
    let bounds = job_bounds(paths);
    let visual = visual_bounds(paths);
    let tol = SNAP_PT / scale;
    if resp.drag_started() && bounds.is_some() {
        st.drag = Some((offset, Vec2::ZERO));
    }
    let mut guides = (None, None);
    let mut shift = Vec2::ZERO;
    if let (Some((start, travel)), Some(b)) = (st.drag.as_mut(), bounds) {
        let d = resp.drag_delta();
        *travel += Vec2::new(f64::from(d.x), -f64::from(d.y)) / scale;
        let want = *start + *travel - offset;
        let (snapped, gx, gy) = snap(b, want, bed, tol);
        shift = snapped;
        guides = (gx, gy);
    }
    let mut commit = None;
    if resp.drag_stopped()
        && let Some(_) = st.drag.take()
    {
        commit = Some(offset + shift);
    }
    if resp.hovered() && bounds.is_some() {
        ui.ctx().set_cursor_icon(if st.drag.is_some() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
    }

    for p in paths.iter().filter(|p| p.shown_at(scale)) {
        let c = crate::views::op_chip::colour(p.kind, dark);
        let pts: Vec<Pos2> = p.points.iter().map(|q| to(*q + shift)).collect();
        if p.band * scale >= 1.0 {
            painter.add(egui::Shape::line(
                pts.clone(),
                Stroke::new((p.band * scale) as f32, c.gamma_multiply(0.3)),
            ));
        }
        // Faint engrave fill (#455): its spacing reads, fills show through.
        let (width, c) = if p.hatch {
            (0.6, c.gamma_multiply(0.45))
        } else {
            (1.25, c)
        };
        painter.add(egui::Shape::line(pts, Stroke::new(width, c)));
    }
    if let Some(b) = visual {
        let moved = Aabb::from_points(b.min + shift, b.max + shift);
        let fits = moved.min.x >= -locus_geom::EXPORT_TOL
            && moved.min.y >= -locus_geom::EXPORT_TOL
            && moved.max.x <= bed.width_mm + locus_geom::EXPORT_TOL
            && moved.max.y <= bed.height_mm + locus_geom::EXPORT_TOL;
        let colour = if fits {
            ui.visuals().weak_text_color()
        } else {
            ui.visuals().error_fg_color
        };
        let r = Rect::from_two_pos(to(moved.min), to(moved.max));
        for (a, b) in [
            (r.left_top(), r.right_top()),
            (r.right_top(), r.right_bottom()),
            (r.right_bottom(), r.left_bottom()),
            (r.left_bottom(), r.left_top()),
        ] {
            painter.add(egui::Shape::dashed_line(
                &[a, b],
                Stroke::new(1.0, colour),
                4.0,
                3.0,
            ));
        }
    }
    let accent = ui.visuals().selection.stroke.color;
    if let Some(x) = guides.0 {
        painter.vline(
            to(Vec2::new(x, 0.0)).x,
            bed_rect.y_range(),
            Stroke::new(1.0, accent),
        );
    }
    if let Some(y) = guides.1 {
        painter.hline(
            bed_rect.x_range(),
            to(Vec2::new(0.0, y)).y,
            Stroke::new(1.0, accent),
        );
    }
    commit
}

/// The bed view, the job's X / Y, Centre / To origin / Unplace and the fit
/// message: shared by the pane and the export dialog. Every change is one
/// `SetPlacement`; returns `true` when the document changed.
#[allow(clippy::too_many_arguments)]
pub fn placement_ui(
    ui: &mut Ui,
    st: &mut BedView,
    app: &mut AppState,
    units: Units,
    bed: &Bed,
    paths: &[MachinePath],
    offset: Vec2,
    max_h: f32,
) -> bool {
    let mut next: Option<Option<Vec2>> = None;
    if let Some(o) = bed_canvas(ui, st, bed, paths, offset, max_h) {
        next = Some(Some(o));
    }
    let bounds = job_bounds(paths);
    match bounds {
        None => {
            ui.weak("Nothing to place: no exported layer has geometry.");
        }
        Some(b) => {
            ui.horizontal(|ui| {
                let now = job_xy(b, bed);
                let mut xy = st.pending.unwrap_or(now);
                ui.label("X");
                let rx = len_drag(ui, &mut xy.x, units);
                ui.label("Y");
                let ry = len_drag(ui, &mut xy.y, units);
                ui.weak(format!(
                    "from the {} origin",
                    bed.origin.name().to_lowercase()
                ))
                .on_hover_text(
                    "Measured from the machine's origin corner to the design's nearest corner",
                );
                if rx.changed() || ry.changed() {
                    st.pending = Some(xy);
                }
                let typed = (rx.changed() && !rx.dragged()) || (ry.changed() && !ry.dragged());
                let released = rx.drag_stopped() || ry.drag_stopped();
                if (typed || released)
                    && let Some(p) = st.pending.take()
                {
                    next = Some(Some(offset + move_to(b, bed, p)));
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .button("Centre")
                    .on_hover_text("Centre the job on the bed")
                    .clicked()
                {
                    let mid = Vec2::new(bed.width_mm / 2.0, bed.height_mm / 2.0);
                    next = Some(Some(offset + mid - (b.min + b.max) * 0.5));
                }
                if ui
                    .button("To origin")
                    .on_hover_text("Put the job's corner on the machine origin")
                    .clicked()
                {
                    next = Some(Some(offset + move_to(b, bed, Vec2::ZERO)));
                }
                if ui
                    .add_enabled(
                        app.document().placement().is_some(),
                        egui::Button::new("Unplace"),
                    )
                    .on_hover_text("Forget the placement: export frames the design itself again")
                    .clicked()
                {
                    next = Some(None);
                }
            });
            if app.document().placement().is_none() {
                ui.weak("Not placed yet: export frames the design itself. Drag it, or press Centre, to place it on the bed.");
            }
            match fit_message(visual_bounds(paths).unwrap_or(b), bed, units) {
                Some(m) => {
                    ui.colored_label(ui.visuals().error_fg_color, m);
                }
                None => {
                    ui.weak(format!(
                        "Fits the bed: {} × {}.",
                        units.format_len(b.max.x - b.min.x),
                        units.format_len(b.max.y - b.min.y)
                    ));
                }
            }
        }
    }
    match next {
        Some(offset) => app.apply_reporting(&Command::SetPlacement { offset }),
        None => false,
    }
}

impl MachinePanel {
    /// The pane. Returns `true` when "Manage profiles…" was picked.
    pub fn ui(&mut self, ui: &mut Ui, app: &mut AppState, settings: &Settings) -> bool {
        let profile = crate::views::machine_profile::current(app.document(), settings);
        let mut manage = false;
        ui.horizontal(|ui| {
            ui.label("Machine");
            let text = profile
                .as_ref()
                .map_or_else(|| "None".to_owned(), |p| p.name.clone());
            let tip = profile.as_ref().map_or_else(
                || "Choose the machine this document is for".to_owned(),
                crate::views::machine_profile::summary,
            );
            manage = crate::views::machine_profile::picker_button(
                ui,
                app,
                settings,
                egui::RichText::new(text),
                &tip,
            );
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            match profile.as_ref().and_then(|p| p.bed.map(|b| (p, b))) {
                None if profile.is_none() => {
                    ui.weak("No machine profile set: choose the machine above to see its bed and place the job on it.");
                }
                None => {
                    ui.weak("This profile has no bed size, so there is nothing to place the job on.");
                    if ui.button("Set a bed in Machine profiles…").clicked() {
                        manage = true;
                    }
                }
                Some((_, bed)) => {
                    ui.weak(format!(
                        "Bed {} × {}, origin {}",
                        settings.units.format_len(bed.width_mm),
                        settings.units.format_len(bed.height_mm),
                        bed.origin.name().to_lowercase()
                    ));
                    let paths = app.view.machine.clone();
                    let offset = app.view.machine_offset;
                    placement_ui(
                        ui,
                        &mut self.view,
                        app,
                        settings.units,
                        &bed,
                        &paths,
                        offset,
                        ui.available_height().max(160.0) - 90.0,
                    );
                }
            }
            ui.separator();
            legend_ui(ui, app);
        });
        manage
    }
}

/// Each layer's operation in its colour (#420) and why the preview may look
/// like the sketch (#423).
fn legend_ui(ui: &mut Ui, app: &mut AppState) {
    let dark = ui.visuals().dark_mode;
    let mut switch: Option<(locus_doc::LayerId, locus_doc::StrokeStyles)> = None;
    for row in crate::views::canvas::legend(app.document()) {
        ui.horizontal(|ui| {
            let c = crate::views::op_chip::colour(row.kind, dark);
            let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            if row.idle {
                ui.painter()
                    .circle_stroke(dot.center(), 4.0, Stroke::new(1.2, c));
            } else {
                ui.painter().circle_filled(dot.center(), 4.5, c);
            }
            let short = egui::RichText::new(crate::views::op_chip::short(row.kind))
                .small()
                .color(if row.idle { c.gamma_multiply(0.5) } else { c });
            ui.add_sized(vec2(58.0, 14.0), egui::Label::new(short));
            let name = if row.idle && row.kind != OperationKind::Reference {
                format!("{} (not exported)", row.layer)
            } else if row.hidden {
                format!("{} (hidden)", row.layer)
            } else {
                row.layer.clone()
            };
            if row.idle || row.hidden {
                ui.weak(name);
            } else {
                ui.label(name);
            }
            // Stroke styles, one click away (#502).
            if let Some(cur) = row.stroke_styles {
                use locus_doc::StrokeStyles;
                for (v, label, tip) in [
                    (
                        StrokeStyles::Centreline,
                        "Centreline",
                        "Cut or mark the path itself; line styles are visual only",
                    ),
                    (
                        StrokeStyles::Styled,
                        "Styled",
                        "Cut or mark the strokes: dashes perforate, extra strokes cut in parallel, wide strokes cut their outline",
                    ),
                ] {
                    if ui
                        .selectable_label(cur == v, egui::RichText::new(label).small())
                        .on_hover_text(tip)
                        .clicked()
                        && cur != v
                    {
                        switch = Some((row.id, v));
                    }
                }
            }
        });
    }
    if let Some((id, v)) = switch {
        app.apply_reporting(&locus_doc::Command::SetLayerProps {
            id,
            props: locus_doc::LayerProps {
                stroke_styles: Some(v),
                ..locus_doc::LayerProps::default()
            },
        });
    }
    for n in &app.view.machine_notes {
        ui.weak(n);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn bed(origin: Origin) -> Bed {
        Bed {
            width_mm: 400.0,
            height_mm: 300.0,
            origin,
        }
    }

    fn b(x0: f64, y0: f64, x1: f64, y1: f64) -> Aabb {
        Aabb::from_points(Vec2::new(x0, y0), Vec2::new(x1, y1))
    }

    /// A drag near an edge or the centre lands on it; far away it doesn't.
    #[test]
    fn drags_snap_to_edges_and_centre() {
        let job = b(10.0, 10.0, 50.0, 30.0);
        let bed = bed(Origin::TopLeft);
        let (d, gx, gy) = snap(job, Vec2::new(-8.0, 100.0), &bed, 3.0);
        assert_eq!(d.x, -10.0, "left edge");
        assert_eq!(gx, Some(0.0));
        assert_eq!(gy, None);
        assert_eq!(d.y, 100.0);
        // Centre on the bed's centre (job centre 30 → 200).
        let (d, gx, _) = snap(job, Vec2::new(169.0, 0.0), &bed, 3.0);
        assert_eq!(d.x, 170.0);
        assert_eq!(gx, Some(200.0));
    }

    /// X / Y count from the origin corner, inward, and round-trip.
    #[test]
    fn xy_counts_from_the_machine_origin() {
        let job = b(10.0, 20.0, 50.0, 60.0);
        assert_eq!(job_xy(job, &bed(Origin::BottomLeft)), Vec2::new(10.0, 20.0));
        assert_eq!(job_xy(job, &bed(Origin::TopLeft)), Vec2::new(10.0, 240.0));
        assert_eq!(job_xy(job, &bed(Origin::TopRight)), Vec2::new(350.0, 240.0));
        for o in Origin::ALL {
            let bed = bed(o);
            let d = move_to(job, &bed, Vec2::new(5.0, 7.0));
            let moved = Aabb::from_points(job.min + d, job.max + d);
            assert_eq!(job_xy(moved, &bed), Vec2::new(5.0, 7.0), "{o:?}");
        }
    }

    #[test]
    fn fit_warnings_say_what_is_wrong() {
        let bed = bed(Origin::TopLeft);
        assert!(fit_message(b(0.0, 0.0, 400.0, 300.0), &bed, Units::Mm).is_none());
        let big = fit_message(b(0.0, 0.0, 450.0, 100.0), &bed, Units::Mm).unwrap();
        assert!(big.contains("doesn't fit"), "{big}");
        let past = fit_message(b(390.0, 0.0, 412.5, 10.0), &bed, Units::Mm).unwrap();
        // A 4 mm pen on a line along the bed's left edge: the centre line
        // fits, the stroke doesn't (#450).
        let line = MachinePath {
            points: vec![Vec2::new(0.0, 10.0), Vec2::new(0.0, 50.0)],
            kind: OperationKind::Draw,
            hatch: false,
            band: 4.0,
            row: None,
            styled: Default::default(),
        };
        let geom = job_bounds(std::slice::from_ref(&line)).unwrap();
        assert!(fit_message(geom, &bed, Units::Mm).is_none());
        let seen = visual_bounds(std::slice::from_ref(&line)).unwrap();
        let m = fit_message(seen, &bed, Units::Mm).unwrap();
        assert!(m.contains("2.000 mm") && m.contains("left"), "{m}");
        assert!(
            past.contains("12.500 mm") && past.contains("right"),
            "{past}"
        );
    }

    /// The pane's placement is one undoable command, and the preview follows.
    #[test]
    fn placing_the_job_is_one_undo_step_and_moves_the_preview() {
        let settings = Settings::default();
        let mut app = AppState::default(); // the demo sketch
        assert!(app.choose_profile(Some("lightburn".into()), &settings));
        app.view.profile_offer = None;
        app.set_machine_preview(true);
        app.sync_machine_profile(&settings);
        let before = job_bounds(&app.view.machine).unwrap();
        let bed = locus_export::profile::find("lightburn", &[])
            .unwrap()
            .bed
            .unwrap();
        let mid = Vec2::new(bed.width_mm / 2.0, bed.height_mm / 2.0);
        let d = mid - (before.min + before.max) * 0.5;
        assert!(app.apply_reporting(&Command::SetPlacement { offset: Some(d) }));
        assert_eq!(app.view.machine_offset, d);
        let after = job_bounds(&app.view.machine).unwrap();
        assert!(((after.min + after.max) * 0.5 - mid).length() < 1e-6);
        assert_eq!(app.undo().as_deref(), Some("Place job on bed"));
        assert_eq!(app.document().placement(), None);
    }
}
