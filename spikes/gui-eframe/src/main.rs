// SPDX-License-Identifier: AGPL-3.0-or-later
//! Spike A (#44, SPEC §12.1, §12.2): reference canvas on eframe 0.35 +
//! egui_dock + wgpu. Desktop and web (trunk) from one source.
//!
//! Native flags: `--no-vsync`, `--msaa N`, `--bench` (run the benchmark at
//! start-up, write `spike-eframe-bench.md` and exit), `--shot <dir>`
//! (write the RESULTS.md screenshots and exit). Web: `?bench` in the URL
//! runs it and logs the table to the browser console; `?webgl` forces WebGL2
//! instead of WebGPU.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod doc;
mod gpu;
mod grid;
mod render;
mod view;

#[derive(Clone, Debug)]
pub struct Config {
    pub vsync: bool,
    pub msaa: u32,
    pub bench: bool,
    pub exit_after_bench: bool,
    /// Native only: write scripted screenshots to this folder and exit.
    #[cfg(not(target_arch = "wasm32"))]
    pub shot_dir: Option<std::path::PathBuf>,
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let msaa = args
        .iter()
        .position(|a| a == "--msaa")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(1u16);
    let cfg = Config {
        vsync: !has("--no-vsync"),
        msaa: msaa.into(),
        bench: has("--bench"),
        exit_after_bench: has("--bench"),
        shot_dir: args
            .iter()
            .position(|a| a == "--shot")
            .and_then(|i| args.get(i + 1))
            .map(std::path::PathBuf::from),
    };
    let mut opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("Locus spike A: eframe + egui_dock + wgpu"),
        multisampling: msaa,
        ..Default::default()
    };
    opts.wgpu_options.surface.present_mode = if cfg.vsync {
        eframe::wgpu::PresentMode::AutoVsync
    } else {
        eframe::wgpu::PresentMode::AutoNoVsync
    };
    eframe::run_native(
        "locus-spike-eframe",
        opts,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, cfg)))),
    )
}

#[cfg(not(target_arch = "wasm32"))]
pub fn emit_report(report: &str) {
    println!("{report}");
    // Spike-only convenience: the release .exe has no console.
    if let Err(e) = std::fs::write("spike-eframe-bench.md", report) {
        eprintln!("could not write benchmark report: {e}");
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast;
    let query = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default();
    let bench = query.contains("bench");
    // eframe prefers WebGPU when the browser has it; `?webgl` forces WebGL2
    // so the numbers compare with spike B, which runs on WebGL2.
    let mut web_options = eframe::WebOptions::default();
    if query.contains("webgl") {
        if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) =
            &mut web_options.wgpu_options.wgpu_setup
        {
            setup.instance_descriptor.backends = eframe::wgpu::Backends::GL;
        }
    }
    let cfg = Config {
        vsync: true,
        msaa: 1,
        bench,
        exit_after_bench: false,
    };
    wasm_bindgen_futures::spawn_local(async move {
        let Some(canvas) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("the_canvas_id"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        else {
            web_sys::console::error_1(&"spike: canvas #the_canvas_id not found".into());
            return;
        };
        let res = eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(move |cc| Ok(Box::new(app::App::new(cc, cfg)))),
            )
            .await;
        if let Err(e) = res {
            web_sys::console::error_2(&"spike: failed to start".into(), &e);
        }
    });
}

#[cfg(target_arch = "wasm32")]
pub fn emit_report(report: &str) {
    web_sys::console::log_1(&report.into());
}
