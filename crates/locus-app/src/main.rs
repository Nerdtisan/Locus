// SPDX-License-Identifier: AGPL-3.0-or-later

//! Desktop + web GUI shell (SPEC §3), on eframe + egui_dock + wgpu (ADR 0001).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod canvas;
mod constrain;
mod demo;
mod file_io;
mod fonts;
mod grid;
mod hit;
mod infer;
mod render;
mod scene;
mod settings;
mod tools;
mod view;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("Locus"),
        ..Default::default()
    };
    eframe::run_native(
        "locus",
        options,
        Box::new(|cc| {
            let mut app = app::App::new(cc);
            // Dev aid: `locus --stress` opens the 5 000-curve sample.
            if std::env::args().any(|a| a == "--stress") {
                app.load_stress();
            }
            Ok(Box::new(app))
        }),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast;
    wasm_bindgen_futures::spawn_local(async {
        let Some(canvas) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("locus_canvas"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        else {
            web_sys::console::error_1(&"locus: canvas #locus_canvas not found".into());
            return;
        };
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
            )
            .await;
        if let Err(e) = started {
            web_sys::console::error_2(&"locus: failed to start".into(), &e);
        }
    });
}
