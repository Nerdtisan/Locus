// SPDX-License-Identifier: AGPL-3.0-or-later
//! Auto trace (#461, M6 design §10, ADR 0019 decision 8): a raster
//! reference into ordinary, editable geometry with Locus's own tracer
//! (`locus-trace`).
//!
//! Insert ▸ Auto trace… (or the reference's Inspector) opens a dialog with
//! the mode, threshold, colours, speckles, corners and tolerance. Every
//! change starts a new trace in the background (a worker thread on the
//! desktop, cancelling the one still running; inline on the web, where
//! there are no threads), and the result is drawn over the reference as a
//! preview with its piece count. **Trace** commits it as one undo step:
//! lines and Béziers in mm at the reference's placement (its calibrated
//! scale, turn and pixel size), on the chosen layer, or one new layer per
//! colour in Colours mode. Tracing never changes the reference.

use crate::render::Frame;
use crate::state::AppState;
use egui::{Color32, Stroke};
use locus_doc::reference::{ImportShape, ShapeGroup, ShapeTarget, shapes_command};
use locus_doc::{LayerId, Operation, RefImage};
use locus_geom::Vec2;
use locus_trace::{Budget, CurveParams, Limits, Mode, Params, Seg, TraceError, Traced};

/// The mode picker's choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Threshold,
    Edges,
    Colours,
    Centreline,
}

/// The dialog's settings, kept between openings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Settings {
    pub kind: Kind,
    /// `None`: Otsu's level.
    pub level: Option<u8>,
    pub colours: u8,
    pub speckle: u32,
    /// Corners, as Potrace's `alpha_max` (0 = all corners).
    pub corners: f64,
    /// Centreline simplification, px.
    pub tolerance: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            kind: Kind::Threshold,
            level: None,
            colours: 4,
            speckle: 4,
            corners: 1.0,
            tolerance: 0.5,
        }
    }
}

impl Settings {
    pub(crate) fn params(&self) -> Params {
        Params {
            mode: match self.kind {
                Kind::Threshold => Mode::Threshold { level: self.level },
                Kind::Edges => Mode::Edges { level: self.level },
                Kind::Colours => Mode::Colours {
                    count: self.colours.clamp(2, locus_trace::MAX_COLOURS as u8),
                },
                Kind::Centreline => Mode::Centreline { level: self.level },
            },
            speckle: self.speckle,
            curve: CurveParams {
                alpha_max: self.corners.clamp(0.0, 4.0 / 3.0),
            },
            tolerance: self.tolerance.clamp(0.1, 10.0),
        }
    }
}

/// Where Trace puts the result (Colours mode always makes new layers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Output {
    NewLayer,
    Layer(LayerId),
}

/// The trace for one source and settings: running, done or failed.
enum Job {
    #[cfg(not(target_arch = "wasm32"))]
    Running {
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        rx: std::sync::mpsc::Receiver<Result<Traced, String>>,
    },
    Done(Traced),
    Failed(String),
}

/// The open dialog's state (never part of the document, SPEC §11).
#[derive(Default)]
pub struct TraceUi {
    /// The reference being traced; `None` while the dialog is closed.
    pub(crate) source: Option<LayerId>,
    pub(crate) settings: Settings,
    pub(crate) output: Option<Output>,
    /// What the current job was started for.
    started: Option<(LayerId, Settings)>,
    job: Option<Job>,
}

impl std::fmt::Debug for TraceUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceUi")
            .field("source", &self.source)
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl TraceUi {
    fn cancel(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(Job::Running { cancel, .. }) = &self.job {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.job = None;
        self.started = None;
    }

    /// The finished trace, if it matches the dialog.
    fn done(&self) -> Option<&Traced> {
        match &self.job {
            Some(Job::Done(t)) => Some(t),
            _ => None,
        }
    }
}

/// Trace `bytes` (a PNG or JPEG) with `params`, within the S5 budget.
fn run(bytes: &[u8], params: &Params, cancel: &mut dyn FnMut() -> bool) -> Result<Traced, String> {
    let img = locus_import::raster::decode(bytes).map_err(|e| e.to_string())?;
    let mut budget = Budget::new(locus_trace::DEFAULT_STEPS).with_cancel(cancel);
    locus_trace::trace(
        img.width,
        img.height,
        &img.pixels,
        params,
        &Limits::default(),
        &mut budget,
    )
    .map_err(|e| match e {
        TraceError::Cancelled => "Cancelled".to_owned(),
        e => e.to_string(),
    })
}

/// A traced piece on the page, mm.
fn to_shape(img: &RefImage, s: &Seg) -> ImportShape {
    let w = |p: Vec2| img.pixel_to_world(p);
    match *s {
        Seg::Line(a, b) => ImportShape::Line { a: w(a), b: w(b) },
        Seg::Cubic(a, b, c, d) => ImportShape::Cubic {
            a: w(a),
            c1: w(b),
            c2: w(c),
            b: w(d),
        },
    }
}

/// The groups Trace commits: one per colour in Colours mode, else one on
/// `output`.
pub(crate) fn groups(
    img: &RefImage,
    traced: &Traced,
    kind: Kind,
    output: Output,
) -> Vec<ShapeGroup> {
    traced
        .groups
        .iter()
        .map(|g| {
            let shapes: Vec<ImportShape> = g
                .paths
                .iter()
                .flat_map(|p| &p.segments)
                .map(|s| to_shape(img, s))
                .collect();
            let target = match (kind, output) {
                (Kind::Colours, _) => ShapeTarget::NewLayer {
                    name: format!(
                        "Trace #{:02x}{:02x}{:02x}",
                        g.colour[0], g.colour[1], g.colour[2]
                    ),
                    colour: g.colour,
                    operation: Operation::Draw,
                },
                (_, Output::Layer(l)) => ShapeTarget::Layer(l),
                (_, Output::NewLayer) => ShapeTarget::NewLayer {
                    name: "Trace".to_owned(),
                    colour: [0, 0, 0],
                    operation: Operation::Draw,
                },
            };
            ShapeGroup { target, shapes }
        })
        .collect()
}

impl AppState {
    /// Open the dialog on `layer`'s image, or the first visible image.
    pub(crate) fn open_trace(&mut self, layer: Option<LayerId>) {
        let doc = self.doc.sketcher.document();
        let source = layer
            .filter(|l| doc.image(*l).is_some())
            .or_else(|| self.view.browser_layer.filter(|l| doc.image(*l).is_some()))
            .or_else(|| doc.images().keys().next().copied());
        match source {
            Some(l) => {
                self.tools.trace.source = Some(l);
                self.tools.trace.cancel();
            }
            None => {
                self.doc.notice =
                    Some("Insert a PNG or JPEG reference first, then trace it".to_owned());
            }
        }
    }

    /// Start a trace when the source or settings changed; collect a
    /// finished one.
    fn trace_tick(&mut self, ctx: &egui::Context) {
        let Some(source) = self.tools.trace.source else {
            return;
        };
        let want = (source, self.tools.trace.settings);
        if self.tools.trace.started != Some(want) {
            self.tools.trace.cancel();
            let Some(img) = self.doc.sketcher.document().image(source).cloned() else {
                self.tools.trace.source = None;
                return;
            };
            let params = want.1.params();
            self.tools.trace.started = Some(want);
            #[cfg(not(target_arch = "wasm32"))]
            {
                use std::sync::atomic::{AtomicBool, Ordering};
                let cancel = std::sync::Arc::new(AtomicBool::new(false));
                let (tx, rx) = std::sync::mpsc::channel();
                let flag = cancel.clone();
                let ctx = ctx.clone();
                let spawned = std::thread::Builder::new()
                    .name("locus-trace".to_owned())
                    .spawn(move || {
                        let mut stop = || flag.load(Ordering::Relaxed);
                        let result = run(&img.data, &params, &mut stop);
                        let _ = tx.send(result);
                        ctx.request_repaint();
                    });
                self.tools.trace.job = Some(match spawned {
                    Ok(_) => Job::Running { cancel, rx },
                    Err(e) => Job::Failed(format!("couldn't start the trace ({})", e.kind())),
                });
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = ctx;
                let mut never = || false;
                self.tools.trace.job = Some(match run(&img.data, &params, &mut never) {
                    Ok(t) => Job::Done(t),
                    Err(e) => Job::Failed(e),
                });
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(Job::Running { rx, .. }) = &self.tools.trace.job
            && let Ok(result) = rx.try_recv()
        {
            self.tools.trace.job = Some(match result {
                Ok(t) => Job::Done(t),
                Err(e) => Job::Failed(e),
            });
        }
    }

    /// The Auto trace dialog (M6 design §10).
    pub(crate) fn trace_dialog(&mut self, ctx: &egui::Context) {
        if self.tools.trace.source.is_none() {
            return;
        }
        self.trace_tick(ctx);
        let doc = self.doc.sketcher.document();
        let images: Vec<(LayerId, String)> = doc
            .images()
            .keys()
            .filter_map(|l| doc.layer(*l).map(|layer| (*l, layer.name.clone())))
            .collect();
        let drawing: Vec<(LayerId, String)> = doc
            .layer_order()
            .into_iter()
            .filter_map(|l| doc.layer(l).map(|layer| (l, layer)))
            .filter(|(_, layer)| !layer.locked && !matches!(layer.operation, Operation::Reference))
            .map(|(l, layer)| (l, layer.name.clone()))
            .collect();
        let pixel = self
            .tools
            .trace
            .source
            .and_then(|l| doc.image(l))
            .map(|img| img.placement.pixel());
        let t = &mut self.tools.trace;
        let mut open = true;
        let (mut commit, mut close) = (false, false);
        egui::Window::new("Auto trace")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(320.0)
            .show(ctx, |ui| {
                use crate::views::canvas::dialog::row;
                row(ui, "Source", |ui| {
                    let name = images
                        .iter()
                        .find(|(l, _)| Some(*l) == t.source)
                        .map_or("—", |(_, n)| n.as_str());
                    egui::ComboBox::from_id_salt("trace-source")
                        .selected_text(name)
                        .show_ui(ui, |ui| {
                            for (l, n) in &images {
                                ui.selectable_value(&mut t.source, Some(*l), n);
                            }
                        });
                });
                if let Some(px) = pixel {
                    ui.weak(format!(
                        "Calibrated: {:.3} × {:.3} mm/px (#391)",
                        px.x, px.y
                    ));
                }
                row(ui, "Mode", |ui| {
                    if let Some(k) = crate::theme::segmented(
                        ui,
                        t.settings.kind,
                        &[
                            (Kind::Threshold, "Threshold"),
                            (Kind::Edges, "Edges"),
                            (Kind::Colours, "Colours"),
                            (Kind::Centreline, "Centreline"),
                        ],
                    ) {
                        t.settings.kind = k;
                    }
                });
                let s = &mut t.settings;
                if s.kind == Kind::Colours {
                    row(ui, "Colours", |ui| {
                        ui.add(egui::Slider::new(
                            &mut s.colours,
                            2..=locus_trace::MAX_COLOURS as u8,
                        ))
                        .on_hover_text("Each colour goes on its own new layer")
                    });
                } else {
                    row(ui, "Threshold", |ui| {
                        let mut auto = s.level.is_none();
                        crate::theme::checkbox(ui, &mut auto, "Auto (Otsu)");
                        let mut level = s.level.unwrap_or(128);
                        let r = ui.add_enabled(!auto, egui::Slider::new(&mut level, 0..=255));
                        s.level = if auto { None } else { Some(level) };
                        r
                    });
                }
                row(ui, "Speckles <", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut s.speckle)
                            .range(0..=10_000)
                            .suffix(" px"),
                    )
                    .on_hover_text("Shapes this small are dropped")
                });
                if s.kind == Kind::Centreline {
                    row(ui, "Tolerance", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut s.tolerance)
                                .range(0.1..=10.0)
                                .speed(0.05)
                                .suffix(" px"),
                        )
                        .on_hover_text("How far a line may stray from the centre")
                    });
                } else {
                    row(ui, "Corners", |ui| {
                        ui.add(egui::Slider::new(&mut s.corners, 0.0..=1.33).fixed_decimals(2))
                            .on_hover_text("Lower makes more corners; 0 gives straight lines only")
                    });
                }
                if s.kind != Kind::Colours {
                    row(ui, "Output layer", |ui| {
                        let current = t.output.unwrap_or(Output::NewLayer);
                        let name = match current {
                            Output::NewLayer => "New layer \"Trace\"".to_owned(),
                            Output::Layer(l) => {
                                drawing.iter().find(|(id, _)| *id == l).map_or_else(
                                    || "New layer \"Trace\"".to_owned(),
                                    |(_, n)| n.clone(),
                                )
                            }
                        };
                        egui::ComboBox::from_id_salt("trace-output")
                            .selected_text(name)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut t.output,
                                    Some(Output::NewLayer),
                                    "New layer \"Trace\"",
                                );
                                for (l, n) in &drawing {
                                    ui.selectable_value(&mut t.output, Some(Output::Layer(*l)), n);
                                }
                            });
                    });
                }
                ui.separator();
                let status = match &t.job {
                    None => "Starting…".to_owned(),
                    #[cfg(not(target_arch = "wasm32"))]
                    Some(Job::Running { .. }) => "Tracing…".to_owned(),
                    Some(Job::Failed(e)) => e.clone(),
                    Some(Job::Done(tr)) => {
                        let layers = tr.groups.len();
                        format!(
                            "{} curves · {} layer{}",
                            group_digits(tr.segments()),
                            layers,
                            if layers == 1 { "" } else { "s" }
                        )
                    }
                };
                ui.label(status);
                ui.horizontal(|ui| {
                    let ready = t.done().is_some_and(|tr| tr.segments() > 0);
                    if ui
                        .add_enabled(ready, egui::Button::new("Trace"))
                        .on_hover_text("Add the preview as geometry (one undo step)")
                        .clicked()
                    {
                        commit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        if commit {
            self.trace_commit();
        } else if close || !open {
            self.tools.trace.cancel();
            self.tools.trace.source = None;
        }
    }

    /// Trace: the preview as geometry, one undo step.
    fn trace_commit(&mut self) {
        let t = &self.tools.trace;
        let (Some(source), Some(traced)) = (t.source, t.done()) else {
            return;
        };
        let doc = self.doc.sketcher.document();
        let Some(img) = doc.image(source) else {
            return;
        };
        let out = t.output.unwrap_or(Output::NewLayer);
        let gs = groups(img, traced, t.settings.kind, out);
        match shapes_command(doc, "Auto trace", &gs) {
            Ok(cmd) => {
                if self.apply_reporting(&cmd) {
                    self.tools.trace.cancel();
                    self.tools.trace.source = None;
                }
            }
            Err(e) => self.doc.notice = Some(e.to_string()),
        }
    }

    /// The finished trace drawn over its reference (the dialog's preview).
    pub(crate) fn draw_trace_preview(&self, frame: &Frame<'_>) {
        let t = &self.tools.trace;
        let (Some(source), Some(traced)) = (t.source, t.done()) else {
            return;
        };
        let Some(img) = self.doc.sketcher.document().image(source) else {
            return;
        };
        let s = |p: Vec2| frame.view.to_screen(img.pixel_to_world(p), frame.rect);
        for g in &traced.groups {
            let c = if t.settings.kind == Kind::Colours {
                Color32::from_rgb(g.colour[0], g.colour[1], g.colour[2])
            } else {
                frame.pal.selected
            };
            let stroke = Stroke::new(1.2, c);
            for seg in g.paths.iter().flat_map(|p| &p.segments) {
                let pts: Vec<egui::Pos2> = match *seg {
                    Seg::Line(a, b) => vec![s(a), s(b)],
                    Seg::Cubic(..) => (0..=12).map(|k| s(seg.at(f64::from(k) / 12.0))).collect(),
                };
                frame.painter.add(egui::Shape::line(pts, stroke));
            }
        }
    }
}

/// `1284` as "1 284".
fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A 40 × 40 PNG with a black square in the middle.
    fn square_png() -> Vec<u8> {
        let mut img = image::RgbaImage::new(40, 40);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let ink = (10..30).contains(&x) && (10..30).contains(&y);
            *p = if ink {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            };
        }
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_traced_square_lands_at_the_references_placement_in_one_step() {
        let mut c = AppState::default();
        c.load(locus_doc::Document::new());
        let (cmd, _) = locus_import::reference_command_at(
            c.document(),
            "photo.png",
            &square_png(),
            Vec2::new(100.0, 50.0),
        )
        .unwrap();
        c.apply(&cmd).unwrap();
        let layer = *c.document().images().keys().next().unwrap();
        let img = c.document().image(layer).cloned().unwrap();
        let mut never = || false;
        let traced = run(&img.data, &Settings::default().params(), &mut never).unwrap();
        assert_eq!(traced.segments(), 4);
        let gs = groups(&img, &traced, Kind::Threshold, Output::NewLayer);
        let cmd = shapes_command(c.document(), "Auto trace", &gs).unwrap();
        let before = c.document().entities().len();
        c.apply(&cmd).unwrap();
        assert_eq!(c.undo_label(), Some("Auto trace"));
        let doc = c.document();
        let new: Vec<Vec2> = doc
            .entities()
            .iter()
            .filter_map(|(id, _)| doc.point_pos(id))
            .collect();
        // The square's corners: pixels 10 and 30 of a 40 px image at
        // 96 dpi, centred on (100, 50).
        let px = locus_doc::image::DEFAULT_MM_PER_PX;
        let corner = img.pixel_to_world(Vec2::new(10.0, 10.0));
        assert!(new.iter().any(|p| p.distance(corner) < 1e-9));
        assert!((img.pixel_to_world(Vec2::new(30.0, 10.0)).x - corner.x - 20.0 * px).abs() < 1e-9);
        assert_eq!(doc.entities().len(), before + 8, "4 points + 4 lines");
        c.undo();
        assert_eq!(c.document().entities().len(), before);
    }

    #[test]
    fn colours_make_one_new_layer_each() {
        let traced = Traced {
            groups: vec![
                locus_trace::Group {
                    colour: [200, 0, 0],
                    paths: vec![locus_trace::TracedPath {
                        segments: vec![Seg::Line(Vec2::ZERO, Vec2::new(1.0, 0.0))],
                        closed: false,
                    }],
                },
                locus_trace::Group {
                    colour: [0, 0, 200],
                    paths: vec![],
                },
            ],
            factor: 1,
        };
        let img = RefImage {
            format: locus_doc::ImageFormat::Png,
            width_px: 4,
            height_px: 2,
            data: std::sync::Arc::from(Vec::new()),
            placement: locus_doc::ImagePlacement {
                origin: Vec2::ZERO,
                mm_per_px: 1.0,
                opacity: 1.0,
                rotation: 0.0,
                mm_per_px_y: None,
            },
        };
        let gs = groups(&img, &traced, Kind::Colours, Output::NewLayer);
        assert_eq!(gs.len(), 2);
        assert!(
            matches!(&gs[0].target, ShapeTarget::NewLayer { name, .. } if name == "Trace #c80000")
        );
        assert_eq!(group_digits(1284), "1 284");
    }
}
