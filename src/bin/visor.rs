use eframe::egui;
use memory_visor::edge::ui::app::sample_window;

const TITLE: &str = "Visor de memoria";

fn main() -> eframe::Result {
    let app = match sample_window() {
        Some(app) => app,
        None => return Ok(()),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([1000.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        TITLE,
        options,
        Box::new(move |creation| {
            creation.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(app))
        }),
    )
}
