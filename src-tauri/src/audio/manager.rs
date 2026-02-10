use crate::app_config::{load_config as load_app_config, AsrConfig};
use crate::asr::AsrState;
use crate::audio::config::{ensure_config_file, load_config};
use crate::audio::speaker::SpeakerDiarizer;
use crate::audio::wasapi::LoopbackCapture;
use crate::audio::writer::SegmentWriter;
use crate::transcribe::{transcribe_file, transcribe_with_whisper_server};
use crate::translate::{
    translate_text_batch_with_options, BatchTranslationItem, BatchTranslationOptions,
    TranslateSource,
};
use chrono::Local;
use hound::{SampleFormat, WavSpec, WavWriter};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const DEFAULT_SEGMENT_TRANSLATE_BATCH_SIZE: usize = 1;
const TRANSLATION_BATCH_POLL_MS: u64 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentInfo {
    pub name: String,
    pub duration_ms: u64,
    pub created_at: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub transcript: Option<String>,
    pub translation: Option<String>,
    pub transcript_at: Option<String>,
    pub translation_at: Option<String>,
    pub transcript_ms: Option<u64>,
    pub translation_ms: Option<u64>,
    pub speaker_id: Option<u32>,
    pub speaker_changed: Option<bool>,
    pub speaker_similarity: Option<f32>,
    pub speaker_switches_ms: Option<Vec<u64>>,
}

#[derive(Debug, Clone)]
struct WindowTask {
    samples: Vec<f32>,
    sample_rate: u32,
    channels: u16,
    window_ms: u64,
    created_at: String,
}

#[derive(Debug, Clone, Serialize)]
struct WindowTranscript {
    text: String,
    window_ms: u64,
    elapsed_ms: u64,
    created_at: String,
    speaker_id: Option<u32>,
    speaker_similarity: Option<f32>,
    speaker_mixed: bool,
}

#[derive(Default)]
struct SpeakerState {
    current_id: Option<u32>,
    current_similarity: Option<f32>,
    last_changed: Option<bool>,
}

impl SpeakerState {
    fn apply_decision(&mut self, speaker_id: Option<u32>, similarity: Option<f32>, mixed: bool) {
        if mixed || speaker_id.is_none() {
            self.current_id = None;
            self.current_similarity = None;
            self.last_changed = None;
            return;
        }
        let speaker_id = speaker_id.unwrap();
        let changed = match self.current_id {
            Some(prev) => prev != speaker_id,
            None => true,
        };
        self.current_id = Some(speaker_id);
        self.current_similarity = similarity;
        self.last_changed = Some(changed);
    }
}

pub struct CaptureManager {
    handle: Mutex<Option<CaptureHandle>>,
    segments: Arc<Mutex<Vec<SegmentInfo>>>,
    queues: Mutex<Option<TaskQueues>>,
    translation_pending: Arc<Mutex<HashMap<String, Option<String>>>>,
    speaker_state: Arc<Mutex<SpeakerState>>,
    translation_generation: Arc<AtomicU64>,
    drop_segment_translation: Arc<AtomicBool>,
}

struct CaptureHandle {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
    stream: Option<StreamHandle>,
}

struct StreamHandle {
    child: Mutex<Child>,
    reader: JoinHandle<()>,
}

#[derive(Clone)]
struct TaskQueues {
    transcribe_tx: mpsc::Sender<String>,
    translation_queue: Arc<TranslationQueue>,
    translation_in_flight: Arc<AtomicBool>,
    window_tx: mpsc::Sender<WindowTask>,
    window_in_flight: Arc<AtomicBool>,
    speaker_state: Arc<Mutex<SpeakerState>>,
}

#[derive(Debug, Clone)]
struct TranslationRequest {
    name: String,
    provider: Option<String>,
    order: usize,
    generation: u64,
}

#[derive(Debug, Clone, Copy)]
struct SegmentTranslationBatchConfig {
    size: usize,
}

#[derive(Debug, Clone)]
struct CleanedBatchItem {
    name: String,
    cleaned_text: String,
}

#[derive(Debug, Default)]
struct SegmentTranslationHistory {
    generation: u64,
    provider: Option<String>,
    previous_batch: Vec<CleanedBatchItem>,
}

struct TranslationQueue {
    state: Mutex<TranslationQueueState>,
    cvar: Condvar,
}

struct TranslationQueueState {
    items: Vec<TranslationRequest>,
    pending: HashSet<String>,
}

impl TranslationQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(TranslationQueueState {
                items: Vec::new(),
                pending: HashSet::new(),
            }),
            cvar: Condvar::new(),
        }
    }

    fn push(&self, request: TranslationRequest) {
        let mut guard = match self.state.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        if guard.pending.contains(&request.name) {
            return;
        }
        let insert_at = guard
            .items
            .iter()
            .position(|item| request.order < item.order)
            .unwrap_or(guard.items.len());
        guard.items.insert(insert_at, request.clone());
        guard.pending.insert(request.name);
        self.cvar.notify_one();
    }

    fn pop(&self) -> TranslationRequest {
        let mut guard = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        loop {
            if !guard.items.is_empty() {
                let request = guard.items.remove(0);
                guard.pending.remove(&request.name);
                return request;
            }
            guard = match self.cvar.wait(guard) {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
    }

    fn try_pop(&self) -> Option<TranslationRequest> {
        let mut guard = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.items.is_empty() {
            return None;
        }
        let request = guard.items.remove(0);
        guard.pending.remove(&request.name);
        Some(request)
    }

    fn clear(&self) {
        if let Ok(mut guard) = self.state.lock() {
            guard.items.clear();
            guard.pending.clear();
        }
    }

    fn len(&self) -> usize {
        match self.state.lock() {
            Ok(guard) => guard.items.len(),
            Err(_) => 0,
        }
    }
}

impl CaptureManager {
    pub fn new() -> Self {
        Self {
            handle: Mutex::new(None),
            segments: Arc::new(Mutex::new(Vec::new())),
            queues: Mutex::new(None),
            translation_pending: Arc::new(Mutex::new(HashMap::new())),
            speaker_state: Arc::new(Mutex::new(SpeakerState::default())),
            translation_generation: Arc::new(AtomicU64::new(0)),
            drop_segment_translation: Arc::new(AtomicBool::new(false)),
        }
    }

    fn ensure_queues(&self, app: &AppHandle, dir: &Path) -> TaskQueues {
        let mut guard = match self.queues.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(existing) = guard.as_ref() {
            return existing.clone();
        }

        let (tx, rx) = mpsc::channel();
        let translation_queue = Arc::new(TranslationQueue::new());
        let translation_in_flight = Arc::new(AtomicBool::new(false));
        let segments = Arc::clone(&self.segments);
        let pending = Arc::clone(&self.translation_pending);
        let generation = Arc::clone(&self.translation_generation);
        let drop_segment_translation = Arc::clone(&self.drop_segment_translation);
        let app_handle = app.clone();
        let dir_buf = dir.to_path_buf();
        let translation_queue_clone = Arc::clone(&translation_queue);
        thread::spawn(move || {
            run_transcription_worker(
                app_handle,
                dir_buf,
                segments,
                rx,
                translation_queue_clone,
                pending,
                generation,
                drop_segment_translation,
            );
        });

        let app_handle = app.clone();
        let dir_buf = dir.to_path_buf();
        let segments = Arc::clone(&self.segments);
        let translation_queue_clone = Arc::clone(&translation_queue);
        let translation_in_flight_clone = Arc::clone(&translation_in_flight);
        let generation = Arc::clone(&self.translation_generation);
        thread::spawn(move || {
            run_translation_worker(
                app_handle,
                dir_buf,
                segments,
                translation_queue_clone,
                translation_in_flight_clone,
                generation,
            );
        });

        let (window_tx, window_rx) = mpsc::channel();
        let window_in_flight = Arc::new(AtomicBool::new(false));
        let app_handle = app.clone();
        let in_flight = Arc::clone(&window_in_flight);
        let speaker_state = Arc::clone(&self.speaker_state);
        thread::spawn(move || {
            run_window_worker(app_handle, window_rx, in_flight, speaker_state);
        });

        let queues = TaskQueues {
            transcribe_tx: tx,
            translation_queue,
            translation_in_flight,
            window_tx,
            window_in_flight,
            speaker_state: Arc::clone(&self.speaker_state),
        };
        *guard = Some(queues.clone());
        queues
    }

    pub fn start(&self, app: AppHandle) -> Result<(), String> {
        let mut guard = self
            .handle
            .lock()
            .map_err(|_| "capture state poisoned".to_string())?;
        if let Some(existing) = guard.take() {
            if existing.handle.is_finished() {
                let _ = existing.handle.join();
            } else {
                *guard = Some(existing);
                return Err("capture already running".to_string());
            }
        }

        let segments_dir = ensure_segments_dir(&app)?;
        self.drop_segment_translation.store(false, Ordering::SeqCst);
        let config = load_config(&app);
        let mut asr_config = load_app_config()
            .ok()
            .and_then(|cfg| cfg.asr)
            .unwrap_or_default();
        if let Some(state) = app.try_state::<AsrState>() {
            let language = state.language();
            if !language.trim().is_empty() {
                asr_config.language = Some(language);
            }
        }
        ensure_config_file(&app, &config);

        let segments = Arc::clone(&self.segments);
        load_index_if_needed(&segments_dir, &segments);
        let queues = self.ensure_queues(&app, &segments_dir);

        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let app_handle = app.clone();

        let handle = std::thread::spawn(move || {
            if let Err(err) = run_capture(
                app_handle,
                segments_dir,
                segments,
                config,
                stop_flag,
                queues,
            ) {
                eprintln!("loopback capture stopped: {err}");
            }
        });

        let stream = start_whisper_stream(&app, &asr_config);
        *guard = Some(CaptureHandle {
            stop,
            handle,
            stream,
        });
        Ok(())
    }

    pub fn stop(&self, app: &AppHandle, drop_translations: bool) -> Result<(), String> {
        if drop_translations {
            self.drop_pending_translations(app);
        }
        let mut guard = self
            .handle
            .lock()
            .map_err(|_| "capture state poisoned".to_string())?;
        let Some(handle) = guard.take() else {
            return Ok(());
        };
        handle.stop.store(true, Ordering::SeqCst);
        let _ = handle.handle.join();
        if let Some(stream) = handle.stream {
            if let Ok(mut child) = stream.child.lock() {
                let _ = child.kill();
            }
            let _ = stream.reader.join();
        }
        Ok(())
    }

    pub fn is_translation_busy(&self) -> bool {
        let pending_busy = self
            .translation_pending
            .lock()
            .map(|guard| !guard.is_empty())
            .unwrap_or(false);
        if pending_busy {
            return true;
        }
        let guard = match self.queues.lock() {
            Ok(guard) => guard,
            Err(_) => return false,
        };
        let Some(queues) = guard.as_ref() else {
            return false;
        };
        if queues.translation_in_flight.load(Ordering::SeqCst) {
            return true;
        }
        queues.translation_queue.len() > 0
    }

    pub fn list(&self, app: AppHandle) -> Result<Vec<SegmentInfo>, String> {
        let segments_dir = ensure_segments_dir(&app)?;
        load_index_if_needed(&segments_dir, &self.segments);
        let guard = self
            .segments
            .lock()
            .map_err(|_| "segment list poisoned".to_string())?;
        Ok(guard.clone())
    }

    pub fn read_segment_bytes(&self, app: AppHandle, name: String) -> Result<Vec<u8>, String> {
        let segments_dir = ensure_segments_dir(&app)?;
        let safe_name = Path::new(&name)
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "invalid segment name".to_string())?;
        if safe_name != name {
            return Err("invalid segment name".to_string());
        }
        let path = segments_dir.join(safe_name);
        fs::read(&path).map_err(|err| err.to_string())
    }

    pub fn clear(&self, app: AppHandle) -> Result<(), String> {
        self.stop(&app, true)?;
        let segments_dir = ensure_segments_dir(&app)?;
        if let Ok(entries) = fs::read_dir(&segments_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let _ = fs::remove_file(path);
                }
            }
        }
        if let Ok(mut guard) = self.segments.lock() {
            guard.clear();
        }
        if let Ok(mut guard) = self.translation_pending.lock() {
            guard.clear();
        }
        if let Ok(mut guard) = self.speaker_state.lock() {
            *guard = SpeakerState::default();
        }
        if let Ok(guard) = self.queues.lock() {
            if let Some(queues) = guard.as_ref() {
                queues.translation_queue.clear();
            }
        }
        if let Some(webview) = app.get_webview("output") {
            let _ = webview.emit("segment_list_cleared", true);
        }
        if let Some(webview) = app.get_webview("output") {
            let _ = webview.emit("live_translation_cleared", true);
        }
        Ok(())
    }

    pub fn translate_segment(
        &self,
        app: AppHandle,
        name: String,
        provider: Option<String>,
    ) -> Result<(), String> {
        let segments_dir = ensure_segments_dir(&app)?;
        let safe_name = Path::new(&name)
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "invalid segment name".to_string())?;
        if safe_name != name {
            return Err("invalid segment name".to_string());
        }
        let queues = self.ensure_queues(&app, &segments_dir);
        let provider = provider.filter(|value| !value.trim().is_empty());
        if self.drop_segment_translation.load(Ordering::SeqCst) {
            return Ok(());
        }

        let transcript_ready = {
            let guard = self.segments.lock().ok();
            guard
                .as_ref()
                .and_then(|segments| {
                    segments
                        .iter()
                        .find(|segment| segment.name == name)
                        .and_then(|segment| segment.transcript.as_ref())
                })
                .is_some()
        };

        if transcript_ready {
            enqueue_translation(
                &queues.translation_queue,
                &self.segments,
                &self.translation_generation,
                name,
                provider,
            );
        } else if let Ok(mut guard) = self.translation_pending.lock() {
            guard.entry(name).or_insert(provider);
        }
        Ok(())
    }

    fn drop_pending_translations(&self, app: &AppHandle) {
        self.drop_segment_translation.store(true, Ordering::SeqCst);
        self.translation_generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut guard) = self.translation_pending.lock() {
            guard.clear();
        }
        if let Ok(guard) = self.queues.lock() {
            if let Some(queues) = guard.as_ref() {
                queues.translation_queue.clear();
            }
        }
        if let Some(webview) = app.get_webview("output") {
            let _ = webview.emit("segment_translation_canceled", true);
        }
    }
}

fn ensure_segments_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let base = app.path().app_data_dir().map_err(|err| err.to_string())?;
    let segments_dir = base.join("segments");
    fs::create_dir_all(&segments_dir).map_err(|err| err.to_string())?;
    Ok(segments_dir)
}

fn index_path(dir: &Path) -> PathBuf {
    dir.join("index.json")
}

fn load_index_if_needed(dir: &Path, segments: &Arc<Mutex<Vec<SegmentInfo>>>) {
    let mut guard = match segments.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    if !guard.is_empty() {
        return;
    }
    let path = index_path(dir);
    if let Ok(content) = fs::read_to_string(&path) {
        if let Ok(list) = serde_json::from_str::<Vec<SegmentInfo>>(&content) {
            *guard = list;
        }
    }
}

pub(crate) fn save_index(dir: &Path, segments: &[SegmentInfo]) -> Result<(), String> {
    let path = index_path(dir);
    let content = serde_json::to_string_pretty(segments).map_err(|err| err.to_string())?;
    fs::write(path, content).map_err(|err| err.to_string())
}

fn run_capture(
    app: AppHandle,
    segments_dir: PathBuf,
    segments: Arc<Mutex<Vec<SegmentInfo>>>,
    config: crate::audio::config::AudioConfig,
    stop: Arc<AtomicBool>,
    queues: TaskQueues,
) -> Result<(), String> {
    let asr_config = load_app_config()
        .ok()
        .and_then(|cfg| cfg.asr)
        .unwrap_or_default();
    let mut capture = LoopbackCapture::new()?;
    let sample_rate = capture.sample_rate();
    let channels = capture.channels().max(1);

    let min_segment_frames = config.min_segment_ms.saturating_mul(sample_rate as u64) / 1000;
    let min_silence_frames = config.min_silence_ms.saturating_mul(sample_rate as u64) / 1000;
    let max_segment_frames = config.max_segment_ms.saturating_mul(sample_rate as u64) / 1000;
    let pre_roll_frames = config.pre_roll_ms.saturating_mul(sample_rate as u64) / 1000;
    let pre_roll_samples = pre_roll_frames.saturating_mul(channels as u64) as usize;
    let rolling_enabled = config.rolling_enabled;
    let window_transcribe_enabled = config.window_transcribe_enabled;
    let rolling_window_frames = config.rolling_window_ms.saturating_mul(sample_rate as u64) / 1000;
    let rolling_step_frames = config.rolling_step_ms.saturating_mul(sample_rate as u64) / 1000;
    let rolling_min_frames = config.rolling_min_ms.saturating_mul(sample_rate as u64) / 1000;
    let rolling_window_samples = rolling_window_frames.saturating_mul(channels as u64) as usize;
    let rolling_min_samples = rolling_min_frames.saturating_mul(channels as u64) as usize;

    let mut pre_roll: VecDeque<f32> = VecDeque::with_capacity(pre_roll_samples.max(1));
    let mut current_writer: Option<SegmentWriter> = None;
    let mut segment_frames: u64 = 0;
    let mut silence_frames: u64 = 0;
    let mut rolling_buffer: VecDeque<f32> = VecDeque::with_capacity(rolling_window_samples.max(1));
    let mut rolling_since_emit: u64 = 0;

    println!(
        "[rolling] enabled={} window_transcribe_enabled={}",
        rolling_enabled, window_transcribe_enabled
    );

    while !stop.load(Ordering::SeqCst) {
        let pcm = capture.read()?;
        if pcm.is_empty() {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }

        let frame_count = (pcm.len() / channels as usize) as u64;
        let is_silence = is_silence(&pcm, config.silence_threshold_db);

        if rolling_enabled
            && window_transcribe_enabled
            && rolling_window_frames > 0
            && rolling_step_frames > 0
        {
            for sample in pcm.iter().copied() {
                rolling_buffer.push_back(sample);
            }
            while rolling_buffer.len() > rolling_window_samples {
                rolling_buffer.pop_front();
            }
            rolling_since_emit = rolling_since_emit.saturating_add(frame_count);
            if rolling_since_emit >= rolling_step_frames {
                rolling_since_emit = 0;
                if rolling_buffer.len() >= rolling_min_samples {
                    let already_running = queues.window_in_flight.swap(true, Ordering::SeqCst);
                    if !already_running {
                        let samples: Vec<f32> = rolling_buffer.iter().copied().collect();
                        let frames_in_buffer = (rolling_buffer.len() / channels as usize) as u64;
                        let window_ms = if sample_rate == 0 {
                            0
                        } else {
                            frames_in_buffer.saturating_mul(1000) / sample_rate as u64
                        };
                        let task = WindowTask {
                            samples,
                            sample_rate,
                            channels,
                            window_ms,
                            created_at: Local::now().to_rfc3339(),
                        };
                        if queues.window_tx.send(task).is_err() {
                            queues.window_in_flight.store(false, Ordering::SeqCst);
                        }
                    }
                }
            }
        }

        for sample in pcm.iter().copied() {
            pre_roll.push_back(sample);
        }
        while pre_roll.len() > pre_roll_samples {
            pre_roll.pop_front();
        }

        if let Some(writer) = current_writer.as_mut() {
            writer.write(&pcm)?;
            segment_frames = segment_frames.saturating_add(frame_count);
            if is_silence {
                silence_frames = silence_frames.saturating_add(frame_count);
            } else {
                silence_frames = 0;
            }

            let reached_min = segment_frames >= min_segment_frames;
            let reached_silence = silence_frames >= min_silence_frames;
            let reached_max = max_segment_frames > 0 && segment_frames >= max_segment_frames;
            if (reached_min && reached_silence) || reached_max {
                let writer = current_writer.take().unwrap();
                finalize_segment(
                    &app,
                    &segments_dir,
                    &segments,
                    &queues,
                    &asr_config,
                    writer,
                    config.min_transcribe_ms,
                );
                segment_frames = 0;
                silence_frames = 0;
            }
            continue;
        }

        if !is_silence {
            let mut writer = SegmentWriter::start_new(&segments_dir, sample_rate, channels)?;
            if !pre_roll.is_empty() {
                let pre_roll_vec: Vec<f32> = pre_roll.iter().copied().collect();
                if !pre_roll_vec.is_empty() {
                    writer.write(&pre_roll_vec)?;
                    let pre_frames = (pre_roll_vec.len() / channels as usize) as u64;
                    segment_frames = segment_frames.saturating_add(pre_frames);
                }
            }
            writer.write(&pcm)?;
            segment_frames = segment_frames.saturating_add(frame_count);
            silence_frames = 0;
            current_writer = Some(writer);
        }
    }

    if let Some(writer) = current_writer.take() {
        finalize_segment(
            &app,
            &segments_dir,
            &segments,
            &queues,
            &asr_config,
            writer,
            config.min_transcribe_ms,
        );
    }

    Ok(())
}

fn finalize_segment_with_vad(
    app: &AppHandle,
    dir: &Path,
    segments: &Arc<Mutex<Vec<SegmentInfo>>>,
    queues: &TaskQueues,
    min_transcribe_ms: u64,
    asr_config: &AsrConfig,
    info: SegmentInfo,
) {
    let path = dir.join(&info.name);
    if min_transcribe_ms > 0 && info.duration_ms < min_transcribe_ms {
        let _ = fs::remove_file(&path);
        return;
    }
    let should_keep = match should_keep_segment(&path, asr_config) {
        Ok(result) => result,
        Err(err) => {
            eprintln!("vad check failed: {err}");
            true
        }
    };

    if should_keep {
        push_segment(app, dir, segments, &queues.speaker_state, info.clone());
        enqueue_transcription(queues, info.name);
    } else {
        let _ = fs::remove_file(&path);
    }
}

fn finalize_segment(
    app: &AppHandle,
    dir: &Path,
    segments: &Arc<Mutex<Vec<SegmentInfo>>>,
    queues: &TaskQueues,
    asr_config: &AsrConfig,
    writer: SegmentWriter,
    min_transcribe_ms: u64,
) {
    let info = match writer.finalize() {
        Ok(info) => info,
        Err(err) => {
            eprintln!("segment finalize failed: {err}");
            return;
        }
    };

    if min_transcribe_ms > 0 && info.duration_ms < min_transcribe_ms {
        let path = dir.join(&info.name);
        let _ = fs::remove_file(&path);
        return;
    }

    if asr_config.use_whisper_vad == Some(true) {
        let app_handle = app.clone();
        let dir_buf = dir.to_path_buf();
        let segments = Arc::clone(segments);
        let queues = queues.clone();
        let asr_config = asr_config.clone();
        thread::spawn(move || {
            finalize_segment_with_vad(
                &app_handle,
                &dir_buf,
                &segments,
                &queues,
                min_transcribe_ms,
                &asr_config,
                info,
            );
        });
        return;
    }

    let name = info.name.clone();
    push_segment(app, dir, segments, &queues.speaker_state, info);
    enqueue_transcription(queues, name);
}

fn enqueue_transcription(queues: &TaskQueues, name: String) {
    let _ = queues.transcribe_tx.send(name);
}

fn apply_transcript(
    app: &AppHandle,
    dir: &Path,
    segments: &Arc<Mutex<Vec<SegmentInfo>>>,
    name: &str,
    transcript: Option<String>,
    elapsed_ms: u64,
) {
    let transcript_text = transcript
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let mut updated: Option<SegmentInfo> = None;
    let mut snapshot: Option<Vec<SegmentInfo>> = None;
    if let Ok(mut guard) = segments.lock() {
        if let Some(segment) = guard.iter_mut().find(|segment| segment.name == name) {
            segment.transcript = transcript;
            segment.transcript_at = Some(Local::now().to_rfc3339());
            segment.transcript_ms = Some(elapsed_ms);
            updated = Some(segment.clone());
            snapshot = Some(guard.clone());
        }
    }
    if let Some(snapshot) = snapshot {
        let _ = save_index(dir, &snapshot);
    }

    if let Some(info) = updated {
        if let Some(webview) = app.get_webview("output") {
            let _ = webview.emit("segment_transcribed", info.clone());
        }
    }

    let _ = transcript_text;
}

fn run_transcription_worker(
    app: AppHandle,
    dir: PathBuf,
    segments: Arc<Mutex<Vec<SegmentInfo>>>,
    rx: mpsc::Receiver<String>,
    translation_queue: Arc<TranslationQueue>,
    pending: Arc<Mutex<HashMap<String, Option<String>>>>,
    translation_generation: Arc<AtomicU64>,
    drop_segment_translation: Arc<AtomicBool>,
) {
    while let Ok(name) = rx.recv() {
        let path = dir.join(&name);
        let thread_id = std::thread::current().id();
        println!("[transcribe] thread={thread_id:?} name={name}");
        let started_at = Instant::now();
        let transcript =
            match tauri::async_runtime::block_on(async { transcribe_file(&app, &path).await }) {
                Ok(text) => Some(text),
                Err(err) => {
                    eprintln!("transcription failed for {name}: {err}");
                    Some(String::new())
                }
            };
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        apply_transcript(&app, &dir, &segments, &name, transcript, elapsed_ms);

        if drop_segment_translation.load(Ordering::SeqCst) {
            continue;
        }
        if let Some(provider) = take_pending_translation(&pending, &name) {
            enqueue_translation(
                &translation_queue,
                &segments,
                &translation_generation,
                name.clone(),
                provider,
            );
        }
    }
}

fn load_segment_translation_batch_config() -> SegmentTranslationBatchConfig {
    let translate = load_app_config().ok().and_then(|cfg| cfg.translate);
    let raw_size = translate
        .as_ref()
        .and_then(|cfg| cfg.segment_batch_size)
        .unwrap_or(DEFAULT_SEGMENT_TRANSLATE_BATCH_SIZE);
    let size = raw_size.max(1);
    SegmentTranslationBatchConfig { size }
}

fn collect_translation_batch(
    queue: &Arc<TranslationQueue>,
    first: TranslationRequest,
    config: SegmentTranslationBatchConfig,
    translation_generation: &Arc<AtomicU64>,
) -> Vec<TranslationRequest> {
    let active_generation = first.generation;
    if active_generation != translation_generation.load(Ordering::SeqCst) {
        return Vec::new();
    }
    if config.size <= 1 {
        return vec![first];
    }

    let mut batch = vec![first];
    while batch.len() < config.size {
        if active_generation != translation_generation.load(Ordering::SeqCst) {
            return Vec::new();
        }
        if let Some(request) = queue.try_pop() {
            if request.generation != active_generation {
                queue.push(request);
                std::thread::sleep(Duration::from_millis(TRANSLATION_BATCH_POLL_MS));
                continue;
            }
            batch.push(request);
            continue;
        }
        std::thread::sleep(Duration::from_millis(TRANSLATION_BATCH_POLL_MS));
    }
    batch
}

fn translate_segment_batch_now(
    app: &AppHandle,
    dir: &Path,
    segments: &Arc<Mutex<Vec<SegmentInfo>>>,
    requests: Vec<TranslationRequest>,
    translation_generation: Arc<AtomicU64>,
    history: &mut SegmentTranslationHistory,
) {
    if requests.is_empty() {
        return;
    }

    let mut group: Vec<TranslationRequest> = Vec::new();
    let mut current_provider: Option<String> = None;
    for request in requests {
        if group.is_empty() {
            current_provider = request.provider.clone();
            group.push(request);
            continue;
        }
        if request.provider == current_provider {
            group.push(request);
            continue;
        }

        translate_segment_provider_group(
            app,
            dir,
            segments,
            std::mem::take(&mut group),
            Arc::clone(&translation_generation),
            history,
        );
        current_provider = request.provider.clone();
        group.push(request);
    }

    if !group.is_empty() {
        translate_segment_provider_group(
            app,
            dir,
            segments,
            group,
            translation_generation,
            history,
        );
    }
}

fn translate_segment_provider_group(
    app: &AppHandle,
    dir: &Path,
    segments: &Arc<Mutex<Vec<SegmentInfo>>>,
    requests: Vec<TranslationRequest>,
    translation_generation: Arc<AtomicU64>,
    history: &mut SegmentTranslationHistory,
) {
    if requests.is_empty() {
        return;
    }

    let active_generation = translation_generation.load(Ordering::SeqCst);
    let provider = requests
        .first()
        .and_then(|request| request.provider.clone());
    if history.generation != active_generation || history.provider != provider {
        history.generation = active_generation;
        history.provider = provider.clone();
        history.previous_batch.clear();
    }

    let mut current_batch_items: Vec<BatchTranslationItem> = Vec::new();
    for request in &requests {
        if request.generation != active_generation {
            continue;
        }
        let transcript = {
            let guard = segments.lock().ok();
            guard.as_ref().and_then(|segments| {
                segments
                    .iter()
                    .find(|segment| segment.name == request.name)
                    .and_then(|segment| segment.transcript.clone())
            })
        };
        let Some(transcript) = transcript else {
            continue;
        };
        current_batch_items.push(BatchTranslationItem {
            id: request.name.clone(),
            text: transcript,
        });
    }

    if current_batch_items.is_empty() {
        return;
    }

    let context_items: Vec<BatchTranslationItem> = history
        .previous_batch
        .iter()
        .map(|item| BatchTranslationItem {
            id: item.name.clone(),
            text: item.cleaned_text.clone(),
        })
        .collect();

    let mut all_items = context_items.clone();
    for item in &current_batch_items {
        if all_items.iter().any(|existing| existing.id == item.id) {
            continue;
        }
        all_items.push(item.clone());
    }

    let all_names: Vec<String> = all_items.iter().map(|item| item.id.clone()).collect();
    let started_at = Instant::now();
    let batch_result = tauri::async_runtime::block_on(async {
        translate_text_batch_with_options(
            &all_items,
            provider.clone(),
            TranslateSource::Segment,
            BatchTranslationOptions {
                context_items: context_items.clone(),
            },
        )
        .await
    });

    match batch_result {
        Ok(translations) => {
            if translation_generation.load(Ordering::SeqCst) != active_generation {
                return;
            }
            let elapsed_ms = started_at.elapsed().as_millis() as u64;
            let mut missing_count = 0usize;
            for name in &all_names {
                let translation = translations
                    .get(name)
                    .map(|item| item.translation.clone())
                    .unwrap_or_else(|| {
                        missing_count += 1;
                        String::new()
                    });
                apply_translation(app, dir, segments, name, Some(translation), elapsed_ms);
            }
            if missing_count > 0 {
                eprintln!(
          "batch translation missing {} item(s), marked as failed without single fallback",
          missing_count
        );
            }

            history.generation = active_generation;
            history.provider = provider;
            history.previous_batch = current_batch_items
                .iter()
                .map(|item| {
                    let cleaned_text = translations
                        .get(&item.id)
                        .and_then(|result| result.cleaned_source.clone())
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or_else(|| item.text.clone());
                    CleanedBatchItem {
                        name: item.id.clone(),
                        cleaned_text,
                    }
                })
                .collect();
        }
        Err(err) => {
            if translation_generation.load(Ordering::SeqCst) != active_generation {
                return;
            }
            let elapsed_ms = started_at.elapsed().as_millis() as u64;
            eprintln!("batch translation failed: {err}");
            for name in all_names {
                apply_translation(app, dir, segments, &name, Some(String::new()), elapsed_ms);
            }
            history.generation = active_generation;
            history.provider = provider;
            history.previous_batch.clear();
        }
    }
}

fn run_translation_worker(
    app: AppHandle,
    dir: PathBuf,
    segments: Arc<Mutex<Vec<SegmentInfo>>>,
    queue: Arc<TranslationQueue>,
    in_flight: Arc<AtomicBool>,
    translation_generation: Arc<AtomicU64>,
) {
    let mut history = SegmentTranslationHistory::default();
    loop {
        let first = queue.pop();
        if first.generation != translation_generation.load(Ordering::SeqCst) {
            continue;
        }
        let batch_config = load_segment_translation_batch_config();
        let batch_requests =
            collect_translation_batch(&queue, first, batch_config, &translation_generation);
        if batch_requests.is_empty() {
            continue;
        }
        eprintln!(
            "[translate-worker] batch_size={} picked={}",
            batch_config.size,
            batch_requests.len()
        );
        in_flight.store(true, Ordering::SeqCst);
        translate_segment_batch_now(
            &app,
            &dir,
            &segments,
            batch_requests,
            Arc::clone(&translation_generation),
            &mut history,
        );
        in_flight.store(false, Ordering::SeqCst);
    }
}

fn run_window_worker(
    app: AppHandle,
    rx: mpsc::Receiver<WindowTask>,
    in_flight: Arc<AtomicBool>,
    speaker_state: Arc<Mutex<SpeakerState>>,
) {
    let mut diarizer = SpeakerDiarizer::new(&app);
    while let Ok(task) = rx.recv() {
        let started_at = Instant::now();
        let mut speaker_decision = None;
        if let Some(diarizer) = diarizer.as_mut() {
            if let Some(decision) =
                diarizer.process_window(&task.samples, task.sample_rate, task.channels)
            {
                speaker_decision = Some(decision.clone());
                if let Ok(mut guard) = speaker_state.lock() {
                    guard.apply_decision(decision.speaker_id, decision.similarity, decision.mixed);
                }
            }
        }
        let path = match window_wav_path(&app) {
            Ok(path) => path,
            Err(err) => {
                eprintln!("window wav path error: {err}");
                in_flight.store(false, Ordering::SeqCst);
                continue;
            }
        };

        if let Err(err) = write_window_wav(&path, &task.samples, task.sample_rate, task.channels) {
            eprintln!("window wav write failed: {err}");
            in_flight.store(false, Ordering::SeqCst);
            continue;
        }

        let mut asr_config = load_app_config()
            .ok()
            .and_then(|cfg| cfg.asr)
            .unwrap_or_default();
        if let Some(state) = app.try_state::<AsrState>() {
            let language = state.language();
            if !language.trim().is_empty() {
                asr_config.language = Some(language);
            }
        }
        let transcript = match tauri::async_runtime::block_on(async {
            transcribe_with_whisper_server(&app, &path, &asr_config).await
        }) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("window transcription failed: {err}");
                in_flight.store(false, Ordering::SeqCst);
                continue;
            }
        };

        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        let text = transcript.trim().to_string();
        let (speaker_id, speaker_similarity, speaker_mixed) = speaker_decision
            .map(|decision| (decision.speaker_id, decision.similarity, decision.mixed))
            .unwrap_or((None, None, false));
        let payload = WindowTranscript {
            text,
            window_ms: task.window_ms,
            elapsed_ms,
            created_at: task.created_at.clone(),
            speaker_id,
            speaker_similarity,
            speaker_mixed,
        };
        if let Some(webview) = app.get_webview("output") {
            let _ = webview.emit("window_transcribed", payload.clone());
        }

        in_flight.store(false, Ordering::SeqCst);
    }
}

fn apply_translation(
    app: &AppHandle,
    dir: &Path,
    segments: &Arc<Mutex<Vec<SegmentInfo>>>,
    name: &str,
    translation: Option<String>,
    elapsed_ms: u64,
) {
    let mut updated: Option<SegmentInfo> = None;
    let mut snapshot: Option<Vec<SegmentInfo>> = None;
    if let Ok(mut guard) = segments.lock() {
        if let Some(segment) = guard.iter_mut().find(|segment| segment.name == name) {
            segment.translation = translation;
            segment.translation_at = Some(Local::now().to_rfc3339());
            segment.translation_ms = Some(elapsed_ms);
            updated = Some(segment.clone());
            snapshot = Some(guard.clone());
        }
    }
    if let Some(snapshot) = snapshot {
        let _ = save_index(dir, &snapshot);
    }

    if let Some(info) = updated {
        if let Some(webview) = app.get_webview("output") {
            let _ = webview.emit("segment_translated", info.clone());
        }
    }
}

fn should_keep_segment(path: &Path, asr_config: &AsrConfig) -> Result<bool, String> {
    if asr_config.use_whisper_vad != Some(true) {
        return Ok(true);
    }

    let vad_exe = asr_config
        .whisper_cpp_vad_path
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "whisper VAD path is required".to_string())?;
    let vad_exe =
        resolve_local_path(&vad_exe).ok_or_else(|| format!("whisper VAD not found: {vad_exe}"))?;

    let model_path = asr_config
        .whisper_cpp_vad_model_path
        .clone()
        .filter(|value| !value.trim().is_empty())
        .and_then(|value| resolve_local_path(&value));
    let Some(model_path) = model_path else {
        eprintln!("whisper VAD model path missing, skip VAD check");
        return Ok(true);
    };

    let mut cmd = Command::new(vad_exe);
    cmd.arg("-f").arg(path).arg("-np");

    cmd.arg("--vad-model").arg(model_path);

    let output = cmd.output().map_err(|err| err.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        return Err(format!("whisper VAD failed: {stderr} {stdout}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(!stdout.trim().is_empty())
}

fn is_silence(pcm: &[f32], threshold_db: f32) -> bool {
    if pcm.is_empty() {
        return true;
    }
    let mut sum = 0.0f32;
    for sample in pcm {
        sum += sample * sample;
    }
    let rms = (sum / pcm.len() as f32).sqrt();
    let db = 20.0 * (rms.max(1e-9)).log10();
    db < threshold_db
}

fn window_wav_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = ensure_segments_dir(app)?;
    Ok(dir.join("window_live.wav"))
}

fn write_window_wav(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<(), String> {
    let spec = WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(path, spec).map_err(|err| err.to_string())?;
    for sample in samples {
        writer
            .write_sample(*sample)
            .map_err(|err| err.to_string())?;
    }
    writer.finalize().map_err(|err| err.to_string())?;
    Ok(())
}

fn resolve_local_path(raw: &str) -> Option<PathBuf> {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        return path.exists().then_some(path);
    }

    if path.exists() {
        return Some(path);
    }

    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join(&path);
        if candidate.exists() {
            return Some(candidate);
        }
        if let Some(parent) = cwd.parent() {
            let candidate = parent.join(&path);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(&path);
            if candidate.exists() {
                return Some(candidate);
            }
            if let Some(parent) = dir.parent() {
                let candidate = parent.join(&path);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

fn resolve_in_same_dir(base: &str, target: &str) -> Option<PathBuf> {
    let base_path = PathBuf::from(base);
    let dir = base_path.parent()?;
    let candidate = dir.join(target);
    candidate.exists().then_some(candidate)
}

fn start_whisper_stream(app: &AppHandle, asr_config: &AsrConfig) -> Option<StreamHandle> {
    if asr_config.use_whisper_stream != Some(true) {
        return None;
    }

    let stream_raw = asr_config
        .whisper_cpp_stream_path
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "whisper-stream.exe".to_string());

    let stream_exe = resolve_local_path(&stream_raw)
        .or_else(|| resolve_in_same_dir(&stream_raw, "whisper-stream.exe"))?;

    let model_raw = asr_config
        .whisper_cpp_model_path
        .clone()
        .filter(|value| !value.trim().is_empty())?;
    let model = resolve_local_path(&model_raw)?;

    let step_ms = asr_config.whisper_cpp_stream_step_ms.unwrap_or(1000);
    let language = asr_config
        .language
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "ja".to_string());

    let mut cmd = Command::new(stream_exe);
    cmd.arg("-m")
        .arg(model)
        .arg("--step")
        .arg(step_ms.to_string())
        .arg("-l")
        .arg(language)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let app_handle = app.clone();

    let reader = thread::spawn(move || {
        let mut stdout_reader = std::io::BufReader::new(stdout);
        let mut stderr_reader = std::io::BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = stdout_reader.read_line(&mut line).unwrap_or(0);
            if bytes == 0 {
                break;
            }
            let text = line.trim();
            if text.is_empty() {
                continue;
            }
            let _ = app_handle.emit("stream_transcript", text.to_string());
        }

        let mut err_line = String::new();
        loop {
            err_line.clear();
            let bytes = stderr_reader.read_line(&mut err_line).unwrap_or(0);
            if bytes == 0 {
                break;
            }
            let err = err_line.trim();
            if !err.is_empty() {
                eprintln!("whisper-stream: {err}");
            }
        }
    });

    Some(StreamHandle {
        child: Mutex::new(child),
        reader,
    })
}

fn take_pending_translation(
    pending: &Arc<Mutex<HashMap<String, Option<String>>>>,
    name: &str,
) -> Option<Option<String>> {
    let mut guard = pending.lock().ok()?;
    guard.remove(name)
}

fn enqueue_translation(
    queue: &TranslationQueue,
    segments: &Arc<Mutex<Vec<SegmentInfo>>>,
    translation_generation: &Arc<AtomicU64>,
    name: String,
    provider: Option<String>,
) {
    let order = segment_order(segments, &name);
    queue.push(TranslationRequest {
        name,
        provider,
        order,
        generation: translation_generation.load(Ordering::SeqCst),
    });
}

fn segment_order(segments: &Arc<Mutex<Vec<SegmentInfo>>>, name: &str) -> usize {
    let guard = segments.lock().ok();
    guard
        .as_ref()
        .and_then(|segments| segments.iter().position(|segment| segment.name == name))
        .unwrap_or(usize::MAX)
}

fn push_segment(
    app: &AppHandle,
    dir: &Path,
    segments: &Arc<Mutex<Vec<SegmentInfo>>>,
    speaker_state: &Arc<Mutex<SpeakerState>>,
    mut info: SegmentInfo,
) {
    if let Ok(guard) = speaker_state.lock() {
        info.speaker_id = guard.current_id;
        info.speaker_similarity = guard.current_similarity;
        info.speaker_changed = guard.last_changed;
    }
    let mut snapshot: Option<Vec<SegmentInfo>> = None;
    if let Ok(mut guard) = segments.lock() {
        guard.push(info.clone());
        snapshot = Some(guard.clone());
    }
    if let Some(snapshot) = snapshot {
        let _ = save_index(dir, &snapshot);
    }
    if let Some(webview) = app.get_webview("output") {
        let _ = webview.emit("segment_created", info.clone());
    }
}
