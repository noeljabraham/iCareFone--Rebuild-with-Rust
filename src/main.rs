#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod domain;
mod services;
mod whatsapp;

use app::ICareFoneApp;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 740.0])
            .with_min_inner_size([840.0, 600.0])
            .with_title("iCareFone Clone (Offline Rust Utility)"),
        ..Default::default()
    };

    eframe::run_native(
        "iCareFone Clone",
        options,
        Box::new(|cc| Ok(Box::new(ICareFoneApp::new(cc)))),
    )
}

