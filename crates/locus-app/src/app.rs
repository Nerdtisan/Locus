// SPDX-License-Identifier: AGPL-3.0-or-later

//! Placeholder screen until the M1 app shell (#65) replaces it.

/// The Locus application state. Empty until M1.
#[derive(Debug)]
pub struct App;

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.heading("Locus");
                ui.label("Constrained 2D vector editor for plotters, cutters and lasers.");
                ui.add_space(8.0);
                ui.label(format!(
                    "v{} · pre-alpha: scaffolding only, no editing yet",
                    env!("CARGO_PKG_VERSION")
                ));
                ui.add_space(8.0);
                ui.hyperlink_to("Source on GitHub", "https://github.com/Nerdtisan/Locus");
            });
        });
    }
}
