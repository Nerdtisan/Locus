// SPDX-License-Identifier: AGPL-3.0-or-later

//! Desktop + web GUI shell (SPEC §3), on eframe + egui_dock + wgpu (ADR 0001).
//!
//! For now this is a placeholder screen so the web pipeline (#38) has
//! something real to build and deploy. The app shell proper is M1 (#65).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1024.0, 700.0])
            .with_title("Locus"),
        ..Default::default()
    };
    eframe::run_native("locus", options, Box::new(|_cc| Ok(Box::new(app::App))))
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
                Box::new(|_cc| Ok(Box::new(app::App))),
            )
            .await;
        if let Err(e) = started {
            web_sys::console::error_2(&"locus: failed to start".into(), &e);
        }
    });
}
