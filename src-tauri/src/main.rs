#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_config;
mod asr;
mod audio;
mod rag;
mod transcribe;
mod translate;
mod whisper_server;

use audio::{CaptureManager, SegmentInfo};
use app_config::{load_config, OllamaConfig, TranslateConfig};
use asr::AsrState;
use chrono::Local;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::webview::WebviewBuilder;
use tauri::{
  AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State, Webview, WebviewUrl,
  WebviewWindowBuilder, Window, WindowEvent,
};
use whisper_server::WhisperServerManager;
use rag::{rag_index_add_files, rag_index_remove_files, rag_index_sync_project, rag_search, RagState};

const OUTPUT_LABEL: &str = "output";
const RIGHT_LABEL: &str = "right";
const DIVIDER_LABEL: &str = "divider";
const OUTPUT_URL: &str = "blank.html";
const RIGHT_URL: &str = "empty.html";
const DIVIDER_URL: &str = "divider.html";
const INTRO_URL: &str = "intro.html";
const DIVIDER_WIDTH: f64 = 12.0;
const TOP_RATIO: f64 = 0.33;
const MIN_TOP_HEIGHT: f64 = 190.0;
const MAX_TOP_HEIGHT: f64 = 190.0;
const MIN_BOTTOM_HEIGHT: f64 = 100.0;
const MIN_BOTTOM_WIDTH: f64 = 100.0;
const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434";
const DEFAULT_OLLAMA_TIMEOUT: u64 = 600;
const DEFAULT_OLLAMA_MODEL: &str = "gpt-oss:20b";
const DEFAULT_OPENAI_CHAT_MODEL: &str = "gpt-4.1-mini";
const DEFAULT_OPENAI_CHAT_BASE_URL: &str = "https://api.openai.com/v1/responses";
const DEFAULT_OPENAI_CHAT_TIMEOUT: u64 = 120;

#[derive(Debug, Deserialize)]
struct LlmRequest {
  provider: String,
  base_url: Option<String>,
  api_key: Option<String>,
  model: String,
  prompt: String,
}

#[derive(Debug, Serialize, Clone)]
struct LiveTranslationStart {
  id: String,
  order: u64,
  source: String,
  provider: String,
  target: String,
  created_at: String,
}

#[derive(Debug, Serialize, Clone)]
struct LiveTranslationChunk {
  id: String,
  order: u64,
  chunk: String,
}

#[derive(Debug, Serialize, Clone)]
struct LiveTranslationDone {
  id: String,
  order: u64,
  translation: String,
  elapsed_ms: u64,
}

#[derive(Debug, Serialize, Clone)]
struct LiveTranslationError {
  id: String,
  order: u64,
  error: String,
}

struct LayoutState {
  top_height: Mutex<Option<f64>>,
  bottom_ratio: Mutex<Option<f64>>,
}

struct Layout {
  width: f64,
  top_height: f64,
  bottom_height: f64,
  left_width: f64,
  right_width: f64,
  divider_x: f64,
  divider_width: f64,
}

fn compute_layout(
  window: &Window,
  override_top: Option<f64>,
  override_ratio: Option<f64>,
) -> Result<Layout, String> {
  let size = window.inner_size().map_err(|err| err.to_string())?;
  let scale = window.scale_factor().map_err(|err| err.to_string())?;
  let width = size.width as f64 / scale;
  let height = size.height as f64 / scale;

  let target = (height * TOP_RATIO).round();
  let max_allowed = (height - MIN_BOTTOM_HEIGHT).max(120.0);
  let override_top = override_top.filter(|value| value.is_finite() && *value > 0.0);
  let mut top_height = override_top.unwrap_or(target);
  top_height = top_height.max(MIN_TOP_HEIGHT);
  top_height = top_height.min(MAX_TOP_HEIGHT);
  top_height = top_height.min(max_allowed);
  let bottom_height = (height - top_height).max(120.0);

  let ratio = override_ratio.unwrap_or(0.5).clamp(0.0, 1.0);
  let mut min_ratio = MIN_BOTTOM_WIDTH / width;
  let mut max_ratio = 1.0 - MIN_BOTTOM_WIDTH / width;
  if min_ratio > max_ratio {
    min_ratio = 0.5;
    max_ratio = 0.5;
  }
  let ratio = ratio.clamp(min_ratio, max_ratio);
  let left_width = (width * ratio).round();
  let right_width = (width - left_width).max(0.0);

  let divider_width = DIVIDER_WIDTH.min(width);
  let mut divider_x = left_width - divider_width / 2.0;
  if width <= divider_width {
    divider_x = 0.0;
  } else {
    divider_x = divider_x.clamp(0.0, width - divider_width);
  }

  Ok(Layout {
    width,
    top_height,
    bottom_height,
    left_width,
    right_width,
    divider_x,
    divider_width,
  })
}

fn main_webview(window: &Window) -> Result<Webview, String> {
  window
    .webviews()
    .into_iter()
    .find(|webview| webview.label() == window.label())
    .ok_or_else(|| "main webview not found".to_string())
}

fn read_top_override(state: &LayoutState) -> Option<f64> {
  match state.top_height.lock() {
    Ok(guard) => *guard,
    Err(_) => None,
  }
}

fn read_ratio_override(state: &LayoutState) -> Option<f64> {
  match state.bottom_ratio.lock() {
    Ok(guard) => *guard,
    Err(_) => None,
  }
}

fn apply_layout(
  window: &Window,
  output: &Webview,
  right: &Webview,
  divider: &Webview,
  override_top: Option<f64>,
  override_ratio: Option<f64>,
) -> Result<Layout, String> {
  let layout = compute_layout(window, override_top, override_ratio)?;
  let main = main_webview(window)?;

  main
    .set_position(LogicalPosition::new(0.0, 0.0))
    .map_err(|err| err.to_string())?;
  main
    .set_size(LogicalSize::new(layout.width, layout.top_height))
    .map_err(|err| err.to_string())?;

  output
    .set_position(LogicalPosition::new(0.0, layout.top_height))
    .map_err(|err| err.to_string())?;
  output
    .set_size(LogicalSize::new(layout.left_width, layout.bottom_height))
    .map_err(|err| err.to_string())?;

  right
    .set_position(LogicalPosition::new(layout.left_width, layout.top_height))
    .map_err(|err| err.to_string())?;
  right
    .set_size(LogicalSize::new(layout.right_width, layout.bottom_height))
    .map_err(|err| err.to_string())?;

  divider
    .set_position(LogicalPosition::new(layout.divider_x, layout.top_height))
    .map_err(|err| err.to_string())?;
  divider
    .set_size(LogicalSize::new(layout.divider_width, layout.bottom_height))
    .map_err(|err| err.to_string())?;

  Ok(layout)
}

fn create_output_webview(window: &Window) -> Result<Webview, String> {
  let layout = compute_layout(window, None, None)?;
  let builder = WebviewBuilder::new(OUTPUT_LABEL, WebviewUrl::App(OUTPUT_URL.into()));

  window
    .add_child(
      builder,
      LogicalPosition::new(0.0, layout.top_height),
      LogicalSize::new(layout.left_width, layout.bottom_height),
    )
    .map_err(|err| err.to_string())
}

fn create_right_webview(window: &Window) -> Result<Webview, String> {
  let layout = compute_layout(window, None, None)?;
  let builder = WebviewBuilder::new(RIGHT_LABEL, WebviewUrl::App(RIGHT_URL.into()));

  window
    .add_child(
      builder,
      LogicalPosition::new(layout.left_width, layout.top_height),
      LogicalSize::new(layout.right_width, layout.bottom_height),
    )
    .map_err(|err| err.to_string())
}

fn create_divider_webview(window: &Window) -> Result<Webview, String> {
  let layout = compute_layout(window, None, None)?;
  let builder = WebviewBuilder::new(DIVIDER_LABEL, WebviewUrl::App(DIVIDER_URL.into()));

  window
    .add_child(
      builder,
      LogicalPosition::new(layout.divider_x, layout.top_height),
      LogicalSize::new(layout.divider_width, layout.bottom_height),
    )
    .map_err(|err| err.to_string())
}

fn to_boxed_error(message: String) -> Box<dyn std::error::Error> {
  Box::new(std::io::Error::new(std::io::ErrorKind::Other, message))
}

fn emit_right<T: Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
  if let Some(webview) = app.get_webview(RIGHT_LABEL) {
    let _ = webview.emit(event, payload);
  }
}

fn resolve_translate_settings(
  provider_override: Option<String>,
) -> Result<(String, String, app_config::AppConfig), String> {
  let config = load_config()?;
  let translate_config = config.translate.clone().unwrap_or(TranslateConfig {
    enabled: Some(true),
    provider: Some("ollama".to_string()),
    target_language: Some("zh".to_string()),
  });

  if translate_config.enabled == Some(false) {
    return Err("translation disabled".to_string());
  }

  let provider = provider_override
    .filter(|value| !value.trim().is_empty())
    .or(translate_config.provider)
    .unwrap_or_else(|| "ollama".to_string())
    .to_lowercase();
  let target_language = translate_config
    .target_language
    .unwrap_or_else(|| "zh".to_string());

  Ok((provider, target_language, config))
}

#[tauri::command]
async fn content_navigate(app: AppHandle, url: String) -> Result<(), String> {
  let parsed_url = url::Url::parse(&url).map_err(|err| err.to_string())?;

  let right = if let Some(webview) = app.get_webview(RIGHT_LABEL) {
    webview
  } else {
    let window = app
      .get_window("main")
      .ok_or_else(|| "main window not found".to_string())?;
    create_right_webview(&window)?
  };

  right
    .navigate(parsed_url)
    .map_err(|err| err.to_string())
}

#[tauri::command]
async fn set_top_height(
  app: AppHandle,
  state: State<'_, LayoutState>,
  height: f64,
) -> Result<(), String> {
  let window = app
    .get_window("main")
    .ok_or_else(|| "main window not found".to_string())?;
  let output = app
    .get_webview(OUTPUT_LABEL)
    .ok_or_else(|| "output webview not found".to_string())?;
  let right = app
    .get_webview(RIGHT_LABEL)
    .ok_or_else(|| "right webview not found".to_string())?;
  let divider = app
    .get_webview(DIVIDER_LABEL)
    .ok_or_else(|| "divider webview not found".to_string())?;

  let ratio = read_ratio_override(&state);
  let layout = apply_layout(&window, &output, &right, &divider, Some(height), ratio)?;
  if let Ok(mut guard) = state.top_height.lock() {
    *guard = Some(layout.top_height);
  }
  Ok(())
}

#[tauri::command]
async fn set_bottom_split(
  app: AppHandle,
  state: State<'_, LayoutState>,
  ratio: f64,
) -> Result<(), String> {
  let window = app
    .get_window("main")
    .ok_or_else(|| "main window not found".to_string())?;
  let output = app
    .get_webview(OUTPUT_LABEL)
    .ok_or_else(|| "output webview not found".to_string())?;
  let right = app
    .get_webview(RIGHT_LABEL)
    .ok_or_else(|| "right webview not found".to_string())?;
  let divider = app
    .get_webview(DIVIDER_LABEL)
    .ok_or_else(|| "divider webview not found".to_string())?;

  let top = read_top_override(&state);
  let layout = apply_layout(&window, &output, &right, &divider, top, Some(ratio))?;
  if let Ok(mut guard) = state.bottom_ratio.lock() {
    *guard = Some(layout.left_width / layout.width);
  }
  Ok(())
}

#[tauri::command]
async fn llm_generate(request: LlmRequest) -> Result<String, String> {
  let provider = request.provider.to_lowercase();
  match provider.as_str() {
    "openai" => call_openai(request).await,
    "ollama" => call_ollama(request).await,
    _ => Err(format!("unknown provider: {}", provider)),
  }
}

#[tauri::command]
async fn translate_live(
  app: AppHandle,
  text: String,
  provider: Option<String>,
  name: Option<String>,
  order: Option<u64>,
) -> Result<(), String> {
  let source = text.trim().to_string();
  if source.is_empty() {
    return Ok(());
  }

  let (provider, target, config) = resolve_translate_settings(provider)?;
  let order = order.unwrap_or_else(|| Local::now().timestamp_millis().max(0) as u64);
  eprintln!(
    "translate_live start provider={} text={}",
    provider,
    source.chars().take(60).collect::<String>()
  );
  let id = name
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| format!("live-{}", Local::now().timestamp_millis()));
  let created_at = Local::now().to_rfc3339();

  emit_right(
    &app,
    "live_translation_start",
    LiveTranslationStart {
      id: id.clone(),
      order,
      source: source.clone(),
      provider: provider.clone(),
      target: target.clone(),
      created_at,
    },
  );

  let started_at = Instant::now();
  let result = if provider == "ollama" {
    stream_translate_with_ollama(&app, &id, order, &source, &target, &config).await
  } else if provider == "openai" || provider == "chatgpt" {
    stream_translate_with_openai(&app, &id, order, &source, &target, &config).await
  } else {
    translate::translate_text(&source, Some(provider.clone())).await
  };

  match result {
    Ok(translation) => {
      emit_right(
        &app,
        "live_translation_done",
        LiveTranslationDone {
          id,
          order,
          translation,
          elapsed_ms: started_at.elapsed().as_millis() as u64,
        },
      );
      Ok(())
    }
    Err(err) => {
      emit_right(
        &app,
        "live_translation_error",
        LiveTranslationError { id, order, error: err.clone() },
      );
      Err(err)
    }
  }
}

async fn stream_translate_with_ollama(
  app: &AppHandle,
  id: &str,
  order: u64,
  text: &str,
  target_language: &str,
  config: &app_config::AppConfig,
) -> Result<String, String> {
  let ollama = config.ollama.clone().unwrap_or_else(|| OllamaConfig {
    enabled: Some(true),
    model: Some(DEFAULT_OLLAMA_MODEL.to_string()),
    base_url: Some(DEFAULT_OLLAMA_BASE_URL.to_string()),
    timeout_secs: Some(DEFAULT_OLLAMA_TIMEOUT),
  });

  if ollama.enabled == Some(false) {
    return Err("ollama disabled".to_string());
  }

  let model = ollama
    .model
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| DEFAULT_OLLAMA_MODEL.to_string());
  let base_url = ollama
    .base_url
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| DEFAULT_OLLAMA_BASE_URL.to_string());
  let timeout_secs = ollama.timeout_secs.unwrap_or(DEFAULT_OLLAMA_TIMEOUT);
  let url = format!("{}/api/generate", base_url.trim_end_matches('/'));
  eprintln!(
    "ollama stream request url={} model={} target={} chars={}",
    url,
    model,
    target_language,
    text.len()
  );

  let prompt = format!(
    "Translate the following text to {target_language}. Output only the translated text.\n\n{text}"
  );
  let body = serde_json::json!({
    "model": model,
    "prompt": prompt,
    "stream": true
  });

  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(timeout_secs))
    .build()
    .map_err(|err| err.to_string())?;
  let response = client
    .post(url)
    .json(&body)
    .send()
    .await
    .map_err(|err| err.to_string())?;

  let status = response.status();
  if !status.is_success() {
    let value: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
    return Err(value.to_string());
  }

  let mut stream = response.bytes_stream();
  let mut buffer = String::new();
  let mut full = String::new();
  let mut raw = String::new();
  let mut done = false;

  while let Some(chunk) = stream.next().await {
    let chunk = match chunk {
      Ok(value) => value,
      Err(err) => return Err(err.to_string()),
    };
    let text = String::from_utf8_lossy(&chunk);
    raw.push_str(&text);
    buffer.push_str(&text);

    loop {
      let Some(pos) = buffer.find('\n') else { break };
      let line = buffer[..pos].trim().to_string();
      buffer = buffer[pos + 1..].to_string();
      if line.is_empty() {
        continue;
      }
      let value: serde_json::Value = match serde_json::from_str(&line) {
        Ok(value) => value,
        Err(err) => {
          eprintln!("ollama stream parse error: {err}");
          continue;
        }
      };
      if let Some(response_text) = value.get("response").and_then(|v| v.as_str()) {
        if !response_text.is_empty() {
          full.push_str(response_text);
          emit_right(
            app,
            "live_translation_chunk",
            LiveTranslationChunk {
              id: id.to_string(),
              order,
              chunk: response_text.to_string(),
            },
          );
        }
      }
      if value.get("done").and_then(|v| v.as_bool()) == Some(true) {
        done = true;
        break;
      }
    }

    if done {
      break;
    }
  }

  if !done {
    let line = buffer.trim();
    if !line.is_empty() {
      if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
        if let Some(response_text) = value.get("response").and_then(|v| v.as_str()) {
          if !response_text.is_empty() {
            full.push_str(response_text);
            emit_right(
              app,
              "live_translation_chunk",
              LiveTranslationChunk {
                id: id.to_string(),
                order,
                chunk: response_text.to_string(),
              },
            );
          }
        }
      }
    }
  }

  if full.trim().is_empty() && !raw.is_empty() {
    eprintln!("ollama stream raw (first 1000 chars): {}", raw.chars().take(1000).collect::<String>());
    let mut recovered = String::new();
    for line in raw.lines() {
      let line = line.trim();
      if line.is_empty() {
        continue;
      }
      if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
        if let Some(response_text) = value.get("response").and_then(|v| v.as_str()) {
          if !response_text.is_empty() {
            recovered.push_str(response_text);
          }
        }
      }
    }
    if !recovered.trim().is_empty() {
      full = recovered;
    }
  }

  Ok(full.trim().to_string())
}

async fn stream_translate_with_openai(
  app: &AppHandle,
  id: &str,
  order: u64,
  text: &str,
  target_language: &str,
  config: &app_config::AppConfig,
) -> Result<String, String> {
  let openai = &config.openai;
  let api_key = openai.api_key.trim();
  if api_key.is_empty() {
    return Err("OpenAI apiKey is required".to_string());
  }

  let model = openai
    .chat_model
    .clone()
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| DEFAULT_OPENAI_CHAT_MODEL.to_string());
  let base_url = openai
    .chat_base_url
    .clone()
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| DEFAULT_OPENAI_CHAT_BASE_URL.to_string());
  let timeout_secs = openai.chat_timeout_secs.unwrap_or(DEFAULT_OPENAI_CHAT_TIMEOUT);

  let prompt = format!(
    "Translate the following text to {target_language}. Output only the translated text."
  );
  let body = serde_json::json!({
    "model": model,
    "input": [
      {
        "role": "system",
        "content": [{"type": "input_text", "text": prompt}]
      },
      {
        "role": "user",
        "content": [{"type": "input_text", "text": text}]
      }
    ],
    "temperature": 0.2,
    "stream": true
  });

  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(timeout_secs))
    .build()
    .map_err(|err| err.to_string())?;
  let response = client
    .post(base_url.trim_end_matches('/'))
    .bearer_auth(api_key)
    .json(&body)
    .send()
    .await
    .map_err(|err| err.to_string())?;

  let status = response.status();
  if !status.is_success() {
    let value: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
    return Err(value.to_string());
  }

  let mut stream = response.bytes_stream();
  let mut buffer = String::new();
  let mut full = String::new();
  let mut done = false;

  while let Some(chunk) = stream.next().await {
    let chunk = match chunk {
      Ok(value) => value,
      Err(err) => return Err(err.to_string()),
    };
    let text = String::from_utf8_lossy(&chunk);
    buffer.push_str(&text);

    loop {
      let Some(pos) = buffer.find('\n') else { break };
      let line = buffer[..pos].trim().to_string();
      buffer = buffer[pos + 1..].to_string();
      if line.is_empty() {
        continue;
      }
      if !line.starts_with("data:") {
        continue;
      }
      let payload = line.trim_start_matches("data:").trim();
      if payload == "[DONE]" {
        done = true;
        break;
      }
      let value: serde_json::Value = match serde_json::from_str(payload) {
        Ok(value) => value,
        Err(err) => {
          eprintln!("openai stream parse error: {err}");
          continue;
        }
      };

      if value
        .get("type")
        .and_then(|v| v.as_str())
        .is_some_and(|t| t == "response.completed")
      {
        done = true;
      }

      let delta = value
        .get("delta")
        .and_then(|v| v.as_str())
        .or_else(|| {
          value
            .pointer("/choices/0/delta/content")
            .and_then(|v| v.as_str())
        });
      if let Some(chunk_text) = delta {
        if !chunk_text.is_empty() {
          full.push_str(chunk_text);
          emit_right(
            app,
            "live_translation_chunk",
            LiveTranslationChunk {
              id: id.to_string(),
              order,
              chunk: chunk_text.to_string(),
            },
          );
        }
      }

      if done {
        break;
      }
    }

    if done {
      break;
    }
  }

  Ok(full.trim().to_string())
}

async fn call_openai(request: LlmRequest) -> Result<String, String> {
  let base_url = request
    .base_url
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| "https://api.openai.com".to_string());
  let api_key = request
    .api_key
    .filter(|value| !value.trim().is_empty())
    .ok_or_else(|| "OpenAI api_key is required".to_string())?;

  let url = format!("{}/v1/chat/completions", base_url.trim_end_matches('/'));
  let body = serde_json::json!({
    "model": request.model,
    "messages": [{"role": "user", "content": request.prompt}],
    "temperature": 0.2
  });

  let client = reqwest::Client::new();
  let response = client
    .post(url)
    .bearer_auth(api_key)
    .json(&body)
    .send()
    .await
    .map_err(|err| err.to_string())?;

  let status = response.status();
  let value: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
  if !status.is_success() {
    return Err(value.to_string());
  }

  value
    .get("choices")
    .and_then(|choices| choices.get(0))
    .and_then(|choice| choice.get("message"))
    .and_then(|message| message.get("content"))
    .and_then(|content| content.as_str())
    .map(|text| text.to_string())
    .ok_or_else(|| "OpenAI response missing content".to_string())
}

async fn call_ollama(request: LlmRequest) -> Result<String, String> {
  let base_url = request
    .base_url
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| "http://localhost:11434".to_string());

  let url = format!("{}/api/generate", base_url.trim_end_matches('/'));
  let body = serde_json::json!({
    "model": request.model,
    "prompt": request.prompt,
    "stream": false
  });

  let client = reqwest::Client::new();
  let response = client
    .post(url)
    .json(&body)
    .send()
    .await
    .map_err(|err| err.to_string())?;

  let status = response.status();
  let value: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
  if !status.is_success() {
    return Err(value.to_string());
  }

  value
    .get("response")
    .and_then(|response| response.as_str())
    .map(|text| text.to_string())
    .ok_or_else(|| "Ollama response missing content".to_string())
}

#[tauri::command]
async fn start_loopback_capture(
  app: AppHandle,
  state: State<'_, CaptureManager>,
) -> Result<(), String> {
  state.start(app)
}

#[tauri::command]
async fn stop_loopback_capture(state: State<'_, CaptureManager>) -> Result<(), String> {
  state.stop()
}

#[tauri::command]
async fn list_segments(
  app: AppHandle,
  state: State<'_, CaptureManager>,
) -> Result<Vec<SegmentInfo>, String> {
  state.list(app)
}

#[tauri::command]
async fn read_segment_bytes(
  app: AppHandle,
  state: State<'_, CaptureManager>,
  name: String,
) -> Result<Vec<u8>, String> {
  state.read_segment_bytes(app, name)
}

#[tauri::command]
async fn clear_segments(app: AppHandle, state: State<'_, CaptureManager>) -> Result<(), String> {
  state.clear(app)
}

#[tauri::command]
async fn translate_segment(
  app: AppHandle,
  state: State<'_, CaptureManager>,
  name: String,
  provider: Option<String>,
) -> Result<(), String> {
  state.translate_segment(app, name, provider)
}

#[tauri::command]
async fn open_external_window(app: AppHandle, label: String, url: String) -> Result<(), String> {
  let parsed_url = url::Url::parse(&url).map_err(|err| err.to_string())?;
  WebviewWindowBuilder::new(&app, label, WebviewUrl::External(parsed_url))
    .title("Meeting")
    .build()
    .map_err(|err| err.to_string())?;
  Ok(())
}

#[tauri::command]
fn open_intro_window(app: AppHandle) -> Result<(), String> {
  if let Some(window) = app.get_window("intro") {
    let _ = window.set_closable(true);
    let _ = window.set_minimizable(true);
    let _ = window.set_decorations(true);
    let _ = window.show();
    let _ = window.set_focus();
    return Ok(());
  }

  WebviewWindowBuilder::new(&app, "intro", WebviewUrl::App(INTRO_URL.into()))
    .title("自己紹介")
    .inner_size(520.0, 520.0)
    .closable(true)
    .minimizable(true)
    .decorations(true)
    .resizable(true)
    .build()
    .map_err(|err| err.to_string())?;
  Ok(())
}
#[tauri::command]
fn get_asr_settings(state: State<'_, AsrState>) -> (String, bool, String) {
  (state.provider(), state.fallback_to_openai(), state.language())
}

#[tauri::command]
fn set_asr_provider(state: State<'_, AsrState>, provider: String) -> Result<String, String> {
  Ok(state.set_provider(provider))
}

#[tauri::command]
fn set_asr_fallback(state: State<'_, AsrState>, fallback: bool) -> Result<bool, String> {
  Ok(state.set_fallback_to_openai(fallback))
}

#[tauri::command]
fn set_asr_language(state: State<'_, AsrState>, language: String) -> Result<String, String> {
  Ok(state.set_language(language))
}

#[tauri::command]
fn log_live_line(index: u64, line: String) {
  println!("[live {index}] {line}");
}

#[tauri::command]
fn emit_live_draft(app: AppHandle, text: String) {
  emit_right(&app, "live_draft_update", text);
}

fn main() {
  let asr_state = AsrState::new();
  tauri::Builder::default()
    .manage(LayoutState {
      top_height: Mutex::new(None),
      bottom_ratio: Mutex::new(None),
    })
    .manage(CaptureManager::new())
    .manage(WhisperServerManager::new())
    .manage(asr_state)
    .manage(Arc::new(RagState::new()))
    .setup(|app| {
      let asr_config = load_config().ok().and_then(|cfg| cfg.asr).unwrap_or_default();
      if should_start_whisper_server(&asr_config) {
        let app_handle = app.handle().clone();
        std::thread::spawn(move || {
          if let Some(manager) = app_handle.try_state::<WhisperServerManager>() {
            if let Err(err) = manager.ensure_started(&app_handle, &asr_config) {
              eprintln!("whisper-server start failed: {err}");
            }
          }
        });
      }

      let window = app
        .get_window("main")
        .ok_or_else(|| to_boxed_error("main window not found".to_string()))?;
      let state = app.state::<LayoutState>();

      if app.get_webview(OUTPUT_LABEL).is_none() {
        let _output = create_output_webview(&window).map_err(to_boxed_error)?;
      }
      if app.get_webview(RIGHT_LABEL).is_none() {
        let _right = create_right_webview(&window).map_err(to_boxed_error)?;
      }
      if app.get_webview(DIVIDER_LABEL).is_none() {
        let _divider = create_divider_webview(&window).map_err(to_boxed_error)?;
      }
      let app_handle = app.handle().clone();
      let window_label = window.label().to_string();
      window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Resized(_)) {
          let Some(window) = app_handle.get_window(&window_label) else {
            return;
          };
          let Some(output) = app_handle.get_webview(OUTPUT_LABEL) else {
            return;
          };
          let Some(right) = app_handle.get_webview(RIGHT_LABEL) else {
            return;
          };
          let Some(divider) = app_handle.get_webview(DIVIDER_LABEL) else {
            return;
          };
          let state = app_handle.state::<LayoutState>();
          let override_top = read_top_override(&state);
          let override_ratio = read_ratio_override(&state);
          if let Err(err) = apply_layout(&window, &output, &right, &divider, override_top, override_ratio) {
            eprintln!("layout error: {err}");
          }
        }
      });

      let output = app.get_webview(OUTPUT_LABEL).unwrap();
      let right = app.get_webview(RIGHT_LABEL).unwrap();
      let divider = app.get_webview(DIVIDER_LABEL).unwrap();
      let override_top = read_top_override(&state);
      let override_ratio = read_ratio_override(&state);
      apply_layout(&window, &output, &right, &divider, override_top, override_ratio)
        .map_err(to_boxed_error)?;

      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
      llm_generate,
      translate_live,
      open_external_window,
      open_intro_window,
      content_navigate,
      set_top_height,
      set_bottom_split,
      start_loopback_capture,
      stop_loopback_capture,
      list_segments,
      read_segment_bytes,
      clear_segments,
      translate_segment,
      get_asr_settings,
      set_asr_provider,
      set_asr_fallback,
      set_asr_language,
      log_live_line,
      emit_live_draft,
      rag_index_add_files,
      rag_index_sync_project,
      rag_index_remove_files,
      rag_search
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}

fn should_start_whisper_server(config: &app_config::AsrConfig) -> bool {
  let provider = config
    .provider
    .clone()
    .unwrap_or_else(|| "whisperserver".to_string())
    .to_lowercase();
  matches!(
    provider.as_str(),
    "whisperserver" | "whisper-server" | "whisper_server" | "server"
  )
}
