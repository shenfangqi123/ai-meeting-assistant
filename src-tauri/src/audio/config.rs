use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
  pub silence_threshold_db: f32,
  pub min_segment_ms: u64,
  pub min_silence_ms: u64,
  pub max_segment_ms: u64,
  pub min_transcribe_ms: u64,
  pub pre_roll_ms: u64,
  pub sample_rate: u32,
  pub channels: u16,
  pub rolling_enabled: bool,
  pub window_transcribe_enabled: bool,
  pub rolling_window_ms: u64,
  pub rolling_step_ms: u64,
  pub rolling_min_ms: u64,
}

impl Default for AudioConfig {
  fn default() -> Self {
    Self {
      silence_threshold_db: -30.0,
      min_segment_ms: 800,
      min_silence_ms: 300,
      max_segment_ms: 10000,
      min_transcribe_ms: 500,
      pre_roll_ms: 200,
      sample_rate: 48000,
      channels: 2,
      rolling_enabled: true,
      window_transcribe_enabled: true,
      rolling_window_ms: 8000,
      rolling_step_ms: 500,
      rolling_min_ms: 1500,
    }
  }
}

fn dev_config_path() -> Option<PathBuf> {
  let cwd = std::env::current_dir().ok()?;
  let mut candidates = Vec::new();
  candidates.push(cwd.join("src-tauri").join("config").join("audio.json"));
  candidates.push(cwd.join("config").join("audio.json"));
  if let Some(parent) = cwd.parent() {
    candidates.push(parent.join("src-tauri").join("config").join("audio.json"));
    candidates.push(parent.join("config").join("audio.json"));
  }
  candidates.into_iter().find(|path| path.exists())
}

fn app_config_path(app: &AppHandle) -> Option<PathBuf> {
  let dir = app.path().app_data_dir().ok()?;
  Some(dir.join("audio.json"))
}

fn read_config(path: &Path) -> Option<AudioConfig> {
  let content = fs::read_to_string(path).ok()?;
  serde_json::from_str(&content).ok()
}

fn write_default(path: &Path, config: &AudioConfig) {
  if let Some(parent) = path.parent() {
    let _ = fs::create_dir_all(parent);
  }
  if let Ok(content) = serde_json::to_string_pretty(config) {
    let _ = fs::write(path, content);
  }
}

pub fn load_config(app: &AppHandle) -> AudioConfig {
  if let Some(path) = app_config_path(app) {
    if let Some(config) = read_config(&path) {
      return config;
    }
  }

  if let Some(path) = dev_config_path() {
    if let Some(config) = read_config(&path) {
      return config;
    }
  }

  AudioConfig::default()
}

pub fn ensure_config_file(app: &AppHandle, config: &AudioConfig) {
  let Some(path) = app_config_path(app) else {
    return;
  };
  if !path.exists() {
    write_default(&path, config);
  }
}
