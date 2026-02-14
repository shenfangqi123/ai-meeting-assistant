use crate::audio::SegmentInfo;
use crate::ui_events::{subscribe, UiEventEnvelope};
use eframe::egui;
use serde::Deserialize;
use std::time::Instant;
use tauri::AppHandle;
use tokio::sync::broadcast::error::TryRecvError;

#[derive(Debug, Clone, Deserialize)]
struct WindowTranscript {
    text: String,
    window_ms: u64,
    elapsed_ms: u64,
    speaker_id: Option<u32>,
    speaker_mixed: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct LiveTranslationStart {
    id: String,
    order: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct LiveTranslationChunk {
    id: String,
    order: u64,
    chunk: String,
}

#[derive(Debug, Clone, Deserialize)]
struct LiveTranslationDone {
    id: String,
    order: u64,
    translation: String,
}

#[derive(Debug, Clone, Deserialize)]
struct LiveTranslationError {
    id: String,
    order: u64,
    error: String,
}

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
    event_rx: tokio::sync::broadcast::Receiver<UiEventEnvelope>,
    segments: Vec<SegmentInfo>,
    live_partial: String,
    live_final: String,
    live_meta: String,
    live_speaker: String,
    live_stream_order: u64,
    live_stream_set: bool,
    live_stream_id: String,
}

impl EguiApp {
    fn new(app: AppHandle) -> Self {
        Self {
            app,
            started_at: Instant::now(),
            event_rx: subscribe(),
            segments: Vec::new(),
            live_partial: String::new(),
            live_final: String::new(),
            live_meta: "Idle".to_string(),
            live_speaker: "Speaker ?".to_string(),
            live_stream_order: 0,
            live_stream_set: false,
            live_stream_id: String::new(),
        }
    }

    fn reset_live(&mut self) {
        self.live_partial.clear();
        self.live_final.clear();
        self.live_meta = "Idle".to_string();
        self.live_speaker = "Speaker ?".to_string();
        self.live_stream_order = 0;
        self.live_stream_set = false;
        self.live_stream_id.clear();
    }

    fn upsert_segment(&mut self, info: SegmentInfo) {
        if let Some(entry) = self.segments.iter_mut().find(|entry| entry.name == info.name) {
            *entry = info;
            return;
        }
        self.segments.push(info);
        self.segments.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then(left.name.cmp(&right.name))
        });
    }

    fn parse_event<T: serde::de::DeserializeOwned>(event: UiEventEnvelope) -> Option<T> {
        serde_json::from_value::<T>(event.payload).ok()
    }

    fn handle_event(&mut self, event: UiEventEnvelope) {
        match event.name.as_str() {
            "segment_created" | "segment_transcribed" | "segment_translated" => {
                if let Some(info) = Self::parse_event::<SegmentInfo>(event) {
                    self.upsert_segment(info);
                }
            }
            "segment_list_cleared" => {
                self.segments.clear();
                self.reset_live();
            }
            "window_transcribed" => {
                if let Some(payload) = Self::parse_event::<WindowTranscript>(event) {
                    self.live_partial = payload.text.trim().to_string();
                    self.live_meta = format!(
                        "{:.1}s window | {:.1}s",
                        payload.window_ms as f32 / 1000.0,
                        payload.elapsed_ms as f32 / 1000.0
                    );
                    self.live_speaker = match (payload.speaker_mixed, payload.speaker_id) {
                        (true, _) | (_, None) => "Speaker ?".to_string(),
                        (_, Some(speaker_id)) => format!("Speaker {speaker_id}"),
                    };
                }
            }
            "live_translation_start" => {
                if let Some(payload) = Self::parse_event::<LiveTranslationStart>(event) {
                    if !self.live_stream_set || payload.order >= self.live_stream_order {
                        self.live_stream_set = true;
                        self.live_stream_order = payload.order;
                        self.live_stream_id = payload.id;
                        self.live_final.clear();
                    }
                }
            }
            "live_translation_chunk" => {
                if let Some(payload) = Self::parse_event::<LiveTranslationChunk>(event) {
                    if !self.live_stream_set || payload.order < self.live_stream_order {
                        return;
                    }
                    if !self.live_stream_id.is_empty() && payload.id != self.live_stream_id {
                        return;
                    }
                    if payload.order > self.live_stream_order {
                        self.live_stream_order = payload.order;
                        self.live_stream_id = payload.id;
                        self.live_final.clear();
                    }
                    self.live_final.push_str(&payload.chunk);
                }
            }
            "live_translation_done" => {
                if let Some(payload) = Self::parse_event::<LiveTranslationDone>(event) {
                    if !self.live_stream_set || payload.order >= self.live_stream_order {
                        self.live_stream_set = true;
                        self.live_stream_order = payload.order;
                        self.live_stream_id = payload.id;
                        self.live_final = payload.translation.trim().to_string();
                    }
                }
            }
            "live_translation_error" => {
                if let Some(payload) = Self::parse_event::<LiveTranslationError>(event) {
                    if !self.live_stream_set || payload.order >= self.live_stream_order {
                        self.live_stream_set = true;
                        self.live_stream_order = payload.order;
                        self.live_stream_id = payload.id;
                        self.live_final = payload.error;
                    }
                }
            }
            "live_translation_cleared" => self.reset_live(),
            _ => {}
        }
    }

    fn drain_events(&mut self) {
        loop {
            match self.event_rx.try_recv() {
                Ok(event) => self.handle_event(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Lagged(_)) => continue,
                Err(TryRecvError::Closed) => break,
            }
        }
    }
}

impl eframe::App for EguiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("AI Shepherd (egui)");
                ui.separator();
                ui.label(format!("uptime: {}s", self.started_at.elapsed().as_secs()));
                ui.separator();
                ui.label(format!("segments: {}", self.segments.len()));
                if ui.button("Exit").clicked() {
                    self.app.exit(0);
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.group(|ui| {
                ui.label("Live");
                ui.separator();
                ui.label(format!("Speaker: {}", self.live_speaker));
                ui.label(format!("Meta: {}", self.live_meta));
                ui.add_space(4.0);
                ui.label("Partial:");
                ui.monospace(if self.live_partial.is_empty() {
                    "(waiting)"
                } else {
                    &self.live_partial
                });
                ui.add_space(4.0);
                ui.label("Final:");
                ui.monospace(if self.live_final.is_empty() {
                    "(waiting)"
                } else {
                    &self.live_final
                });
            });

            ui.add_space(8.0);
            ui.label("Segments");
            egui::ScrollArea::vertical()
                .id_salt("segments_scroll")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    for segment in &self.segments {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.strong(&segment.name);
                                ui.separator();
                                ui.label(format!("{} ms", segment.duration_ms));
                            });
                            ui.label(
                                segment
                                    .transcript
                                    .as_deref()
                                    .unwrap_or("Transcribing...")
                                    .trim(),
                            );
                            if let Some(translation) = &segment.translation {
                                if !translation.trim().is_empty() {
                                    ui.separator();
                                    ui.monospace(translation.trim());
                                }
                            }
                        });
                    }
                });
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}
