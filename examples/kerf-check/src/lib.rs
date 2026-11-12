// SPDX-License-Identifier: MIT OR Apache-2.0

//! Kerf check: a first-party example validator plugin (SPEC §15.2, §8, #234).
//!
//! For every Cut layer it compares the geometry with the layer's kerf width:
//!
//! - **error:** a circle or arc whose radius is under half the kerf. The beam
//!   or blade is wider than the hole, so the feature is simply removed;
//! - **warning:** a closed path whose narrower side is under `min-feature`
//!   times the kerf. Such small parts tend to burn away or fall out.
//!
//! It only reads the export model it is given and returns findings; it can't
//! change anything (§15.3).

use locus_plugin_api::prelude::*;

struct KerfCheck;

impl InfoGuest for KerfCheck {
    fn describe() -> PluginInfo {
        PluginInfo {
            id: "io.locus.kerf-check".into(),
            name: "Kerf check".into(),
            version: "1.0.0".into(),
        }
    }
}

/// Smallest radius in a path, over its arcs and circles.
fn min_radius(path: &Path) -> Option<f64> {
    path.segments
        .iter()
        .filter_map(|s| match &s.shape {
            Shape::Circle(c) => Some(c.radius),
            Shape::Arc(a) => Some((a.start.x - a.center.x).hypot(a.start.y - a.center.y)),
            Shape::Line(_) => None,
        })
        .reduce(f64::min)
}

/// Width and height of a path's bounding box. Arcs are sampled, which is
/// exact enough for a warning threshold.
fn size(path: &Path) -> (f64, f64) {
    let mut pts: Vec<(f64, f64)> = Vec::new();
    for s in &path.segments {
        match &s.shape {
            Shape::Line(l) => pts.extend([(l.a.x, l.a.y), (l.b.x, l.b.y)]),
            Shape::Circle(c) => pts.extend([
                (c.center.x - c.radius, c.center.y - c.radius),
                (c.center.x + c.radius, c.center.y + c.radius),
            ]),
            Shape::Arc(a) => {
                let r = (a.start.x - a.center.x).hypot(a.start.y - a.center.y);
                let a0 = (a.start.y - a.center.y).atan2(a.start.x - a.center.x);
                let a1 = (a.end.y - a.center.y).atan2(a.end.x - a.center.x);
                let mut sweep = (a1 - a0).rem_euclid(std::f64::consts::TAU);
                if sweep == 0.0 {
                    sweep = std::f64::consts::TAU;
                }
                for i in 0..=32 {
                    let t = a0 + sweep * f64::from(i) / 32.0;
                    pts.push((a.center.x + r * t.cos(), a.center.y + r * t.sin()));
                }
            }
        }
    }
    let fold = |f: fn(f64, f64) -> f64, init: f64, x: bool| {
        pts.iter().map(|p| if x { p.0 } else { p.1 }).fold(init, f)
    };
    (
        fold(f64::max, f64::MIN, true) - fold(f64::min, f64::MAX, true),
        fold(f64::max, f64::MIN, false) - fold(f64::min, f64::MAX, false),
    )
}

impl ValidatorGuest for KerfCheck {
    fn schema() -> Vec<ParamSpec> {
        vec![number(
            "min-feature",
            "Smallest feature, in kerf widths",
            "",
            1.0,
            20.0,
            3.0,
        )]
    }

    fn validate(model: ExportModel, params: Vec<Param>) -> Result<Vec<Finding>, PluginError> {
        let factor = Params::new(&params).number_in("min-feature", 1.0, 20.0)?;
        let mut out = Vec::new();
        for (li, layer) in model.layers.iter().enumerate() {
            let Operation::Cut(cut) = &layer.operation else {
                continue;
            };
            let kerf = cut.kerf_width;
            if kerf <= 0.0 {
                continue;
            }
            for (pi, path) in layer.paths.iter().enumerate() {
                let at = (Some(li as u32), Some(pi as u32));
                if let Some(r) = min_radius(path)
                    && r < kerf / 2.0
                {
                    out.push(Finding {
                        severity: Severity::Error,
                        message: format!(
                            "on layer {}: a radius of {r:.3} mm is under half the {kerf:.3} mm kerf, so the cut removes it",
                            layer.name
                        ),
                        layer: at.0,
                        path: at.1,
                    });
                    continue;
                }
                if !path.closed {
                    continue;
                }
                let (w, h) = size(path);
                let narrow = w.min(h);
                if narrow < factor * kerf {
                    out.push(Finding {
                        severity: Severity::Warning,
                        message: format!(
                            "on layer {}: a {narrow:.3} mm feature is under {factor} times the {kerf:.3} mm kerf and may burn away",
                            layer.name
                        ),
                        layer: at.0,
                        path: at.1,
                    });
                }
            }
        }
        Ok(out)
    }
}

locus_plugin_api::export_plugin!(KerfCheck: validator);
