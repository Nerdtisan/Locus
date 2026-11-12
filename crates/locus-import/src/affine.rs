// SPDX-License-Identifier: AGPL-3.0-or-later

//! 2-D affine maps and elliptical-arc helpers shared by the SVG and DXF
//! importers.

use locus_doc::reference::ImportShape;
use locus_geom::Vec2;

/// `x' = a x + c y + e`, `y' = b x + d y + f` (SVG matrix order).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine::new(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);

    pub const fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Affine { a, b, c, d, e, f }
    }

    pub fn translate(x: f64, y: f64) -> Self {
        Affine::new(1.0, 0.0, 0.0, 1.0, x, y)
    }

    pub fn scale(x: f64, y: f64) -> Self {
        Affine::new(x, 0.0, 0.0, y, 0.0, 0.0)
    }

    pub fn rotate(deg: f64) -> Self {
        let (s, c) = deg.to_radians().sin_cos();
        Affine::new(c, s, -s, c, 0.0, 0.0)
    }

    /// `self` applied after `rhs` (`self * rhs`).
    pub fn then_after(&self, rhs: &Affine) -> Affine {
        Affine {
            a: self.a * rhs.a + self.c * rhs.b,
            b: self.b * rhs.a + self.d * rhs.b,
            c: self.a * rhs.c + self.c * rhs.d,
            d: self.b * rhs.c + self.d * rhs.d,
            e: self.a * rhs.e + self.c * rhs.f + self.e,
            f: self.b * rhs.e + self.d * rhs.f + self.f,
        }
    }

    pub fn apply(&self, p: Vec2) -> Vec2 {
        Vec2::new(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }

    pub fn det(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    pub fn is_finite(&self) -> bool {
        [self.a, self.b, self.c, self.d, self.e, self.f]
            .iter()
            .all(|v| v.is_finite())
    }

    /// Uniform scale (and rotation / mirror): circles stay circles. Returns
    /// the scale factor.
    pub fn similarity(&self) -> Option<f64> {
        let sx = self.a.hypot(self.b);
        let sy = self.c.hypot(self.d);
        let scale = sx.max(sy);
        if scale <= 0.0 || !scale.is_finite() {
            return None;
        }
        let dot = self.a * self.c + self.b * self.d;
        ((sx - sy).abs() <= 1e-9 * scale && dot.abs() <= 1e-9 * scale * scale).then_some(sx)
    }
}

/// Shapes for a full axis-aligned-in-local-space ellipse (four cubics).
pub(crate) fn ellipse(t: &Affine, c: Vec2, rx: f64, ry: f64, out: &mut Vec<ImportShape>) {
    arc_segments(t, c, rx, ry, 0.0, 0.0, std::f64::consts::TAU, out);
}

/// Cubic Béziers for the elliptical arc about `c` (radii `rx`,`ry`, x-axis
/// rotated `phi` radians) from parameter `t0` sweeping `dt` (signed), mapped
/// through `t`. Each piece covers at most 90°.
#[allow(clippy::too_many_arguments)]
pub(crate) fn arc_segments(
    t: &Affine,
    c: Vec2,
    rx: f64,
    ry: f64,
    phi: f64,
    t0: f64,
    dt: f64,
    out: &mut Vec<ImportShape>,
) {
    let n = ((dt.abs() / std::f64::consts::FRAC_PI_2) - 1e-9)
        .ceil()
        .max(1.0) as usize;
    let step = dt / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let (sp, cp) = phi.sin_cos();
    // Point and derivative of the ellipse at parameter `u`.
    let pt = |u: f64| {
        let (s, co) = u.sin_cos();
        Vec2::new(
            c.x + rx * co * cp - ry * s * sp,
            c.y + rx * co * sp + ry * s * cp,
        )
    };
    let dv = |u: f64| {
        let (s, co) = u.sin_cos();
        Vec2::new(-rx * s * cp - ry * co * sp, -rx * s * sp + ry * co * cp)
    };
    for i in 0..n {
        let u0 = t0 + step * i as f64;
        let u1 = u0 + step;
        let (p0, p1) = (pt(u0), pt(u1));
        let (d0, d1) = (dv(u0), dv(u1));
        out.push(ImportShape::Cubic {
            a: t.apply(p0),
            c1: t.apply(Vec2::new(p0.x + k * d0.x, p0.y + k * d0.y)),
            c2: t.apply(Vec2::new(p1.x - k * d1.x, p1.y - k * d1.y)),
            b: t.apply(p1),
        });
    }
}

/// A circular arc as the app's counter-clockwise `Arc`, after mapping through
/// a similarity `t`. `ccw_local`: the arc runs counter-clockwise (increasing
/// angle) from `start` to `end` in the local frame.
pub(crate) fn circular_arc(
    t: &Affine,
    center: Vec2,
    start: Vec2,
    end: Vec2,
    ccw_local: bool,
) -> ImportShape {
    let (c, s, e) = (t.apply(center), t.apply(start), t.apply(end));
    // A mirroring map reverses the sense of rotation.
    let ccw = ccw_local == (t.det() > 0.0);
    if ccw {
        ImportShape::Arc {
            center: c,
            start: s,
            end: e,
        }
    } else {
        ImportShape::Arc {
            center: c,
            start: e,
            end: s,
        }
    }
}
