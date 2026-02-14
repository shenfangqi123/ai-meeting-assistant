use eframe::egui;
use std::time::Instant;
use tauri::AppHandle;

pub fn run(app: AppHandle) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("AI Shepherd")
            .with_inner_size([1280.0, 840.0]),
        ..Default::default()
    };
    let app_handle = app.clone();
    eframe::run_native(
        "AI Shepherd",
        options,
        Box::new(move |_cc| Ok(Box::new(EguiApp::new(app_handle.clone())))),
    )
    .map_err(|err| err.to_string())
}

struct EguiApp {
    app: AppHandle,
    started_at: Instant,
}

impl EguiApp {
    fn new(app: AppHandle) -> Self {
        Self {
            app,
            started_at: Instant::now(),
        }
    }
}

impl eframe::App for EguiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("AI Shepherd (egui)");
            ui.separator();
            ui.label("Phase 2: egui shell is active.");
            ui.label(format!(
                "uptime: {}s",
                self.started_at.elapsed().as_secs()
            ));
            ui.add_space(10.0);
            if ui.button("Exit").clicked() {
                self.app.exit(0);
            }
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}
