use crate::app_config::{load_config, TranslateConfig};
use reqwest::Client;
use serde_json::json;
use std::time::Duration;

const DEFAULT_OPENAI_CHAT_MODEL: &str = "gpt-4.1-mini";
const DEFAULT_OPENAI_CHAT_BASE_URL: &str = "https://api.openai.com/v1/responses";
const DEFAULT_OPENAI_CHAT_TIMEOUT: u64 = 120;
const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434";
const DEFAULT_OLLAMA_TIMEOUT: u64 = 600;

pub async fn translate_text(
  text: &str,
  provider_override: Option<String>,
) -> Result<String, String> {
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

  match provider.as_str() {
    "openai" | "chatgpt" => translate_with_openai(text, &target_language, &config).await,
    "ollama" => translate_with_ollama(text, &target_language, &config).await,
    other => Err(format!("unsupported translate provider: {other}")),
  }
}

async fn translate_with_openai(
  text: &str,
  target_language: &str,
  config: &crate::app_config::AppConfig,
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

  let client = Client::builder()
    .timeout(Duration::from_secs(timeout_secs))
    .build()
    .map_err(|err| err.to_string())?;

  let prompt = format!(
    "Translate the following text to {target_language}. Output only the translated text."
  );
  let body = json!({
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
    "temperature": 0.2
  });

  let response = client
    .post(base_url.trim_end_matches('/'))
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

  extract_response_text(&value).ok_or_else(|| "OpenAI response missing text".to_string())
}

async fn translate_with_ollama(
  text: &str,
  target_language: &str,
  config: &crate::app_config::AppConfig,
) -> Result<String, String> {
  let ollama = config.ollama.clone().unwrap_or_else(|| crate::app_config::OllamaConfig {
    enabled: Some(true),
    model: Some("gpt-oss:20b".to_string()),
    base_url: Some(DEFAULT_OLLAMA_BASE_URL.to_string()),
    timeout_secs: Some(DEFAULT_OLLAMA_TIMEOUT),
  });

  if ollama.enabled == Some(false) {
    return Err("ollama disabled".to_string());
  }

  let model = ollama
    .model
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| "gpt-oss:20b".to_string());
  let base_url = ollama
    .base_url
    .filter(|value| !value.trim().is_empty())
    .unwrap_or_else(|| DEFAULT_OLLAMA_BASE_URL.to_string());
  let timeout_secs = ollama.timeout_secs.unwrap_or(DEFAULT_OLLAMA_TIMEOUT);
  let url = format!("{}/api/generate", base_url.trim_end_matches('/'));

  let prompt = format!(
    "Translate the following text to {target_language}. Output only the translated text.\n\n{text}"
  );
  let body = json!({
    "model": model,
    "prompt": prompt,
    "stream": false
  });

  let client = Client::builder()
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
  let value: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
  if !status.is_success() {
    return Err(value.to_string());
  }

  value
    .get("response")
    .and_then(|response| response.as_str())
    .map(|text| text.trim().to_string())
    .filter(|text| !text.is_empty())
    .ok_or_else(|| "ollama response missing text".to_string())
}

fn extract_response_text(value: &serde_json::Value) -> Option<String> {
  if let Some(text) = value.get("output_text").and_then(|field| field.as_str()) {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
      return Some(trimmed.to_string());
    }
  }
  if let Some(output) = value.get("output").and_then(|field| field.as_array()) {
    for item in output {
      if let Some(content) = item.get("content").and_then(|field| field.as_array()) {
        for part in content {
          if part.get("type").and_then(|t| t.as_str()) == Some("output_text") {
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
              let trimmed = text.trim();
              if !trimmed.is_empty() {
                return Some(trimmed.to_string());
              }
            }
          }
        }
      }
    }
  }
  None
}
