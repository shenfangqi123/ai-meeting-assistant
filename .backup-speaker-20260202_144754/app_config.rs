use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

const CONFIG_FILE: &str = "ai-interview.config";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiConfig {
  pub api_key: String,
  pub model: Option<String>,
  pub base_url: Option<String>,
  pub timeout_secs: Option<u64>,
  pub language: Option<String>,
  pub response_format: Option<String>,
  pub chat_model: Option<String>,
  pub chat_base_url: Option<String>,
  pub chat_timeout_secs: Option<u64>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaConfig {
  pub enabled: Option<bool>,
  pub model: Option<String>,
  pub base_url: Option<String>,
  pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
  pub openai: OpenAiConfig,
  #[allow(dead_code)]
  pub ollama: Option<OllamaConfig>,
  pub translate: Option<TranslateConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateConfig {
  pub enabled: Option<bool>,
  pub provider: Option<String>,
  pub target_language: Option<String>,
}

pub fn load_config() -> Result<AppConfig, String> {
  let path = find_config_path()?;
  let content = fs::read_to_string(&path)
    .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
  serde_json::from_str(&content)
    .map_err(|err| format!("invalid config {}: {err}", path.display()))
}

fn find_config_path() -> Result<PathBuf, String> {
  let candidates = config_candidates();
  for path in &candidates {
    if path.exists() {
      return Ok(path.clone());
    }
  }
  let attempted = candidates
    .iter()
    .map(|path| path.display().to_string())
    .collect::<Vec<_>>()
    .join(", ");
  Err(format!(
    "ai-interview.config not found. Tried: {attempted}"
  ))
}

fn config_candidates() -> Vec<PathBuf> {
  let mut candidates = Vec::new();

  if let Ok(path) = std::env::var("AI_INTERVIEW_CONFIG") {
    candidates.push(PathBuf::from(path));
  }

  if let Ok(cwd) = std::env::current_dir() {
    push_candidate(&mut candidates, cwd.join(CONFIG_FILE));
    if let Some(parent) = cwd.parent() {
      push_candidate(&mut candidates, parent.join(CONFIG_FILE));
    }
  }

  if let Ok(exe) = std::env::current_exe() {
    if let Some(dir) = exe.parent() {
      push_candidate(&mut candidates, dir.join(CONFIG_FILE));
      if let Some(parent) = dir.parent() {
        push_candidate(&mut candidates, parent.join(CONFIG_FILE));
      }
    }
  }

  candidates
}

fn push_candidate(candidates: &mut Vec<PathBuf>, path: PathBuf) {
  if !candidates.iter().any(|existing| same_path(existing, &path)) {
    candidates.push(path);
  }
}

fn same_path(left: &Path, right: &Path) -> bool {
  left == right
}
