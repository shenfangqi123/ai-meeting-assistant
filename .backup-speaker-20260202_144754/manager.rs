use crate::audio::config::{ensure_config_file, load_config};
use crate::audio::vad::{SilenceDetector, SpeechGate};
use crate::audio::writer::SegmentWriter;
use crate::transcribe::transcribe_file;
use crate::translate::translate_text;
use crate::audio::wasapi::LoopbackCapture;
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

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
}

pub struct CaptureManager {
  handle: Mutex<Option<CaptureHandle>>,
  segments: Arc<Mutex<Vec<SegmentInfo>>>,
}

struct CaptureHandle {
  stop: Arc<AtomicBool>,
  handle: JoinHandle<()>,
}

impl CaptureManager {
  pub fn new() -> Self {
    Self {
      handle: Mutex::new(None),
      segments: Arc::new(Mutex::new(Vec::new())),
    }
  }

  pub fn start(&self, app: AppHandle) -> Result<(), String> {
    let mut guard = self.handle.lock().map_err(|_| "capture state poisoned".to_string())?;
    if let Some(existing) = guard.take() {
      if existing.handle.is_finished() {
        let _ = existing.handle.join();
      } else {
        *guard = Some(existing);
        return Err("capture already running".to_string());
      }
    }

    let segments_dir = ensure_segments_dir(&app)?;
    let config = load_config(&app);
    ensure_config_file(&app, &config);

    let segments = Arc::clone(&self.segments);
    load_index_if_needed(&segments_dir, &segments);

    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let app_handle = app.clone();

    let handle = std::thread::spawn(move || {
      if let Err(err) = run_capture(app_handle, segments_dir, segments, config, stop_flag) {
        eprintln!("loopback capture stopped: {err}");
      }
    });

    *guard = Some(CaptureHandle { stop, handle });
    Ok(())
  }

  pub fn stop(&self) -> Result<(), String> {
    let mut guard = self.handle.lock().map_err(|_| "capture state poisoned".to_string())?;
    let Some(handle) = guard.take() else {
      return Ok(());
    };
    handle.stop.store(true, Ordering::SeqCst);
    let _ = handle.handle.join();
    Ok(())
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
    self.stop()?;
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
    if let Some(webview) = app.get_webview("output") {
      let _ = webview.emit("segment_list_cleared", true);
    }
    let _ = app.emit("segment_list_cleared", true);
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
    let segments = Arc::clone(&self.segments);
    start_translation_task(app, segments_dir, segments, name, provider);
    Ok(())
  }
}

fn ensure_segments_dir(app: &AppHandle) -> Result<PathBuf, String> {
  let base = app
    .path()
    .app_data_dir()
    .map_err(|err| err.to_string())?;
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

fn save_index(dir: &Path, segments: &[SegmentInfo]) -> Result<(), String> {
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
) -> Result<(), String> {
  let mut capture = LoopbackCapture::new()?;
  let sample_rate = capture.sample_rate();
  let channels = capture.channels().max(1);

  let min_segment_frames = config.min_segment_ms.saturating_mul(sample_rate as u64) / 1000;
  let min_silence_frames = config.min_silence_ms.saturating_mul(sample_rate as u64) / 1000;
  let pre_roll_frames = config.pre_roll_ms.saturating_mul(sample_rate as u64) / 1000;
  let pre_roll_samples = pre_roll_frames.saturating_mul(channels as u64) as usize;

  let mut pre_roll: VecDeque<f32> = VecDeque::with_capacity(pre_roll_samples.max(1));
  let mut writer: Option<SegmentWriter> = None;
  let mut segment_frames: u64 = 0;
  let mut silent_frames: u64 = 0;

  let vad = SilenceDetector::new(config.silence_threshold_db);
  let mut speech_gate = SpeechGate::new(sample_rate)?;

  while !stop.load(Ordering::SeqCst) {
    let pcm = capture.read()?;
    if pcm.is_empty() {
      std::thread::sleep(Duration::from_millis(10));
      continue;
    }

    let frame_count = (pcm.len() / channels as usize) as u64;
    let is_silence = vad.is_silence(&pcm);

    for sample in pcm.iter().copied() {
      pre_roll.push_back(sample);
    }
    while pre_roll.len() > pre_roll_samples {
      pre_roll.pop_front();
    }

    let mut should_finalize = false;
    if let Some(active) = writer.as_mut() {
      active.write(&pcm)?;
      segment_frames = segment_frames.saturating_add(frame_count);
      if is_silence {
        silent_frames = silent_frames.saturating_add(frame_count);
      } else {
        silent_frames = 0;
      }

      if silent_frames >= min_silence_frames && segment_frames >= min_segment_frames {
        should_finalize = true;
      }
    } else if !is_silence {
      let mut next = SegmentWriter::start_new(&segments_dir, sample_rate, channels)?;
      if !pre_roll.is_empty() {
        let pre_roll_vec: Vec<f32> = pre_roll.iter().copied().collect();
        next.write(&pre_roll_vec)?;
        segment_frames = (pre_roll_vec.len() / channels as usize) as u64;
      }
      next.write(&pcm)?;
      segment_frames = segment_frames.saturating_add(frame_count);
      silent_frames = 0;
      writer = Some(next);
    }

    if should_finalize {
      if let Some(active) = writer.take() {
        let info = active.finalize()?;
        segment_frames = 0;
        silent_frames = 0;
        finalize_segment_with_vad(
          &app,
          &segments_dir,
          &segments,
          &mut speech_gate,
          info,
        );
      }
    }
  }

  if let Some(active) = writer {
    let info = active.finalize()?;
    finalize_segment_with_vad(
      &app,
      &segments_dir,
      &segments,
      &mut speech_gate,
      info,
    );
  }

  Ok(())
}

fn finalize_segment_with_vad(
  app: &AppHandle,
  dir: &Path,
  segments: &Arc<Mutex<Vec<SegmentInfo>>>,
  speech_gate: &mut SpeechGate,
  info: SegmentInfo,
) {
  let path = dir.join(&info.name);
  match speech_gate.should_keep(&path) {
    Ok(true) => {
      push_segment(app, dir, segments, info.clone());
      start_transcription_task(app.clone(), dir.to_path_buf(), Arc::clone(segments), info.name);
    }
    Ok(false) => {
      let _ = fs::remove_file(&path);
    }
    Err(err) => {
      eprintln!("vad check failed: {err}");
      push_segment(app, dir, segments, info.clone());
      start_transcription_task(app.clone(), dir.to_path_buf(), Arc::clone(segments), info.name);
    }
  }
}

fn start_transcription_task(
  app: AppHandle,
  dir: PathBuf,
  segments: Arc<Mutex<Vec<SegmentInfo>>>,
  name: String,
) {
  tauri::async_runtime::spawn(async move {
    let path = dir.join(&name);
    let started_at = Instant::now();
    let transcript = match transcribe_file(&path).await {
      Ok(text) => Some(text),
      Err(err) => {
        eprintln!("transcription failed for {name}: {err}");
        Some(String::new())
      }
    };
    let elapsed_ms = started_at.elapsed().as_millis() as u64;
    apply_transcript(&app, &dir, &segments, &name, transcript, elapsed_ms);
  });
}

fn apply_transcript(
  app: &AppHandle,
  dir: &Path,
  segments: &Arc<Mutex<Vec<SegmentInfo>>>,
  name: &str,
  transcript: Option<String>,
  elapsed_ms: u64,
) {
  let mut updated: Option<SegmentInfo> = None;
  if let Ok(mut guard) = segments.lock() {
    if let Some(segment) = guard.iter_mut().find(|segment| segment.name == name) {
      segment.transcript = transcript;
      segment.transcript_at = Some(Local::now().to_rfc3339());
      segment.transcript_ms = Some(elapsed_ms);
      updated = Some(segment.clone());
      let _ = save_index(dir, &guard);
    }
  }

  if let Some(info) = updated {
    if let Some(webview) = app.get_webview("output") {
      let _ = webview.emit("segment_transcribed", info.clone());
    }
    let _ = app.emit("segment_transcribed", info);
  }
}

fn start_translation_task(
  app: AppHandle,
  dir: PathBuf,
  segments: Arc<Mutex<Vec<SegmentInfo>>>,
  name: String,
  provider: Option<String>,
) {
  tauri::async_runtime::spawn(async move {
    let transcript = {
      let guard = segments.lock().ok();
      if let Some(segments) = guard.as_ref() {
        segments
          .iter()
          .find(|segment| segment.name == name)
          .and_then(|segment| segment.transcript.clone())
      } else {
        None
      }
    };
    let Some(transcript) = transcript else {
      return;
    };

    let started_at = Instant::now();
    let translation = match translate_text(&transcript, provider).await {
      Ok(text) => Some(text),
      Err(err) => {
        eprintln!("translation failed for {name}: {err}");
        Some(String::new())
      }
    };
    let elapsed_ms = started_at.elapsed().as_millis() as u64;
    apply_translation(&app, &dir, &segments, &name, translation, elapsed_ms);
  });
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
  if let Ok(mut guard) = segments.lock() {
    if let Some(segment) = guard.iter_mut().find(|segment| segment.name == name) {
      segment.translation = translation;
      segment.translation_at = Some(Local::now().to_rfc3339());
      segment.translation_ms = Some(elapsed_ms);
      updated = Some(segment.clone());
      let _ = save_index(dir, &guard);
    }
  }

  if let Some(info) = updated {
    if let Some(webview) = app.get_webview("output") {
      let _ = webview.emit("segment_translated", info.clone());
    }
    let _ = app.emit("segment_translated", info);
  }
}

fn push_segment(
  app: &AppHandle,
  dir: &Path,
  segments: &Arc<Mutex<Vec<SegmentInfo>>>,
  info: SegmentInfo,
) {
  if let Ok(mut guard) = segments.lock() {
    guard.push(info.clone());
    let _ = save_index(dir, &guard);
  }
  if let Some(webview) = app.get_webview("output") {
    let _ = webview.emit("segment_created", info.clone());
  }
  let _ = app.emit("segment_created", info);
}
