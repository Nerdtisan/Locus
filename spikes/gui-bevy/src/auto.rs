// SPDX-License-Identifier: AGPL-3.0-or-later
//! The scripted benchmark for RESULTS.md, same protocol as spike A: load the
//! 5 000-line scene, fit all, 30 warm-up + 600 measured frames while the view
//! pans and zooms (vsync off natively; the browser caps at rAF), then report.
//!
//! - Native: `locus-spike-bevy --bench <out-dir>` writes `bench.txt`, saves
//!   screenshots of its own window (build with `--features capture` for PNG
//!   encoding) and exits.
//! - Web: open the page with `?bench`; the result is logged to the console
//!   and the app keeps running.

#[cfg(not(target_arch = "wasm32"))]
use bevy::math::DVec2;
use bevy::prelude::*;

use crate::canvas::Canvas;
use crate::doc;
use crate::ui::{self, Bench};
#[cfg(not(target_arch = "wasm32"))]
use crate::view;

const WARMUP_FRAMES: u32 = 30;
const MEASURED_FRAMES: usize = 600;
#[cfg(not(target_arch = "wasm32"))]
const SHOT_SETTLE_FRAMES: u32 = 15;

#[derive(Debug)]
enum Stage {
    Setup,
    Warmup(u32),
    Measure,
    #[cfg(not(target_arch = "wasm32"))]
    Shot(usize, u32),
    Done(u32),
}

#[derive(Resource, Debug)]
pub struct Auto {
    #[cfg(not(target_arch = "wasm32"))]
    out: std::path::PathBuf,
    stage: Stage,
}

impl Auto {
    /// `Some` when started with `--bench <out-dir>`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn requested() -> Option<Self> {
        let mut args = std::env::args().skip_while(|a| a != "--bench");
        args.next()?;
        Some(Self {
            out: args.next().map_or_else(|| ".".into(), Into::into),
            stage: Stage::Setup,
        })
    }

    /// `Some` when the page URL has a `bench` query parameter.
    #[cfg(target_arch = "wasm32")]
    pub fn requested() -> Option<Self> {
        let search = web_sys::window()?.location().search().ok()?;
        search
            .trim_start_matches('?')
            .split('&')
            .any(|kv| kv == "bench" || kv.starts_with("bench="))
            .then_some(Self {
                stage: Stage::Setup,
            })
    }
}

/// Sets the canvas up for one screenshot.
#[cfg(not(target_arch = "wasm32"))]
type ShotSetup = fn(&mut Canvas);

/// Named views to capture: (file stem, set-up).
#[cfg(not(target_arch = "wasm32"))]
const SHOTS: [(&str, ShotSetup); 3] = [
    ("overview-5000-lines", |c| {
        c.selected = Some(7);
        ui::fit_all(c);
    }),
    // A 1 µm view next to a line end away from the origin: the line, grid and
    // labels stay exact at the smallest extent.
    ("extent-0.001mm", |c| {
        c.view.center = c.lines[7].a + DVec2::new(0.0002, 0.0001);
        c.view.set_extent(view::MIN_EXTENT_MM);
    }),
    ("extent-2m", |c| {
        c.view.center = DVec2::ZERO;
        c.view.px_per_mm = 0.0;
        c.view.clamp_zoom();
    }),
];

fn report(canvas: &Canvas, bench: &Bench) -> String {
    let fmt = |s: &ui::Series| {
        s.stats()
            .map_or_else(|| "n/a".into(), |(m, p)| format!("mean {m:.3} p95 {p:.3}"))
    };
    let size = canvas.view.size;
    format!(
        "frames={} | interval_ms {} | update_ms {} | draw_ms {} | lines={} canvas_pt={}x{} ppp={} arch={}\n",
        bench.interval.len(),
        fmt(&bench.interval),
        fmt(&bench.update),
        fmt(&bench.draw),
        canvas.lines.len(),
        size.x,
        size.y,
        canvas.ppp,
        std::env::consts::ARCH,
    )
}

pub fn run(
    mut auto: ResMut<Auto>,
    mut canvas: ResMut<Canvas>,
    mut bench: ResMut<Bench>,
    #[cfg(not(target_arch = "wasm32"))] mut commands: Commands,
    #[cfg(not(target_arch = "wasm32"))] mut exit: MessageWriter<AppExit>,
) {
    auto.stage = match std::mem::replace(&mut auto.stage, Stage::Setup) {
        // Wait for egui to lay the canvas out so the view knows its size.
        Stage::Setup if canvas.rect.is_none() => Stage::Setup,
        Stage::Setup => {
            canvas.lines = doc::stress_lines(5000, doc::STRESS_SEED);
            ui::fit_all(&mut canvas);
            bench.continuous = true;
            bench.auto_pan = true;
            Stage::Warmup(WARMUP_FRAMES)
        }
        Stage::Warmup(0) => {
            bench.reset();
            Stage::Measure
        }
        Stage::Warmup(n) => Stage::Warmup(n - 1),
        Stage::Measure if bench.interval.len() < MEASURED_FRAMES => Stage::Measure,
        Stage::Measure => {
            let line = report(&canvas, &bench);
            info!("BENCH {}", line.trim_end());
            bench.auto_pan = false;
            #[cfg(not(target_arch = "wasm32"))]
            {
                if let Err(e) = std::fs::write(auto.out.join("bench.txt"), &line) {
                    error!("cannot write bench.txt: {e}");
                }
                Stage::Shot(0, SHOT_SETTLE_FRAMES)
            }
            #[cfg(target_arch = "wasm32")]
            {
                bench.continuous = false;
                Stage::Done(0)
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        Stage::Shot(i, SHOT_SETTLE_FRAMES) => {
            (SHOTS[i].1)(&mut canvas);
            Stage::Shot(i, SHOT_SETTLE_FRAMES - 1)
        }
        #[cfg(not(target_arch = "wasm32"))]
        Stage::Shot(i, 0) => {
            use bevy::render::view::screenshot::{save_to_disk, Screenshot};
            let path = auto.out.join(format!("{}.png", SHOTS[i].0));
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path));
            if i + 1 < SHOTS.len() {
                Stage::Shot(i + 1, SHOT_SETTLE_FRAMES)
            } else {
                Stage::Done(60)
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        Stage::Shot(i, n) => Stage::Shot(i, n - 1),
        Stage::Done(0) => {
            #[cfg(not(target_arch = "wasm32"))]
            exit.write(AppExit::Success);
            Stage::Done(0)
        }
        Stage::Done(n) => Stage::Done(n - 1),
    };
}
