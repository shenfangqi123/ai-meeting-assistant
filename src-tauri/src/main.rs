#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_config;
mod asr;
mod audio;
mod egui_app;
mod ui_events;
mod rag;
mod transcribe;
mod translate;
mod whisper_server;

use app_config::{load_config, LocalGptConfig, OllamaConfig, TranslateConfig};
use asr::AsrState;
use audio::{CaptureManager, SegmentInfo};
use chrono::Local;
use futures_util::StreamExt;
use rag::{
    rag_index_add_files, rag_index_remove_files, rag_index_sync_project, rag_pick_folder,
    rag_project_create, rag_project_delete, rag_project_list, rag_search, RagState,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};
use whisper_server::WhisperServerManager;

const INTRO_URL: &str = "intro.html";
const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434";
const DEFAULT_OLLAMA_TIMEOUT: u64 = 600;
const DEFAULT_OLLAMA_MODEL: &str = "gpt-oss:20b";
const DEFAULT_OPENAI_CHAT_MODEL: &str = "gpt-4.1-mini";
const DEFAULT_OPENAI_CHAT_BASE_URL: &str = "https://api.openai.com/v1/responses";
const DEFAULT_OPENAI_CHAT_TIMEOUT: u64 = 120;
const DEFAULT_LOCAL_GPT_BASE_URL: &str = "http://127.0.0.1:8787";
const DEFAULT_LOCAL_GPT_TIMEOUT: u64 = 240;
const DEFAULT_LOCAL_GPT_DIRECT_PATH: &str = "/local-gpt-sse/direct";
const DEFAULT_LOCAL_GPT_PROJECT_ID: &str = "g-p-698c11cf2bc08191b07e28128883fcbb-testapi";
const DEFAULT_LIVE_PROMPT: &str =
    "Translate the following text to {target_language}. Output only the translated text.";
const ENABLE_EGUI_UI: bool = true;

#[derive(Debug, Deserialize)]
struct LlmRequest {
    provider: String,
    base_url: Option<String>,
    api_key: Option<String>,
    model: String,
    prompt: String,
}

#[derive(Debug, Deserialize)]
struct RagAskRequest {
    query: String,
    project_ids: Vec<String>,
    top_k: Option<usize>,
    allow_out_of_context: Option<bool>,
}

#[derive(Debug, Serialize)]
struct RagAnswerReference {
    index: usize,
    score: f32,
    file_path: String,
    chunk_id: String,
    snippet: String,
}

#[derive(Debug, Serialize)]
struct RagAnswerResponse {
    provider: String,
    answer: String,
    references: Vec<RagAnswerReference>,
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

struct TranslateProviderState {
    provider: Mutex<String>,
}

fn emit_output<T: Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    let _ = app;
    ui_events::emit(event, payload);
}

fn resolve_live_prompt_template(config: &app_config::AppConfig) -> String {
    config
        .translate
        .as_ref()
        .and_then(|translate| translate.live_prompt.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_LIVE_PROMPT.to_string())
}

fn render_prompt_template(template: &str, target_language: &str, text: Option<&str>) -> String {
    let mut rendered = template.replace("{target_language}", target_language);
    if let Some(text) = text {
        rendered = rendered.replace("{text}", text);
    }
    rendered
}

fn resolve_translate_settings(
    provider_override: Option<String>,
) -> Result<(String, String, app_config::AppConfig), String> {
    let config = load_config()?;
    let translate_config = config.translate.clone().unwrap_or(TranslateConfig {
        enabled: Some(true),
        provider: Some("ollama".to_string()),
        target_language: Some("zh".to_string()),
        segment_batch_size: None,
        segment_single_prompt: None,
        segment_batch_prompt: None,
        live_prompt: None,
    });

    if translate_config.enabled == Some(false) {
        return Err("translation disabled".to_string());
    }

    let provider = provider_override
        .filter(|value| !value.trim().is_empty())
        .or(translate_config.provider)
        .unwrap_or_else(|| "ollama".to_string());
    let provider = normalize_translate_provider(&provider);
    let target_language = translate_config
        .target_language
        .unwrap_or_else(|| "zh".to_string());

    Ok((provider, target_language, config))
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
async fn rag_ask_with_provider(
    app: AppHandle,
    rag_state: State<'_, Arc<RagState>>,
    provider_state: State<'_, TranslateProviderState>,
    request: RagAskRequest,
) -> Result<RagAnswerResponse, String> {
    let provider = provider_state
        .provider
        .lock()
        .map(|value| normalize_translate_provider(&value))
        .unwrap_or_else(|_| "ollama".to_string());
    rag_ask_with_provider_inner(app, rag_state.inner().clone(), provider, request).await
}

async fn rag_ask_with_provider_inner(
    app: AppHandle,
    rag_state: Arc<RagState>,
    provider: String,
    request: RagAskRequest,
) -> Result<RagAnswerResponse, String> {
    let query = request.query.trim().to_string();
    if query.is_empty() {
        return Err("query is empty".to_string());
    }
    if request.project_ids.is_empty() {
        return Err("project_ids is empty".to_string());
    }
    let top_k = request.top_k.unwrap_or(8).clamp(1, 20);
    let allow_out_of_context = request.allow_out_of_context.unwrap_or(false);

    let app_handle = app.clone();
    let search_query = query.clone();
    let project_ids = request.project_ids;
    let hits = tauri::async_runtime::spawn_blocking(move || {
        rag_state.with_service(&app_handle, |service| {
            service.search(&search_query, project_ids, top_k)
        })
    })
    .await
    .map_err(|err| err.to_string())??;

    let context = if hits.is_empty() {
        "No relevant context found in local project index.".to_string()
    } else {
        hits.iter()
            .enumerate()
            .map(|(index, hit)| {
                format!(
                    "[{index}] score={score:.4} file={file_path} chunk={chunk_id}\n{text}",
                    index = index + 1,
                    score = hit.score,
                    file_path = hit.file_path,
                    chunk_id = hit.chunk_id,
                    text = hit.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    let prompt = if allow_out_of_context {
        format!(
            "你是项目代码/文档问答助手。请优先使用给定上下文回答问题。\n\
若上下文不足，你可以补充通用知识完成回答，但要明确标注“以下内容超出检索上下文”。\n\
若引用上下文结论，请在句尾用 [n] 标注来源编号。\n\n\
问题:\n{query}\n\n\
上下文:\n{context}"
        )
    } else {
        format!(
            "你是项目代码/文档问答助手。请仅基于给定上下文回答问题。\n\
如果上下文不足，请明确说“根据当前检索结果无法确定”。\n\
回答要简洁，并在关键结论后用 [n] 标注来源编号。\n\n\
问题:\n{query}\n\n\
上下文:\n{context}"
        )
    };

    let config = load_config()?;
    let answer = generate_with_selected_provider(&provider, &prompt, &config).await?;
    let references = hits
        .iter()
        .enumerate()
        .map(|(index, hit)| RagAnswerReference {
            index: index + 1,
            score: hit.score,
            file_path: hit.file_path.clone(),
            chunk_id: hit.chunk_id.clone(),
            snippet: compact_text(&hit.text, 240),
        })
        .collect();

    Ok(RagAnswerResponse {
        provider,
        answer,
        references,
    })
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

    emit_output(
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
        translate::translate_text(
            &source,
            Some(provider.clone()),
            translate::TranslateSource::Live,
        )
        .await
    };

    match result {
        Ok(translation) => {
            emit_output(
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
            emit_output(
                &app,
                "live_translation_error",
                LiveTranslationError {
                    id,
                    order,
                    error: err.clone(),
                },
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

    let prompt_template = resolve_live_prompt_template(config);
    let prompt_uses_text = prompt_template.contains("{text}");
    let prompt = render_prompt_template(&prompt_template, target_language, Some(text));
    let prompt = if prompt_uses_text {
        prompt
    } else {
        format!("{prompt}\n\n{text}")
    };
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
                    emit_output(
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
                        emit_output(
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
        eprintln!(
            "ollama stream raw (first 1000 chars): {}",
            raw.chars().take(1000).collect::<String>()
        );
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
    let timeout_secs = openai
        .chat_timeout_secs
        .unwrap_or(DEFAULT_OPENAI_CHAT_TIMEOUT);

    let prompt_template = resolve_live_prompt_template(config);
    let prompt_uses_text = prompt_template.contains("{text}");
    let prompt = render_prompt_template(&prompt_template, target_language, Some(text));
    let mut input = vec![serde_json::json!({
        "role": "system",
        "content": [{"type": "input_text", "text": prompt}]
    })];
    if !prompt_uses_text {
        input.push(serde_json::json!({
            "role": "user",
            "content": [{"type": "input_text", "text": text}]
        }));
    }
    let body = serde_json::json!({
      "model": model,
      "input": input,
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

            let delta = value.get("delta").and_then(|v| v.as_str()).or_else(|| {
                value
                    .pointer("/choices/0/delta/content")
                    .and_then(|v| v.as_str())
            });
            if let Some(chunk_text) = delta {
                if !chunk_text.is_empty() {
                    full.push_str(chunk_text);
                    emit_output(
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

fn compact_text(input: &str, max_chars: usize) -> String {
    let compact = input.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut output = compact.chars().take(max_chars).collect::<String>();
    if compact.chars().count() > max_chars {
        output.push_str("...");
    }
    output
}

async fn generate_with_selected_provider(
    provider: &str,
    prompt: &str,
    config: &app_config::AppConfig,
) -> Result<String, String> {
    match provider {
        "openai" => generate_with_openai(prompt, config).await,
        "local-gpt" => generate_with_local_gpt(prompt, config).await,
        _ => generate_with_ollama(prompt, config).await,
    }
}

async fn generate_with_openai(
    prompt: &str,
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
    let timeout_secs = openai
        .chat_timeout_secs
        .unwrap_or(DEFAULT_OPENAI_CHAT_TIMEOUT);

    let body = serde_json::json!({
      "model": model,
      "input": [
        {
          "role": "system",
          "content": [{"type": "input_text", "text": "Answer using provided context and cite sources as [n]."}]
        },
        {
          "role": "user",
          "content": [{"type": "input_text", "text": prompt}]
        }
      ],
      "temperature": 0.2
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
    let value: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(value.to_string());
    }

    extract_openai_response_text(&value).ok_or_else(|| "OpenAI response missing text".to_string())
}

fn extract_openai_response_text(value: &serde_json::Value) -> Option<String> {
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

async fn generate_with_local_gpt(
    prompt: &str,
    config: &app_config::AppConfig,
) -> Result<String, String> {
    let local_gpt = config.local_gpt.clone().unwrap_or_else(|| LocalGptConfig {
        enabled: Some(true),
        base_url: Some(DEFAULT_LOCAL_GPT_BASE_URL.to_string()),
        timeout_secs: Some(DEFAULT_LOCAL_GPT_TIMEOUT),
        project_id: None,
    });

    if local_gpt.enabled == Some(false) {
        eprintln!(
            "[local-gpt-direct] config localGpt.enabled=false, but proceeding because local-gpt provider is selected"
        );
    }

    let base_url = local_gpt
        .base_url
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_LOCAL_GPT_BASE_URL.to_string());
    let timeout_secs = local_gpt.timeout_secs.unwrap_or(DEFAULT_LOCAL_GPT_TIMEOUT);
    let project_id = local_gpt
        .project_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_LOCAL_GPT_PROJECT_ID.to_string());
    let url = format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        DEFAULT_LOCAL_GPT_DIRECT_PATH.trim_start_matches('/')
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(|err| err.to_string())?;
    let response = client
        .post(url)
        .json(&serde_json::json!({
          "project_id": project_id.as_str(),
          "project-id": project_id.as_str(),
          "prompt": prompt
        }))
        .send()
        .await
        .map_err(|err| err.to_string())?;

    let status = response.status();
    let raw = response.text().await.map_err(|err| err.to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({ "message": raw }));

    let message = value
        .get("message")
        .and_then(|field| field.as_str())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| value.to_string());
    let timed_out = value
        .get("timed_out")
        .and_then(|field| field.as_bool())
        .unwrap_or(false);
    let result = value
        .get("result")
        .and_then(|field| field.as_str())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());

    if status.is_success() && value.get("ok").and_then(|field| field.as_bool()) != Some(false) {
        return result.ok_or_else(|| "local-gpt response missing result".to_string());
    }

    if timed_out {
        if let Some(partial) = result {
            eprintln!(
                "local-gpt rag prompt timed out, returning partial result chars={}",
                partial.chars().count()
            );
            return Ok(partial);
        }
    }

    Err(message)
}

async fn generate_with_ollama(
    prompt: &str,
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

    let body = serde_json::json!({
      "model": model,
      "prompt": prompt,
      "stream": false
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
    let value: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(value.to_string());
    }

    value
        .get("response")
        .and_then(|field| field.as_str())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
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
async fn stop_loopback_capture(
    app: AppHandle,
    state: State<'_, CaptureManager>,
    drop_translations: Option<bool>,
) -> Result<(), String> {
    state.stop(&app, drop_translations.unwrap_or(false))
}

#[tauri::command]
fn is_translation_busy(state: State<'_, CaptureManager>) -> bool {
    state.is_translation_busy()
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
    (
        state.provider(),
        state.fallback_to_openai(),
        state.language(),
    )
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
fn get_translate_provider(state: State<'_, TranslateProviderState>) -> String {
    state
        .provider
        .lock()
        .map(|provider| provider.clone())
        .unwrap_or_else(|_| "ollama".to_string())
}

#[tauri::command]
fn set_translate_provider(
    state: State<'_, TranslateProviderState>,
    provider: String,
) -> Result<String, String> {
    let normalized = normalize_translate_provider(&provider);
    let mut guard = state
        .provider
        .lock()
        .map_err(|_| "translate provider state poisoned".to_string())?;
    *guard = normalized.clone();
    Ok(normalized)
}

#[tauri::command]
fn log_live_line(index: u64, line: String) {
    println!("[live {index}] {line}");
}

#[tauri::command]
fn emit_live_draft(app: AppHandle, text: String) {
    emit_output(&app, "live_draft_update", text);
}

fn main() {
    let asr_state = AsrState::new();
    let initial_translate_provider = load_config()
        .ok()
        .and_then(|cfg| cfg.translate.and_then(|translate| translate.provider))
        .unwrap_or_else(|| "ollama".to_string());
    tauri::Builder::default()
        .manage(TranslateProviderState {
            provider: Mutex::new(normalize_translate_provider(&initial_translate_provider)),
        })
        .manage(CaptureManager::new())
        .manage(WhisperServerManager::new())
        .manage(asr_state)
        .manage(Arc::new(RagState::new()))
        .setup(|app| {
            let asr_config = load_config()
                .ok()
                .and_then(|cfg| cfg.asr)
                .unwrap_or_default();
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
                .ok_or_else(|| "main window not found".to_string())?;

            if ENABLE_EGUI_UI {
                let app_handle = app.handle().clone();
                let _ = window.hide();
                std::thread::spawn(move || {
                    if let Err(err) = egui_app::run(app_handle.clone()) {
                        eprintln!("egui ui failed: {err}");
                    }
                    app_handle.exit(0);
                });
                return Ok(());
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            llm_generate,
            translate_live,
            open_intro_window,
            start_loopback_capture,
            stop_loopback_capture,
            is_translation_busy,
            list_segments,
            read_segment_bytes,
            clear_segments,
            translate_segment,
            get_asr_settings,
            set_asr_provider,
            set_asr_fallback,
            set_asr_language,
            get_translate_provider,
            set_translate_provider,
            log_live_line,
            emit_live_draft,
            rag_ask_with_provider,
            rag_index_add_files,
            rag_index_sync_project,
            rag_index_remove_files,
            rag_search,
            rag_pick_folder,
            rag_project_list,
            rag_project_create,
            rag_project_delete
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

fn normalize_translate_provider(provider: &str) -> String {
    match provider.trim().to_lowercase().as_str() {
        "openai" | "chatgpt" => "openai".to_string(),
        "local-gpt" | "local_gpt" | "localgpt" => "local-gpt".to_string(),
        _ => "ollama".to_string(),
    }
}
