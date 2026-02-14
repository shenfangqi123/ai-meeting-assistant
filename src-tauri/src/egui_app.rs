use crate::asr::AsrState;
use crate::audio::{CaptureManager, SegmentInfo};
use crate::rag::{
    self, IndexSyncRequest, RagProject, RagProjectCreateRequest, RagProjectDeleteRequest, RagState,
};
use crate::ui_events::{subscribe, UiEventEnvelope};
use crate::{
    normalize_translate_provider, rag_ask_with_provider_inner, RagAskRequest, TranslateProviderState,
};
use eframe::egui;
use serde::Deserialize;
use std::fs;
use std::sync::Arc;
use std::time::Instant;
use tauri::{AppHandle, Manager};
use tokio::sync::broadcast::error::TryRecvError;
#[cfg(target_os = "windows")]
use winit::platform::windows::EventLoopBuilderExtWindows;

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

const TRANSLATE_PROVIDER_ORDER: [&str; 3] = ["ollama", "openai", "local-gpt"];

pub fn run(app: AppHandle) -> Result<(), String> {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("AI Shepherd")
            .with_inner_size([1340.0, 900.0]),
        ..Default::default()
    };
    #[cfg(target_os = "windows")]
    {
        options.event_loop_builder = Some(Box::new(|builder| {
            builder.with_any_thread(true);
        }));
    }
    let app_handle = app.clone();
    eframe::run_native(
        "AI Shepherd",
        options,
        Box::new(move |cc| {
            install_cjk_fallback_fonts(cc);
            Ok(Box::new(EguiApp::new(app_handle.clone())))
        }),
    )
    .map_err(|err| err.to_string())
}

fn install_cjk_fallback_fonts(cc: &eframe::CreationContext<'_>) {
    #[cfg(target_os = "windows")]
    {
        let candidates = [
            r"C:\Windows\Fonts\msyh.ttc",   // Microsoft YaHei
            r"C:\Windows\Fonts\simhei.ttf", // SimHei
            r"C:\Windows\Fonts\meiryo.ttc", // Meiryo
            r"C:\Windows\Fonts\msgothic.ttc",
        ];

        let Some((name, bytes)) = candidates.iter().find_map(|path| {
            fs::read(path)
                .ok()
                .map(|bytes| (format!("cjk:{}", path), bytes))
        }) else {
            return;
        };

        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert(name.clone(), egui::FontData::from_owned(bytes).into());

        if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            family.insert(0, name.clone());
        }
        if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            family.insert(0, name);
        }
        cc.egui_ctx.set_fonts(fonts);
    }
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
    capture_running: bool,
    asr_provider: String,
    asr_fallback: bool,
    asr_language: String,
    translate_provider: String,
    segment_translate_enabled: bool,
    status_line: String,
    projects: Vec<RagProject>,
    selected_project_id: String,
    new_project_name: String,
    new_project_root: String,
    rag_query: String,
    rag_allow_out_of_context: bool,
    rag_output: String,
}

impl EguiApp {
    fn new(app: AppHandle) -> Self {
        let mut this = Self {
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
            capture_running: false,
            asr_provider: "whisperserver".to_string(),
            asr_fallback: true,
            asr_language: "ja".to_string(),
            translate_provider: "ollama".to_string(),
            segment_translate_enabled: false,
            status_line: String::new(),
            projects: Vec::new(),
            selected_project_id: String::new(),
            new_project_name: String::new(),
            new_project_root: String::new(),
            rag_query: String::new(),
            rag_allow_out_of_context: false,
            rag_output: String::new(),
        };
        this.refresh_runtime_state();
        this.reload_projects();
        this
    }

    fn set_status(&mut self, text: impl Into<String>) {
        self.status_line = text.into();
    }

    fn refresh_runtime_state(&mut self) {
        if let Some(capture) = self.app.try_state::<CaptureManager>() {
            self.capture_running = capture.is_running();
        }
        if let Some(asr_state) = self.app.try_state::<AsrState>() {
            self.asr_provider = asr_state.provider();
            self.asr_fallback = asr_state.fallback_to_openai();
            self.asr_language = asr_state.language();
        }
        if let Some(provider_state) = self.app.try_state::<TranslateProviderState>() {
            self.translate_provider = provider_state
                .provider
                .lock()
                .map(|value| value.clone())
                .unwrap_or_else(|_| "ollama".to_string());
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
            "segment_created" | "segment_translated" => {
                if let Some(info) = Self::parse_event::<SegmentInfo>(event) {
                    self.upsert_segment(info);
                }
            }
            "segment_transcribed" => {
                if let Some(info) = Self::parse_event::<SegmentInfo>(event) {
                    let name = info.name.clone();
                    self.upsert_segment(info);
                    if self.segment_translate_enabled {
                        self.request_segment_translation(&name);
                    }
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

    fn request_segment_translation(&mut self, name: &str) {
        let Some(manager) = self.app.try_state::<CaptureManager>() else {
            self.set_status("capture manager unavailable");
            return;
        };
        if let Err(err) = manager.translate_segment(
            self.app.clone(),
            name.to_string(),
            Some(self.translate_provider.clone()),
        ) {
            self.set_status(format!("translate enqueue failed: {err}"));
        }
    }

    fn queue_missing_segment_translations(&mut self) {
        let names = self
            .segments
            .iter()
            .filter(|segment| {
                segment
                    .transcript
                    .as_deref()
                    .is_some_and(|text| !text.trim().is_empty())
                    && segment
                        .translation
                        .as_deref()
                        .is_none_or(|text| text.trim().is_empty())
            })
            .map(|segment| segment.name.clone())
            .collect::<Vec<_>>();
        for name in names {
            self.request_segment_translation(&name);
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

    fn toggle_capture(&mut self) {
        let Some(manager) = self.app.try_state::<CaptureManager>() else {
            self.set_status("capture manager unavailable");
            return;
        };
        let result = if manager.is_running() {
            manager.stop(&self.app, false)
        } else {
            manager.start(self.app.clone())
        };
        match result {
            Ok(_) => self.refresh_runtime_state(),
            Err(err) => self.set_status(format!("capture error: {err}")),
        }
    }

    fn clear_segments(&mut self) {
        let Some(manager) = self.app.try_state::<CaptureManager>() else {
            self.set_status("capture manager unavailable");
            return;
        };
        match manager.clear(self.app.clone()) {
            Ok(_) => {
                self.refresh_runtime_state();
                self.set_status("segments cleared");
            }
            Err(err) => self.set_status(format!("clear failed: {err}")),
        }
    }

    fn cycle_asr_provider(&mut self) {
        let Some(state) = self.app.try_state::<AsrState>() else {
            self.set_status("asr state unavailable");
            return;
        };
        let next = if state.provider() == "whisperserver" {
            "openai".to_string()
        } else {
            "whisperserver".to_string()
        };
        let updated = state.set_provider(next);
        self.asr_provider = updated;
    }

    fn set_asr_fallback(&mut self, value: bool) {
        let Some(state) = self.app.try_state::<AsrState>() else {
            self.set_status("asr state unavailable");
            return;
        };
        self.asr_fallback = state.set_fallback_to_openai(value);
    }

    fn set_asr_language(&mut self, language: &str) {
        let Some(state) = self.app.try_state::<AsrState>() else {
            self.set_status("asr state unavailable");
            return;
        };
        self.asr_language = state.set_language(language.to_string());
    }

    fn cycle_translate_provider(&mut self) {
        let Some(state) = self.app.try_state::<TranslateProviderState>() else {
            self.set_status("translate provider state unavailable");
            return;
        };
        let current = state
            .provider
            .lock()
            .map(|value| value.clone())
            .unwrap_or_else(|_| "ollama".to_string());
        let current_index = TRANSLATE_PROVIDER_ORDER
            .iter()
            .position(|provider| *provider == current)
            .unwrap_or(0);
        let next = TRANSLATE_PROVIDER_ORDER[(current_index + 1) % TRANSLATE_PROVIDER_ORDER.len()];
        let normalized = normalize_translate_provider(next);
        if let Ok(mut guard) = state.provider.lock() {
            *guard = normalized.clone();
        }
        self.translate_provider = normalized;
    }

    fn reload_projects(&mut self) {
        match rag::rag_project_list(self.app.clone()) {
            Ok(response) => {
                self.projects = response.projects;
                if self.selected_project_id.is_empty() {
                    if let Some(project) = self.projects.first() {
                        self.selected_project_id = project.project_id.clone();
                    }
                } else if !self
                    .projects
                    .iter()
                    .any(|project| project.project_id == self.selected_project_id)
                {
                    self.selected_project_id.clear();
                }
            }
            Err(err) => self.set_status(format!("load projects failed: {err}")),
        }
    }

    fn selected_project(&self) -> Option<&RagProject> {
        self.projects
            .iter()
            .find(|project| project.project_id == self.selected_project_id)
    }

    fn create_project(&mut self) {
        let name = self.new_project_name.trim();
        let root = self.new_project_root.trim();
        if name.is_empty() || root.is_empty() {
            self.set_status("project name/root is required");
            return;
        }
        let created = match rag::rag_project_create(
            self.app.clone(),
            RagProjectCreateRequest {
                project_name: name.to_string(),
                root_dir: root.to_string(),
            },
        ) {
            Ok(project) => project,
            Err(err) => {
                self.set_status(format!("create project failed: {err}"));
                return;
            }
        };

        let Some(state) = self.app.try_state::<Arc<RagState>>() else {
            self.set_status("rag state unavailable");
            return;
        };
        match rag::rag_index_sync_project_direct(
            &self.app,
            state.inner(),
            IndexSyncRequest {
                project_id: created.project_id.clone(),
                root_dir: Some(created.root_dir.clone()),
            },
        ) {
            Ok(report) => {
                self.selected_project_id = created.project_id.clone();
                self.reload_projects();
                self.set_status(format!(
                    "project indexed: indexed={} updated={} deleted={} chunks+={} chunks-={}",
                    report.indexed_files,
                    report.updated_files,
                    report.deleted_files,
                    report.chunks_added,
                    report.chunks_deleted
                ));
            }
            Err(err) => self.set_status(format!("index project failed: {err}")),
        }
    }

    fn sync_selected_project(&mut self) {
        let Some(project) = self.selected_project().cloned() else {
            self.set_status("select a project first");
            return;
        };
        let Some(state) = self.app.try_state::<Arc<RagState>>() else {
            self.set_status("rag state unavailable");
            return;
        };
        match rag::rag_index_sync_project_direct(
            &self.app,
            state.inner(),
            IndexSyncRequest {
                project_id: project.project_id.clone(),
                root_dir: Some(project.root_dir.clone()),
            },
        ) {
            Ok(report) => self.set_status(format!(
                "sync done: indexed={} updated={} deleted={} chunks+={} chunks-={}",
                report.indexed_files,
                report.updated_files,
                report.deleted_files,
                report.chunks_added,
                report.chunks_deleted
            )),
            Err(err) => self.set_status(format!("sync failed: {err}")),
        }
    }

    fn delete_selected_project(&mut self) {
        let Some(project) = self.selected_project().cloned() else {
            self.set_status("select a project first");
            return;
        };
        match rag::rag_project_delete_direct(
            &self.app,
            RagProjectDeleteRequest {
                project_id: project.project_id.clone(),
            },
        ) {
            Ok(report) => {
                self.reload_projects();
                self.set_status(format!(
                    "project deleted: files={} chunks={}",
                    report.deleted_files, report.deleted_chunks
                ));
            }
            Err(err) => self.set_status(format!("delete failed: {err}")),
        }
    }

    fn ask_rag(&mut self) {
        let query = self.rag_query.trim().to_string();
        if query.is_empty() {
            self.set_status("rag query is empty");
            return;
        }
        let Some(project) = self.selected_project().cloned() else {
            self.set_status("select a project first");
            return;
        };
        let Some(rag_state) = self.app.try_state::<Arc<RagState>>() else {
            self.set_status("rag state unavailable");
            return;
        };
        let provider = self.translate_provider.clone();
        let response = tauri::async_runtime::block_on(rag_ask_with_provider_inner(
            self.app.clone(),
            rag_state.inner().clone(),
            provider,
            RagAskRequest {
                query,
                project_ids: vec![project.project_id.clone()],
                top_k: Some(8),
                allow_out_of_context: Some(self.rag_allow_out_of_context),
            },
        ));
        match response {
            Ok(answer) => {
                let refs = answer
                    .references
                    .iter()
                    .map(|reference| {
                        format!(
                            "[{}] {:.4} {}",
                            reference.index, reference.score, reference.file_path
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                self.rag_output = format!(
                    "provider: {}\n\n{}\n\nreferences:\n{}",
                    answer.provider, answer.answer, refs
                );
                self.set_status("rag answered");
            }
            Err(err) => self.set_status(format!("rag ask failed: {err}")),
        }
    }
}

impl eframe::App for EguiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.refresh_runtime_state();

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("AI Shepherd (egui)");
                ui.separator();
                ui.label(format!("uptime: {}s", self.started_at.elapsed().as_secs()));
                ui.separator();
                ui.label(format!("segments: {}", self.segments.len()));
                if ui
                    .button(if self.capture_running {
                        "Stop Capture"
                    } else {
                        "Start Capture"
                    })
                    .clicked()
                {
                    self.toggle_capture();
                }
                if ui.button("Clear Segments").clicked() {
                    self.clear_segments();
                }
                if ui.button("Exit").clicked() {
                    self.app.exit(0);
                }
            });

            ui.horizontal_wrapped(|ui| {
                if ui.button(format!("ASR: {}", self.asr_provider)).clicked() {
                    self.cycle_asr_provider();
                }
                let mut fallback = self.asr_fallback;
                if ui.checkbox(&mut fallback, "OpenAI fallback").changed() {
                    self.set_asr_fallback(fallback);
                }
                egui::ComboBox::from_label("Language")
                    .selected_text(self.asr_language.clone())
                    .show_ui(ui, |ui| {
                        for candidate in ["zh", "en", "ja"] {
                            if ui
                                .selectable_label(self.asr_language == candidate, candidate)
                                .clicked()
                            {
                                self.set_asr_language(candidate);
                            }
                        }
                    });
                if ui
                    .button(format!("Translate: {}", self.translate_provider))
                    .clicked()
                {
                    self.cycle_translate_provider();
                }
                let changed = ui
                    .checkbox(&mut self.segment_translate_enabled, "Auto Segment Translate")
                    .changed();
                if changed && self.segment_translate_enabled {
                    self.queue_missing_segment_translations();
                }
            });
        });

        egui::TopBottomPanel::bottom("status_panel").show(ctx, |ui| {
            if !self.status_line.is_empty() {
                ui.label(self.status_line.as_str());
            } else {
                ui.label("ready");
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |columns| {
                columns[0].group(|ui| {
                    ui.label("Project Management");
                    if ui.button("Reload Projects").clicked() {
                        self.reload_projects();
                    }
                    egui::ComboBox::from_label("Current Project")
                        .selected_text(
                            self.selected_project()
                                .map(|project| project.project_name.as_str())
                                .unwrap_or("(none)"),
                        )
                        .show_ui(ui, |ui| {
                            for project in &self.projects {
                                let selected = self.selected_project_id == project.project_id;
                                if ui
                                    .selectable_label(
                                        selected,
                                        format!("{} ({})", project.project_name, project.project_id),
                                    )
                                    .clicked()
                                {
                                    self.selected_project_id = project.project_id.clone();
                                }
                            }
                        });
                    ui.horizontal(|ui| {
                        if ui.button("Sync Selected").clicked() {
                            self.sync_selected_project();
                        }
                        if ui.button("Delete Selected").clicked() {
                            self.delete_selected_project();
                        }
                    });
                    ui.separator();
                    ui.label("Create Project");
                    ui.text_edit_singleline(&mut self.new_project_name);
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.new_project_root);
                        if ui.button("Pick Folder").clicked() {
                            if let Some(path) = rag::rag_pick_folder() {
                                self.new_project_root = path;
                            }
                        }
                    });
                    if ui.button("Create + Index").clicked() {
                        self.create_project();
                    }
                    ui.separator();
                    ui.label("RAG Ask");
                    ui.text_edit_singleline(&mut self.rag_query);
                    ui.checkbox(&mut self.rag_allow_out_of_context, "Allow out of context");
                    if ui.button("Ask").clicked() {
                        self.ask_rag();
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("rag_output_scroll")
                        .max_height(240.0)
                        .show(ui, |ui| {
                            ui.monospace(if self.rag_output.is_empty() {
                                "(no response)"
                            } else {
                                self.rag_output.as_str()
                            });
                        });
                });

                columns[1].group(|ui| {
                    ui.label("Live");
                    ui.separator();
                    ui.label(format!("Speaker: {}", self.live_speaker));
                    ui.label(format!("Meta: {}", self.live_meta));
                    ui.label("Partial:");
                    ui.monospace(if self.live_partial.is_empty() {
                        "(waiting)"
                    } else {
                        self.live_partial.as_str()
                    });
                    ui.separator();
                    ui.label("Final:");
                    ui.monospace(if self.live_final.is_empty() {
                        "(waiting)"
                    } else {
                        self.live_final.as_str()
                    });
                    ui.separator();
                    ui.label("Segments");
                    egui::ScrollArea::vertical()
                        .id_salt("segments_scroll")
                        .auto_shrink([false; 2])
                        .max_height(520.0)
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
            });
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}
