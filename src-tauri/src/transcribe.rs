use crate::app_config::{load_config, AsrConfig, OpenAiConfig};
use crate::asr::AsrState;
use crate::whisper_server::WhisperServerManager;
use reqwest::multipart::{Form, Part};
use std::path::Path;
use std::time::Duration;
use tauri::{AppHandle, Manager};

const DEFAULT_MODEL: &str = "whisper-1";
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1/audio/transcriptions";
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const DEFAULT_RESPONSE_FORMAT: &str = "json";
const DEFAULT_WHISPER_SERVER_URL: &str = "http://127.0.0.1:8080/inference";
const DEFAULT_WHISPER_SERVER_RESPONSE_FORMAT: &str = "text";
const DEFAULT_WHISPER_SERVER_TEMPERATURE: &str = "0";

pub async fn transcribe_file(app: &AppHandle, path: &Path) -> Result<String, String> {
  let config = load_config()?;
  let mut openai = config.openai.clone();
  let mut asr_config = config.asr.unwrap_or_default();
  let asr_state = app.state::<AsrState>();
  let provider = asr_state.provider();
  let fallback = asr_state.fallback_to_openai();
  let language_override = asr_state.language();
  if !language_override.trim().is_empty() {
    asr_config.language = Some(language_override.clone());
    openai.language = Some(language_override);
  }

  match provider.as_str() {
    "whisperserver" => {
      let server_result = transcribe_with_whisper_server(app, path, &asr_config).await;
      match server_result {
        Ok(text) => return Ok(text),
        Err(err) => {
          if fallback {
            eprintln!("whisper-server failed, fallback to OpenAI: {err}");
          } else {
            return Err(err);
          }
        }
      }
    }
    "openai" => {}
    other => {
      if fallback {
        eprintln!("unknown ASR provider {other}, fallback to OpenAI");
      } else {
        return Err(format!("unsupported ASR provider: {other}"));
      }
    }
  }

  transcribe_with_openai(path, &openai).await
}

pub async fn transcribe_with_whisper_server(
  app: &AppHandle,
  path: &Path,
  config: &AsrConfig,
) -> Result<String, String> {
  let manual_url = config
    .whisper_server_url
    .clone()
    .filter(|value| !value.trim().is_empty())
    .filter(|value| value.trim() != DEFAULT_WHISPER_SERVER_URL);
  let url = if let Some(url) = manual_url {
    url
  } else {
    let manager = app
      .try_state::<WhisperServerManager>()
      .ok_or_else(|| "whisper-server manager not available".to_string())?;
    manager.ensure_started(app, config)?
  };
  let timeout_secs = config
    .whisper_server_timeout_secs
    .unwrap_or(DEFAULT_TIMEOUT_SECS);

  let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
  let file_name = path
    .file_name()
    .and_then(|value| value.to_str())
    .unwrap_or("segment.wav")
    .to_string();
  let part = Part::bytes(bytes)
    .file_name(file_name)
    .mime_str("audio/wav")
    .map_err(|err| err.to_string())?;

  let mut form = Form::new()
    .part("file", part)
    .text("temperature", DEFAULT_WHISPER_SERVER_TEMPERATURE.to_string())
    .text(
      "response_format",
      DEFAULT_WHISPER_SERVER_RESPONSE_FORMAT.to_string(),
    );
  if let Some(language) = config
    .language
    .clone()
    .filter(|value| !value.trim().is_empty())
  {
    form = form.text("language", language);
  }

  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(timeout_secs))
    .build()
    .map_err(|err| err.to_string())?;

  let response = client
    .post(url)
    .multipart(form)
    .send()
    .await
    .map_err(|err| err.to_string())?;

  let status = response.status();
  let text = response.text().await.map_err(|err| err.to_string())?;
  if !status.is_success() {
    return Err(text);
  }
  let trimmed = text.trim();
  if trimmed.is_empty() {
    return Err("whisper-server returned empty text".to_string());
  }
  Ok(trimmed.to_string())
}

async fn transcribe_with_openai(path: &Path, openai: &OpenAiConfig) -> Result<String, String> {
  let api_key = openai.api_key.trim();
  if api_key.is_empty() {
    return Err("OpenAI apiKey is required".to_string());
  }

  let model = openai
    .model
    .clone()
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| DEFAULT_MODEL.to_string());
  let base_url = openai
    .base_url
    .clone()
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
  let url = normalize_transcriptions_url(&base_url);
  let timeout_secs = openai.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
  let response_format = openai
    .response_format
    .clone()
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| DEFAULT_RESPONSE_FORMAT.to_string());

  let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
  let file_name = path
    .file_name()
    .and_then(|value| value.to_str())
    .unwrap_or("segment.wav")
    .to_string();
  let part = Part::bytes(bytes)
    .file_name(file_name)
    .mime_str("audio/wav")
    .map_err(|err| err.to_string())?;

  let mut form = Form::new().part("file", part).text("model", model);
  if !response_format.is_empty() {
    form = form.text("response_format", response_format.clone());
  }
  if let Some(language) = openai
    .language
    .clone()
    .filter(|value| !value.trim().is_empty())
  {
    form = form.text("language", language);
  }

  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(timeout_secs))
    .build()
    .map_err(|err| err.to_string())?;

  let response = client
    .post(url)
    .bearer_auth(api_key)
    .multipart(form)
    .send()
    .await
    .map_err(|err| err.to_string())?;

  let status = response.status();
  if response_format == "text" {
    let text = response.text().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
      return Err(text);
    }
    return Ok(text.trim().to_string());
  }

  let value: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
  if !status.is_success() {
    return Err(value.to_string());
  }
  let text = value
    .get("text")
    .and_then(|field| field.as_str())
    .unwrap_or("")
    .trim();
  if text.is_empty() {
    return Err("transcription returned empty text".to_string());
  }
  Ok(text.to_string())
}

fn normalize_transcriptions_url(raw: &str) -> String {
  let trimmed = raw.trim().trim_end_matches('/');
  if trimmed.ends_with("/audio/transcriptions") {
    return trimmed.to_string();
  }
  if trimmed.ends_with("/v1/responses") {
    return trimmed.replace("/v1/responses", "/v1/audio/transcriptions");
  }
  if trimmed.ends_with("/v1") {
    return format!("{trimmed}/audio/transcriptions");
  }
  if trimmed.contains("/v1/") {
    return trimmed.to_string();
  }
  format!("{trimmed}/v1/audio/transcriptions")
}
