#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

mod animation;
mod app;
mod diagnostics;
mod export;
mod model;
mod recovery;
mod scene;
mod storage;
mod timeline;
mod ui_style;

use animation::state_at;
use model::{Asset, AssetContent, DrawingElement, Project, Shape, Transform};
#[cfg(test)]
use model::{Pose, SceneObject};

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 760.0])
            .with_min_inner_size([1024.0, 640.0])
            .with_icon(eframe::egui::IconData {
                rgba: include_bytes!("../assets/icon.rgba").to_vec(),
                width: 64,
                height: 64,
            }),
        ..Default::default()
    };
    eframe::run_native(
        "AnimaPro",
        options,
        Box::new(|cc| {
            ui_style::install(&cc.egui_ctx);
            Ok(Box::new(app::AnimaApp::default()))
        }),
    )
}
