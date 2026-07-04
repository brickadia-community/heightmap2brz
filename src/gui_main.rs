#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use eframe::NativeOptions;
use heightmap::gui::{BrzApp, logger};
use log::info;

// run the window with glium
fn main() -> Result<(), eframe::Error> {
    logger::init().unwrap();

    eframe::run_native(
        "brztools",
        NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_decorations(true)
                .with_drag_and_drop(true)
                .with_inner_size([600.0, 600.0])
                .with_resizable(true),
            ..Default::default()
        },
        Box::new(|cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            info!("Select a tab to get started.");
            Ok(Box::<BrzApp>::default())
        }),
    )
}
