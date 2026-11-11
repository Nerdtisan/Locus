// SPDX-License-Identifier: MIT OR Apache-2.0

//! Finger-joint box generator: a first-party example plugin (SPEC §13 M3,
//! §15.2, #185).
//!
//! Lays out the flat panels of a box with outer size width × depth × height,
//! cut from material `thickness` thick, in a row: bottom, front, back, left,
//! right and (optionally) a lid. Edges that meet get complementary finger
//! patterns with the same number of fingers, so they interlock.
//!
//! Output is a pure function of the parameters (§15.5): no randomness, no
//! clock, no state.

use locus_plugin_api::prelude::*;

/// Gap between panels in the layout, in mm.
const GAP: f64 = 5.0;
/// Most fingers on one edge; keeps output bounded for silly inputs.
const MAX_FINGERS: f64 = 999.0;

/// What one panel edge does.
#[derive(Clone, Copy, PartialEq)]
enum Edge {
    /// Straight.
    Plain,
    /// Fingers stick out: even segments at the outer edge, odd ones recessed.
    Tabs,
    /// The complement of `Tabs`: even segments recessed.
    Slots,
}

/// Odd number of segments, at least 3, close to `finger` mm each.
fn segment_count(len: f64, finger: f64) -> usize {
    let n = (len / finger).floor().min(MAX_FINGERS) as usize;
    let n = if n.is_multiple_of(2) {
        n.saturating_sub(1)
    } else {
        n
    };
    n.max(3)
}

impl Edge {
    fn segments(self, len: f64, finger: f64) -> usize {
        if self == Edge::Plain {
            1
        } else {
            segment_count(len, finger)
        }
    }

    /// How far segment `k` sits inside the outer edge.
    fn depth(self, k: usize, t: f64) -> f64 {
        match (self, k.is_multiple_of(2)) {
            (Edge::Plain, _) | (Edge::Tabs, true) | (Edge::Slots, false) => 0.0,
            _ => t,
        }
    }
}

/// The outline of a `w` × `h` panel whose lower-left corner is at `origin`.
/// `edges` are bottom, right, top, left (counter-clockwise).
fn panel(
    w: f64,
    h: f64,
    edges: [Edge; 4],
    t: f64,
    finger: f64,
    origin: (f64, f64),
) -> Vec<(f64, f64)> {
    let corners = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    let dirs = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)];
    // Inward normals of a counter-clockwise outline.
    let normals = [(0.0, 1.0), (-1.0, 0.0), (0.0, -1.0), (1.0, 0.0)];
    let lens = [w, h, w, h];
    let counts: Vec<usize> = (0..4).map(|i| edges[i].segments(lens[i], finger)).collect();

    let at = |i: usize, s: f64, depth: f64, extra: (f64, f64)| {
        let (cx, cy) = corners[i];
        (
            origin.0 + cx + s * dirs[i].0 + depth * normals[i].0 + extra.0,
            origin.1 + cy + s * dirs[i].1 + depth * normals[i].1 + extra.1,
        )
    };

    let mut out = Vec::new();
    for i in 0..4 {
        let (e, n) = (edges[i], counts[i]);
        let prev = (i + 3) % 4;
        // The corner is pushed in by both edges that meet there.
        let prev_depth = edges[prev].depth(counts[prev] - 1, t);
        let shift = (prev_depth * normals[prev].0, prev_depth * normals[prev].1);
        out.push(at(i, 0.0, e.depth(0, t), shift));
        for k in 1..n {
            let s = lens[i] * k as f64 / n as f64;
            let (a, b) = (e.depth(k - 1, t), e.depth(k, t));
            if a != b {
                out.push(at(i, s, a, (0.0, 0.0)));
                out.push(at(i, s, b, (0.0, 0.0)));
            }
        }
    }
    out
}

struct FingerBox;

impl InfoGuest for FingerBox {
    fn describe() -> PluginInfo {
        PluginInfo {
            id: "io.locus.finger-box".into(),
            name: "Finger-joint box".into(),
            version: "1.0.0".into(),
        }
    }
}

impl GeneratorGuest for FingerBox {
    fn schema() -> Vec<ParamSpec> {
        vec![
            number("width", "Width", "mm", 20.0, 2000.0, 100.0),
            number("depth", "Depth", "mm", 20.0, 2000.0, 80.0),
            number("height", "Height", "mm", 20.0, 2000.0, 60.0),
            number("thickness", "Material thickness", "mm", 0.5, 25.0, 3.0),
            number("finger", "Finger width", "mm", 2.0, 200.0, 10.0),
            flag("lid", "Lid", false),
        ]
    }

    fn generate(params: Vec<Param>) -> Result<Generated, PluginError> {
        let p = Params::new(&params);
        let w = p.number_in("width", 20.0, 2000.0)?;
        let d = p.number_in("depth", 20.0, 2000.0)?;
        let h = p.number_in("height", 20.0, 2000.0)?;
        let t = p.number_in("thickness", 0.5, 25.0)?;
        let finger = p.number_in("finger", 2.0, 200.0)?;
        let lid = p.boolean("lid")?;
        if w.min(d).min(h) <= 4.0 * t {
            return Err(PluginError::BadParameter(
                "every side must be more than four times the material thickness".into(),
            ));
        }
        if finger < t {
            return Err(PluginError::BadParameter(
                "fingers must be at least as wide as the material is thick".into(),
            ));
        }

        use Edge::{Plain, Slots, Tabs};
        let top = if lid { Slots } else { Plain };
        let mut panels = vec![
            (w, d, [Tabs, Tabs, Tabs, Tabs]),   // bottom
            (w, h, [Slots, Tabs, top, Tabs]),   // front
            (w, h, [Slots, Tabs, top, Tabs]),   // back
            (d, h, [Slots, Slots, top, Slots]), // left
            (d, h, [Slots, Slots, top, Slots]), // right
        ];
        if lid {
            panels.push((w, d, [Tabs, Tabs, Tabs, Tabs]));
        }

        let mut sketch = Sketch::new();
        let mut x = 0.0;
        for (pw, ph, edges) in panels {
            sketch.polygon(&panel(pw, ph, edges, t, finger, (x, 0.0)));
            x += pw + GAP;
        }
        Ok(sketch.finish())
    }
}

locus_plugin_api::export_plugin!(FingerBox: generator);
