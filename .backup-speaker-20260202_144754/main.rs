#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_config;
mod audio;
mod transcribe;
mod translate;

use audio::{CaptureManager, SegmentInfo};
use serde::Deserialize;
use std::sync::Mutex;
use tauri::webview::WebviewBuilder;
use tauri::{
  AppHandle, LogicalPosition, LogicalSize, Manager, State, Webview, WebviewUrl,
  WebviewWindowBuilder, Window, WindowEvent,
};

const CONTENT_LABEL: &str = "content";
const OUTPUT_LABEL: &str = "output";
const DIVIDER_LABEL: &str = "divider";
const DEFAULT_URL: &str = "https://zoom.us/signin";
const OUTPUT_URL: &str = "blank.html";
const DIVIDER_URL: &str = "divider.html";
const DIVIDER_WIDTH: f64 = 12.0;
const TOP_RATIO: f64 = 0.33;
const MIN_TOP_HEIGHT: f64 = 190.0;
const MAX_TOP_HEIGHT: f64 = 190.0;
const MIN_BOTTOM_HEIGHT: f64 = 100.0;
const MIN_BOTTOM_WIDTH: f64 = 100.0;

#[derive(Debug, Deserialize)]
struct LlmRequest {
  provider: String,
  base_url: Option<String>,
  api_key: Option<String>,
  model: String,
  prompt: String,
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
  content: &Webview,
  output: &Webview,
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

  content
    .set_position(LogicalPosition::new(0.0, layout.top_height))
    .map_err(|err| err.to_string())?;
  content
    .set_size(LogicalSize::new(layout.left_width, layout.bottom_height))
    .map_err(|err| err.to_string())?;

  output
    .set_position(LogicalPosition::new(layout.left_width, layout.top_height))
    .map_err(|err| err.to_string())?;
  output
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

fn create_content_webview(window: &Window) -> Result<Webview, String> {
  let layout = compute_layout(window, None, None)?;
  let url = url::Url::parse(DEFAULT_URL).map_err(|err| err.to_string())?;
  let builder = WebviewBuilder::new(CONTENT_LABEL, WebviewUrl::External(url));

  window
    .add_child(
      builder,
      LogicalPosition::new(0.0, layout.top_height),
      LogicalSize::new(layout.left_width, layout.bottom_height),
    )
    .map_err(|err| err.to_string())
}

fn create_output_webview(window: &Window) -> Result<Webview, String> {
  let layout = compute_layout(window, None, None)?;
  let builder = WebviewBuilder::new(OUTPUT_LABEL, WebviewUrl::App(OUTPUT_URL.into()));

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

#[tauri::command]
async fn content_navigate(app: AppHandle, url: String) -> Result<(), String> {
  let parsed_url = url::Url::parse(&url).map_err(|err| err.to_string())?;

  let content = if let Some(webview) = app.get_webview(CONTENT_LABEL) {
    webview
  } else {
    let window = app
      .get_window("main")
      .ok_or_else(|| "main window not found".to_string())?;
    create_content_webview(&window)?
  };

  content
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
  let content = app
    .get_webview(CONTENT_LABEL)
    .ok_or_else(|| "content webview not found".to_string())?;
  let output = app
    .get_webview(OUTPUT_LABEL)
    .ok_or_else(|| "output webview not found".to_string())?;
  let divider = app
    .get_webview(DIVIDER_LABEL)
    .ok_or_else(|| "divider webview not found".to_string())?;

  let ratio = read_ratio_override(&state);
  let layout = apply_layout(&window, &content, &output, &divider, Some(height), ratio)?;
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
  let content = app
    .get_webview(CONTENT_LABEL)
    .ok_or_else(|| "content webview not found".to_string())?;
  let output = app
    .get_webview(OUTPUT_LABEL)
    .ok_or_else(|| "output webview not found".to_string())?;
  let divider = app
    .get_webview(DIVIDER_LABEL)
    .ok_or_else(|| "divider webview not found".to_string())?;

  let top = read_top_override(&state);
  let layout = apply_layout(&window, &content, &output, &divider, top, Some(ratio))?;
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

fn main() {
  tauri::Builder::default()
    .manage(LayoutState {
      top_height: Mutex::new(None),
      bottom_ratio: Mutex::new(None),
    })
    .manage(CaptureManager::new())
    .setup(|app| {
      let window = app
        .get_window("main")
        .ok_or_else(|| to_boxed_error("main window not found".to_string()))?;
      let state = app.state::<LayoutState>();

      if app.get_webview(CONTENT_LABEL).is_none() {
        let _content = create_content_webview(&window).map_err(to_boxed_error)?;
      }
      if app.get_webview(OUTPUT_LABEL).is_none() {
        let _output = create_output_webview(&window).map_err(to_boxed_error)?;
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
          let Some(content) = app_handle.get_webview(CONTENT_LABEL) else {
            return;
          };
          let Some(output) = app_handle.get_webview(OUTPUT_LABEL) else {
            return;
          };
          let Some(divider) = app_handle.get_webview(DIVIDER_LABEL) else {
            return;
          };
          let state = app_handle.state::<LayoutState>();
          let override_top = read_top_override(&state);
          let override_ratio = read_ratio_override(&state);
          if let Err(err) = apply_layout(&window, &content, &output, &divider, override_top, override_ratio) {
            eprintln!("layout error: {err}");
          }
        }
      });

      let content = app.get_webview(CONTENT_LABEL).unwrap();
      let output = app.get_webview(OUTPUT_LABEL).unwrap();
      let divider = app.get_webview(DIVIDER_LABEL).unwrap();
      let override_top = read_top_override(&state);
      let override_ratio = read_ratio_override(&state);
      apply_layout(&window, &content, &output, &divider, override_top, override_ratio)
        .map_err(to_boxed_error)?;

      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
      llm_generate,
      open_external_window,
      content_navigate,
      set_top_height,
      set_bottom_split,
      start_loopback_capture,
      stop_loopback_capture,
      list_segments,
      read_segment_bytes,
      clear_segments,
      translate_segment
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
