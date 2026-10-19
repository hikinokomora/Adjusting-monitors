#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod display;
mod edid;
mod layout;
mod overlay;
mod theme;

use eframe::egui;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

fn main() -> eframe::Result {
    // Real pixels everywhere: monitor positions and rulers must not be scaled by Windows.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Выравнивание мониторов")
            .with_inner_size([1240.0, 780.0])
            .with_min_inner_size([820.0, 520.0])
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap_or_default()),
        ..Default::default()
    };
    eframe::run_native(
        "Выравнивание мониторов",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
