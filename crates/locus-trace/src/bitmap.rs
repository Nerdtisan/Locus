// SPDX-License-Identifier: AGPL-3.0-or-later
//! Images in and one-bit bitmaps out: luminance, Otsu's threshold, Sobel
//! edges, k-means colour quantisation and box downsampling (ADR 0019
//! decision 8). Everything is bounded by the caller's [`Budget`].

use crate::{Budget, TraceError};

/// A one-bit image: `true` is ink. Row 0 is the image's top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub width: usize,
    pub height: usize,
    bits: Vec<bool>,
}

impl Bitmap {
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            bits: vec![false; width * height],
        }
    }

    /// Ink at (x, y); anything outside the image is paper.
    #[must_use]
    pub fn get(&self, x: i64, y: i64) -> bool {
        if x < 0 || y < 0 {
            return false;
        }
        let (x, y) = (x as usize, y as usize);
        x < self.width && y < self.height && self.bits[y * self.width + x]
    }

    pub fn set(&mut self, x: usize, y: usize, v: bool) {
        if x < self.width && y < self.height {
            self.bits[y * self.width + x] = v;
        }
    }

    pub(crate) fn flip(&mut self, x: usize, y: usize) {
        if x < self.width && y < self.height {
            let i = y * self.width + x;
            self.bits[i] = !self.bits[i];
        }
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.bits.iter().filter(|b| **b).count()
    }
}

/// An RGBA8 image as decoded, at most `max_pixels`: larger images are box
/// downsampled by a whole factor (returned) so tracing stays bounded.
#[derive(Clone, Debug)]
pub(crate) struct Pixels {
    pub width: usize,
    pub height: usize,
    /// RGB, alpha composited onto white.
    pub rgb: Vec<[u8; 3]>,
    /// Source pixels per traced pixel, along each axis.
    pub factor: usize,
}

impl Pixels {
    pub(crate) fn from_rgba(
        width: u32,
        height: u32,
        rgba: &[u8],
        max_pixels: usize,
        budget: &mut Budget,
    ) -> Result<Self, TraceError> {
        let (w, h) = (width as usize, height as usize);
        let n = w.checked_mul(h).ok_or(TraceError::BadImage)?;
        if w == 0 || h == 0 || rgba.len() != n.checked_mul(4).ok_or(TraceError::BadImage)? {
            return Err(TraceError::BadImage);
        }
        let mut factor = 1;
        while (w / factor) * (h / factor) > max_pixels.max(1) {
            factor += 1;
        }
        let (tw, th) = ((w / factor).max(1), (h / factor).max(1));
        let mut rgb = Vec::with_capacity(tw * th);
        for ty in 0..th {
            budget.spend(tw * factor * factor)?;
            for tx in 0..tw {
                let mut sum = [0u32; 3];
                let mut k = 0u32;
                for y in ty * factor..((ty + 1) * factor).min(h) {
                    for x in tx * factor..((tx + 1) * factor).min(w) {
                        let i = (y * w + x) * 4;
                        let a = u32::from(rgba[i + 3]);
                        for c in 0..3 {
                            // Transparent shows the white page.
                            sum[c] += (u32::from(rgba[i + c]) * a + 255 * (255 - a)) / 255;
                        }
                        k += 1;
                    }
                }
                let k = k.max(1);
                rgb.push([(sum[0] / k) as u8, (sum[1] / k) as u8, (sum[2] / k) as u8]);
            }
        }
        Ok(Self {
            width: tw,
            height: th,
            rgb,
            factor,
        })
    }

    /// Rec. 709 luminance per pixel.
    pub(crate) fn luminance(&self) -> Vec<u8> {
        self.rgb
            .iter()
            .map(|[r, g, b]| {
                let l = 0.2126 * f64::from(*r) + 0.7152 * f64::from(*g) + 0.0722 * f64::from(*b);
                l.round().clamp(0.0, 255.0) as u8
            })
            .collect()
    }
}

/// Otsu's threshold: the level that best separates the histogram into two
/// classes (maximum between-class variance).
#[must_use]
pub fn otsu(values: &[u8]) -> u8 {
    let mut hist = [0u64; 256];
    for v in values {
        hist[usize::from(*v)] += 1;
    }
    let total = values.len() as f64;
    if total == 0.0 {
        return 128;
    }
    let sum_all: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, c)| i as f64 * *c as f64)
        .sum();
    let (mut w0, mut sum0, mut best, mut level) = (0.0, 0.0, -1.0, 128u8);
    for (t, c) in hist.iter().enumerate() {
        w0 += *c as f64;
        if w0 == 0.0 {
            continue;
        }
        let w1 = total - w0;
        if w1 == 0.0 {
            break;
        }
        sum0 += t as f64 * *c as f64;
        let (m0, m1) = (sum0 / w0, (sum_all - sum0) / w1);
        let between = w0 * w1 * (m0 - m1) * (m0 - m1);
        if between > best {
            best = between;
            level = t as u8;
        }
    }
    level
}

/// Ink where `values` are at most `level` (dark on light).
pub(crate) fn threshold(width: usize, height: usize, values: &[u8], level: u8) -> Bitmap {
    let mut b = Bitmap::new(width, height);
    for (i, v) in values.iter().enumerate() {
        if *v <= level {
            b.bits[i] = true;
        }
    }
    b
}

/// Sobel gradient magnitude, scaled to 0..=255 and inverted so edges are
/// dark (they threshold as ink like any drawing).
pub(crate) fn sobel(
    width: usize,
    height: usize,
    lum: &[u8],
    budget: &mut Budget,
) -> Result<Vec<u8>, TraceError> {
    let at = |x: i64, y: i64| -> f64 {
        let x = x.clamp(0, width as i64 - 1) as usize;
        let y = y.clamp(0, height as i64 - 1) as usize;
        f64::from(lum[y * width + x])
    };
    let mut mags = Vec::with_capacity(width * height);
    for y in 0..height as i64 {
        budget.spend(width)?;
        for x in 0..width as i64 {
            let gx = at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x - 1, y)
                - at(x - 1, y + 1);
            let gy = at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x, y - 1)
                - at(x + 1, y - 1);
            mags.push((gx * gx + gy * gy).sqrt());
        }
    }
    let max = mags.iter().copied().fold(0.0_f64, f64::max);
    let scale = if max > 0.0 { 255.0 / max } else { 0.0 };
    Ok(mags
        .into_iter()
        .map(|m| 255 - (m * scale).round().clamp(0.0, 255.0) as u8)
        .collect())
}

/// `k` colours by k-means (deterministic: the first centres are spread
/// along the luminance order), and each pixel's colour index.
pub(crate) fn quantise(
    px: &Pixels,
    k: usize,
    budget: &mut Budget,
) -> Result<(Vec<[u8; 3]>, Vec<u8>), TraceError> {
    let k = k.clamp(2, crate::MAX_COLOURS);
    let n = px.rgb.len();
    let mut order: Vec<usize> = (0..n).collect();
    let lum = px.luminance();
    order.sort_by_key(|i| lum[*i]);
    let mut centres: Vec<[f64; 3]> = (0..k)
        .map(|c| {
            let i = order[((2 * c + 1) * n / (2 * k)).min(n - 1)];
            px.rgb[i].map(f64::from)
        })
        .collect();
    let mut labels = vec![0u8; n];
    for _ in 0..16 {
        budget.spend(n * k)?;
        let mut sums = vec![[0.0f64; 4]; k];
        let mut changed = false;
        for (i, p) in px.rgb.iter().enumerate() {
            let p = p.map(f64::from);
            let best = (0..k)
                .min_by(|a, b| dist2(&p, &centres[*a]).total_cmp(&dist2(&p, &centres[*b])))
                .unwrap_or(0);
            if labels[i] != best as u8 {
                labels[i] = best as u8;
                changed = true;
            }
            let s = &mut sums[best];
            s[0] += p[0];
            s[1] += p[1];
            s[2] += p[2];
            s[3] += 1.0;
        }
        for (c, s) in centres.iter_mut().zip(&sums) {
            if s[3] > 0.0 {
                *c = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
            }
        }
        if !changed {
            break;
        }
    }
    let colours = centres
        .iter()
        .map(|c| c.map(|v| v.round().clamp(0.0, 255.0) as u8))
        .collect();
    Ok((colours, labels))
}

fn dist2(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn otsu_splits_two_clusters() {
        let mut v = vec![20u8; 100];
        v.extend(vec![220u8; 100]);
        let t = otsu(&v);
        assert!((20..220).contains(&t), "{t}");
    }

    #[test]
    fn large_images_are_downsampled_by_a_whole_factor() {
        let (w, h) = (100u32, 60u32);
        let rgba = vec![255u8; (w * h * 4) as usize];
        let mut budget = Budget::new(u64::MAX);
        let p =
            Pixels::from_rgba(w, h, &rgba, 1_000, &mut budget).unwrap_or_else(|_| unreachable!());
        assert!(p.width * p.height <= 1_000);
        assert_eq!(p.factor, 3);
        assert!(Pixels::from_rgba(w, h, &rgba[..10], 1_000, &mut budget).is_err());
        assert!(Pixels::from_rgba(0, 5, &[], 1_000, &mut budget).is_err());
    }
}
