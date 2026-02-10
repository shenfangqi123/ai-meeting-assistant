use crate::app_config::{load_config, AppConfig, TranslateConfig};
use reqwest::Client;
use serde_json::json;
use std::collections::HashMap;
use std::time::Duration;

const DEFAULT_OPENAI_CHAT_MODEL: &str = "gpt-4.1-mini";
const DEFAULT_OPENAI_CHAT_BASE_URL: &str = "https://api.openai.com/v1/responses";
const DEFAULT_OPENAI_CHAT_TIMEOUT: u64 = 120;
const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434";
const DEFAULT_OLLAMA_TIMEOUT: u64 = 600;

#[derive(Debug, Clone)]
pub struct BatchTranslationItem {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct BatchTranslationResult {
    pub translation: String,
    pub cleaned_source: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct BatchTranslationOptions {
    pub context_items: Vec<BatchTranslationItem>,
}

#[derive(Debug, Clone, Copy)]
pub enum TranslateSource {
    Segment,
    Live,
}

impl TranslateSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Segment => "segment",
            Self::Live => "live",
        }
    }
}

fn log_translate_request(
    source: TranslateSource,
    provider: &str,
    mode: &str,
    endpoint: &str,
    model: &str,
    target: &str,
    items: usize,
    chars: usize,
) {
    eprintln!(
    "[translate-request] source={} provider={} mode={} model={} endpoint={} target={} items={} chars={}",
    source.as_str(),
    provider,
    mode,
    model,
    endpoint,
    target,
    items,
    chars
  );
}

pub async fn translate_text(
    text: &str,
    provider_override: Option<String>,
    source: TranslateSource,
) -> Result<String, String> {
    let config = load_config()?;
    let (provider, target_language) = resolve_translate_settings(&config, provider_override)?;

    match provider.as_str() {
        "openai" | "chatgpt" => {
            translate_with_openai(text, &target_language, &config, source).await
        }
        "ollama" => translate_with_ollama(text, &target_language, &config, source).await,
        other => Err(format!("unsupported translate provider: {other}")),
    }
}

#[allow(dead_code)]
pub async fn translate_text_batch(
    items: &[BatchTranslationItem],
    provider_override: Option<String>,
    source: TranslateSource,
) -> Result<HashMap<String, String>, String> {
    let detailed = translate_text_batch_with_options(
        items,
        provider_override,
        source,
        BatchTranslationOptions::default(),
    )
    .await?;

    if detailed.is_empty() {
        return Err("batch translation response is empty".to_string());
    }

    let mut translations = HashMap::new();
    for (id, result) in detailed {
        translations.insert(id, result.translation);
    }

    Ok(translations)
}

pub async fn translate_text_batch_with_options(
    items: &[BatchTranslationItem],
    provider_override: Option<String>,
    source: TranslateSource,
    options: BatchTranslationOptions,
) -> Result<HashMap<String, BatchTranslationResult>, String> {
    if items.is_empty() {
        return Ok(HashMap::new());
    }

    let config = load_config()?;
    let (provider, target_language) = resolve_translate_settings(&config, provider_override)?;

    let translations = match provider.as_str() {
        "openai" | "chatgpt" => {
            translate_batch_with_openai(items, &target_language, &config, source, &options).await?
        }
        "ollama" => {
            translate_batch_with_ollama(items, &target_language, &config, source, &options).await?
        }
        other => return Err(format!("unsupported translate provider: {other}")),
    };

    if translations.is_empty() {
        return Err("batch translation response is empty".to_string());
    }

    Ok(translations)
}

async fn translate_with_openai(
    text: &str,
    target_language: &str,
    config: &crate::app_config::AppConfig,
    source: TranslateSource,
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
    let timeout_secs = openai
        .chat_timeout_secs
        .unwrap_or(DEFAULT_OPENAI_CHAT_TIMEOUT);

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
    let endpoint = base_url.trim_end_matches('/').to_string();
    log_translate_request(
        source,
        "openai",
        "single",
        endpoint.as_str(),
        model.as_str(),
        target_language,
        1,
        text.chars().count(),
    );

    let response = match client
        .post(endpoint.as_str())
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => return Err(err.to_string()),
    };

    let status = response.status();
    let value: serde_json::Value = match response.json().await {
        Ok(value) => value,
        Err(err) => return Err(err.to_string()),
    };
    if !status.is_success() {
        return Err(value.to_string());
    }

    extract_response_text(&value).ok_or_else(|| "OpenAI response missing text".to_string())
}

async fn translate_with_ollama(
    text: &str,
    target_language: &str,
    config: &crate::app_config::AppConfig,
    source: TranslateSource,
) -> Result<String, String> {
    let ollama = config
        .ollama
        .clone()
        .unwrap_or_else(|| crate::app_config::OllamaConfig {
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

    log_translate_request(
        source,
        "ollama",
        "single",
        url.as_str(),
        model.as_str(),
        target_language,
        1,
        text.chars().count(),
    );

    let response = match client.post(url.as_str()).json(&body).send().await {
        Ok(response) => response,
        Err(err) => return Err(err.to_string()),
    };

    let status = response.status();
    let value: serde_json::Value = match response.json().await {
        Ok(value) => value,
        Err(err) => return Err(err.to_string()),
    };
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

fn resolve_translate_settings(
    config: &AppConfig,
    provider_override: Option<String>,
) -> Result<(String, String), String> {
    let translate_config = config.translate.clone().unwrap_or(TranslateConfig {
        enabled: Some(true),
        provider: Some("ollama".to_string()),
        target_language: Some("zh".to_string()),
        segment_batch_size: None,
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

    Ok((provider, target_language))
}

async fn translate_batch_with_openai(
    items: &[BatchTranslationItem],
    target_language: &str,
    config: &AppConfig,
    source: TranslateSource,
    options: &BatchTranslationOptions,
) -> Result<HashMap<String, BatchTranslationResult>, String> {
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
    let timeout_secs = openai
        .chat_timeout_secs
        .unwrap_or(DEFAULT_OPENAI_CHAT_TIMEOUT);

    let payload = build_batch_payload(items, &options.context_items)?;

    let prompt = format!(
    "You rewrite noisy ASR text and translate it.\n\
For each item in `items`:\n\
1) rewrite `text` into readable text in the same language as input and return as `cleaned_source`;\n\
2) translate `cleaned_source` to {target_language} and return as `translation`.\n\
Use `context` only as previous conversation context.\n\
Return ONLY JSON array.\n\
Each element must be {{\"id\": string, \"cleaned_source\": string, \"translation\": string}}.\n\
Return exactly one element for every id in `items`."
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
          "content": [{"type": "input_text", "text": payload}]
        }
      ],
      "temperature": 0.1
    });

    let client = Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(|err| err.to_string())?;

    let endpoint = base_url.trim_end_matches('/').to_string();
    let batch_chars: usize = items.iter().map(|item| item.text.chars().count()).sum();
    log_translate_request(
        source,
        "openai",
        "batch",
        endpoint.as_str(),
        model.as_str(),
        target_language,
        items.len(),
        batch_chars,
    );

    let response = match client
        .post(endpoint.as_str())
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => return Err(err.to_string()),
    };

    let status = response.status();
    let value: serde_json::Value = match response.json().await {
        Ok(value) => value,
        Err(err) => return Err(err.to_string()),
    };
    if !status.is_success() {
        return Err(value.to_string());
    }

    let text = extract_response_text(&value)
        .ok_or_else(|| "OpenAI batch response missing text".to_string())?;
    parse_batch_translation_json(&text)
}

async fn translate_batch_with_ollama(
    items: &[BatchTranslationItem],
    target_language: &str,
    config: &AppConfig,
    source: TranslateSource,
    options: &BatchTranslationOptions,
) -> Result<HashMap<String, BatchTranslationResult>, String> {
    let ollama = config
        .ollama
        .clone()
        .unwrap_or_else(|| crate::app_config::OllamaConfig {
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

    let payload = build_batch_payload(items, &options.context_items)?;

    let prompt = format!(
    "You rewrite noisy ASR text and translate it.\n\
For each item in `items`:\n\
1) rewrite `text` into readable text in the same language as input and return as `cleaned_source`;\n\
2) translate `cleaned_source` to {target_language} and return as `translation`.\n\
Use `context` only as previous conversation context.\n\
Return ONLY JSON array.\n\
Each element must be {{\"id\": string, \"cleaned_source\": string, \"translation\": string}}.\n\
Return exactly one element for every id in `items`.\n\n{payload}"
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

    let batch_chars: usize = items.iter().map(|item| item.text.chars().count()).sum();
    log_translate_request(
        source,
        "ollama",
        "batch",
        url.as_str(),
        model.as_str(),
        target_language,
        items.len(),
        batch_chars,
    );

    let response = match client.post(url.as_str()).json(&body).send().await {
        Ok(response) => response,
        Err(err) => return Err(err.to_string()),
    };

    let status = response.status();
    let value: serde_json::Value = match response.json().await {
        Ok(value) => value,
        Err(err) => return Err(err.to_string()),
    };
    if !status.is_success() {
        return Err(value.to_string());
    }

    let text = value
        .get("response")
        .and_then(|response| response.as_str())
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
        .ok_or_else(|| "ollama batch response missing text".to_string())?;
    parse_batch_translation_json(&text)
}

fn build_batch_payload(
    items: &[BatchTranslationItem],
    context_items: &[BatchTranslationItem],
) -> Result<String, String> {
    let payload_items = items
        .iter()
        .map(|item| {
            json!({
              "id": item.id.as_str(),
              "text": item.text.as_str()
            })
        })
        .collect::<Vec<_>>();
    let payload_context = context_items
        .iter()
        .map(|item| {
            json!({
              "id": item.id.as_str(),
              "text": item.text.as_str()
            })
        })
        .collect::<Vec<_>>();

    serde_json::to_string(&json!({
      "context": payload_context,
      "items": payload_items
    }))
    .map_err(|err| err.to_string())
}

fn parse_batch_translation_json(
    raw: &str,
) -> Result<HashMap<String, BatchTranslationResult>, String> {
    let raw = raw.trim();
    let mut candidates = Vec::new();
    if !raw.is_empty() {
        candidates.push(raw.to_string());
    }

    let without_code_fence = strip_code_fence(raw);
    if without_code_fence != raw {
        candidates.push(without_code_fence.to_string());
    }

    if let (Some(start), Some(end)) = (raw.find('['), raw.rfind(']')) {
        if start < end {
            candidates.push(raw[start..=end].to_string());
        }
    }
    if let (Some(start), Some(end)) = (raw.find('{'), raw.rfind('}')) {
        if start < end {
            candidates.push(raw[start..=end].to_string());
        }
    }

    for candidate in candidates {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&candidate) {
            let parsed = parse_batch_translation_value(&value);
            if !parsed.is_empty() {
                return Ok(parsed);
            }
        }
    }

    Err("failed to parse batch translation JSON".to_string())
}

fn strip_code_fence(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let lines: Vec<&str> = trimmed.lines().collect();
    if lines.len() < 3 {
        return trimmed.to_string();
    }
    let body = lines[1..lines.len() - 1].join("\n");
    body.trim().to_string()
}

fn parse_batch_translation_value(
    value: &serde_json::Value,
) -> HashMap<String, BatchTranslationResult> {
    let mut map = HashMap::new();

    if let Some(array) = value.as_array() {
        collect_batch_items(array, &mut map);
        return map;
    }

    if let Some(items) = value.get("items").and_then(|field| field.as_array()) {
        collect_batch_items(items, &mut map);
        return map;
    }

    if let Some(object) = value.as_object() {
        if object.contains_key("id") || object.contains_key("name") {
            collect_batch_item(value, &mut map);
        }
    }

    map
}

fn collect_batch_items(
    array: &[serde_json::Value],
    map: &mut HashMap<String, BatchTranslationResult>,
) {
    for item in array {
        collect_batch_item(item, map);
    }
}

fn collect_batch_item(item: &serde_json::Value, map: &mut HashMap<String, BatchTranslationResult>) {
    let id = item
        .get("id")
        .and_then(|field| field.as_str())
        .or_else(|| item.get("name").and_then(|field| field.as_str()))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let translation = item
        .get("translation")
        .and_then(|field| field.as_str())
        .or_else(|| item.get("text").and_then(|field| field.as_str()))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let cleaned_source = item
        .get("cleaned_source")
        .and_then(|field| field.as_str())
        .or_else(|| item.get("cleaned").and_then(|field| field.as_str()))
        .or_else(|| item.get("source").and_then(|field| field.as_str()))
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    if let (Some(id), Some(translation)) = (id, translation) {
        let id = id.to_string();
        map.insert(
            id,
            BatchTranslationResult {
                translation: translation.to_string(),
                cleaned_source,
            },
        );
    }
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
