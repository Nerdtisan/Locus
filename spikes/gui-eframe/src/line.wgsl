// SPDX-License-Identifier: AGPL-3.0-or-later
// Instanced anti-aliased line segments (spike A, SPEC §12.2).
// One instance per segment; six vertices expand it into a quad in the vertex
// shader, and the fragment shader computes coverage from the exact distance to
// the segment (round caps), so AA costs nothing on the CPU.

struct U {
    center_rel: vec2<f32>, // view centre, relative to the upload anchor (mm)
    px_per_mm: f32,        // physical pixels per mm
    half_width: f32,       // physical pixels
    viewport: vec2<f32>,   // physical pixels
    hover: i32,
    selected: i32,
    srgb_target: u32,
    hover_rgb: u32,
    p1: u32,
    p2: u32,
};

@group(0) @binding(0) var<uniform> u: U;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) len_hw: vec2<f32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) a: vec2<f32>,
    @location(1) b: vec2<f32>,
    @location(2) color: u32,
    @location(3) id: u32,
) -> VOut {
    // World y is up, and so is NDC y: no flip needed.
    let pa = (a - u.center_rel) * u.px_per_mm;
    let pb = (b - u.center_rel) * u.px_per_mm;
    var hw = u.half_width;
    var c = unpack4x8unorm(color);
    if (i32(id) == u.selected) {
        hw = hw + 1.5;
        c = vec4<f32>(1.0, 0.72, 0.1, 1.0);
    } else if (i32(id) == u.hover) {
        hw = hw + 1.5;
        let h = unpack4x8unorm(u.hover_rgb);
        c = vec4<f32>(mix(c.rgb, h.rgb / max(h.a, 1e-3), 0.6), 1.0);
    }
    let d = pb - pa;
    let len = length(d);
    var dir = vec2<f32>(1.0, 0.0);
    if (len > 1e-6) {
        dir = d / len;
    }
    let n = vec2<f32>(-dir.y, dir.x);
    let r = hw + 1.0; // one pixel of AA margin
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let k = corners[vi];
    let along = mix(-r, len + r, k.x);
    let across = k.y * r;
    let p = pa + dir * along + n * across;
    var out: VOut;
    out.pos = vec4<f32>(p / (u.viewport * 0.5), 0.0, 1.0);
    out.local = vec2<f32>(along, across);
    out.len_hw = vec2<f32>(len, hw);
    out.color = c;
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    let dx = max(max(-in.local.x, in.local.x - in.len_hw.x), 0.0);
    let dist = length(vec2<f32>(dx, in.local.y));
    let cov = clamp(in.len_hw.y + 0.5 - dist, 0.0, 1.0) * in.color.a;
    var rgb = in.color.rgb;
    if (u.srgb_target == 1u) {
        rgb = srgb_to_linear(rgb);
    }
    // Premultiplied, matching egui's own blend state.
    return vec4<f32>(rgb * cov, cov);
}
