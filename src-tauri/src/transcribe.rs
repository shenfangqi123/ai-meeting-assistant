use crate::app_config::{load_config, AsrConfig, OpenAiConfig};
use crate::asr::AsrState;
use crate::transcribe_backend::{resolve_whisper_transcribe_backend, WhisperTranscribeBackend};
use crate::whisper_server::WhisperServerManager;
use reqwest::multipart::{Form, Part};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const DEFAULT_MODEL: &str = "whisper-1";
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1/audio/transcriptions";
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const DEFAULT_RESPONSE_FORMAT: &str = "json";
const DEFAULT_WHISPER_SERVER_URL: &str = "http://127.0.0.1:8080/inference";
const DEFAULT_WHISPER_SERVER_RESPONSE_FORMAT: &str = "text";
const DEFAULT_WHISPER_SERVER_TEMPERATURE: &str = "0";
const DEFAULT_WHISPER_PIPE_TIMEOUT_SECS: u64 = 120;
const PIPE_IO_POLL_MS: u64 = 30;
const PIPE_ERROR_SNIPPET_CHARS: usize = 320;

pub async fn transcribe_file(
    app: &AppHandle,
    path: &Path,
    whisper_prompt_hint: Option<&str>,
) -> Result<String, String> {
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
            let server_result =
                transcribe_with_whisper_backend(app, path, &asr_config, whisper_prompt_hint).await;
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

pub async fn transcribe_with_whisper_backend(
    app: &AppHandle,
    path: &Path,
    config: &AsrConfig,
    prompt_hint: Option<&str>,
) -> Result<String, String> {
    match resolve_whisper_transcribe_backend(config) {
        WhisperTranscribeBackend::Server => {
            transcribe_with_whisper_server(app, path, config, prompt_hint).await
        }
        WhisperTranscribeBackend::Pipe => {
            transcribe_with_whisper_pipe(app, path, config, prompt_hint).await
        }
    }
}

pub async fn transcribe_with_whisper_server(
    app: &AppHandle,
    path: &Path,
    config: &AsrConfig,
    prompt_hint: Option<&str>,
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
        .text(
            "temperature",
            DEFAULT_WHISPER_SERVER_TEMPERATURE.to_string(),
        )
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
    if let Some(prompt) = prompt_hint.map(str::trim).filter(|value| !value.is_empty()) {
        // Context is passed as a soft hint, not an instruction that forces correction.
        form = form
            .text("prompt", prompt.to_string())
            .text("initial_prompt", prompt.to_string());
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

async fn transcribe_with_whisper_pipe(
    app: &AppHandle,
    path: &Path,
    config: &AsrConfig,
    prompt_hint: Option<&str>,
) -> Result<String, String> {
    let app = app.clone();
    let path = path.to_path_buf();
    let config = config.clone();
    let prompt_hint = prompt_hint.map(|value| value.to_string());
    tauri::async_runtime::spawn_blocking(move || {
        transcribe_with_whisper_pipe_blocking(&app, &path, &config, prompt_hint.as_deref())
    })
    .await
    .map_err(|err| err.to_string())?
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

fn transcribe_with_whisper_pipe_blocking(
    app: &AppHandle,
    path: &Path,
    config: &AsrConfig,
    prompt_hint: Option<&str>,
) -> Result<String, String> {
    let pipe_exe = resolve_whisper_pipe_executable(app, config).ok_or_else(|| {
        "whisper pipe executable not found (set `asr.whisperPipePath`)".to_string()
    })?;
    let timeout_secs = config
        .whisper_pipe_timeout_secs
        .unwrap_or(DEFAULT_WHISPER_PIPE_TIMEOUT_SECS)
        .max(1);
    let audio_bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    let model_path = resolve_whisper_pipe_model_path(app, config);

    let mut cmd = Command::new(&pipe_exe);
    if let Some(args) = config.whisper_pipe_args.as_ref() {
        for arg in args.iter().map(|value| value.trim()) {
            if !arg.is_empty() {
                cmd.arg(arg);
            }
        }
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(parent) = pipe_exe.parent() {
        cmd.current_dir(parent);
    }

    cmd.env("AI_SHEPHERD_WHISPER_IPC", "stdin-stdout");
    cmd.env("AI_SHEPHERD_WHISPER_INPUT_MIME", "audio/wav");
    cmd.env("AI_SHEPHERD_WHISPER_INPUT_PATH", path.as_os_str());

    if let Some(model) = model_path.as_ref() {
        cmd.env("AI_SHEPHERD_WHISPER_MODEL", model.as_os_str());
    }
    if let Some(language) = config
        .language
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        cmd.env("AI_SHEPHERD_WHISPER_LANGUAGE", language);
    }
    if let Some(prompt) = prompt_hint.map(str::trim).filter(|value| !value.is_empty()) {
        cmd.env("AI_SHEPHERD_WHISPER_PROMPT_HINT", prompt);
    }

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("failed to spawn whisper pipe: {err}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "failed to open whisper pipe stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to open whisper pipe stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to open whisper pipe stderr".to_string())?;

    let stdout_reader = thread::spawn(move || read_pipe_text(stdout));
    let stderr_reader = thread::spawn(move || read_pipe_text(stderr));

    stdin
        .write_all(&audio_bytes)
        .map_err(|err| format!("failed to write audio to whisper pipe stdin: {err}"))?;
    drop(stdin);

    let started = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let stdout_text = join_reader(stdout_reader, "stdout")?;
                    let stderr_text = join_reader(stderr_reader, "stderr")?;
                    return Err(format!(
                        "whisper pipe timed out after {}s (stderr: {}, stdout: {})",
                        timeout_secs,
                        compact_error_text(&stderr_text),
                        compact_error_text(&stdout_text),
                    ));
                }
                thread::sleep(Duration::from_millis(PIPE_IO_POLL_MS));
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                let stdout_text = join_reader(stdout_reader, "stdout")?;
                let stderr_text = join_reader(stderr_reader, "stderr")?;
                return Err(format!(
                    "failed while waiting whisper pipe process: {err} (stderr: {}, stdout: {})",
                    compact_error_text(&stderr_text),
                    compact_error_text(&stdout_text),
                ));
            }
        }
    };

    let stdout_text = join_reader(stdout_reader, "stdout")?;
    let stderr_text = join_reader(stderr_reader, "stderr")?;

    if !status.success() {
        return Err(format!(
            "whisper pipe exited with {status} (stderr: {}, stdout: {})",
            compact_error_text(&stderr_text),
            compact_error_text(&stdout_text),
        ));
    }

    extract_pipe_transcript(&stdout_text).ok_or_else(|| {
        format!(
            "whisper pipe returned empty transcript (stderr: {})",
            compact_error_text(&stderr_text),
        )
    })
}

fn resolve_whisper_pipe_executable(app: &AppHandle, config: &AsrConfig) -> Option<PathBuf> {
    if let Some(path) = config
        .whisper_pipe_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| resolve_path_with_context(app, value))
    {
        return Some(path);
    }

    let names = [
        "whisper-engine.exe",
        "whisper-pipe.exe",
        "whisper-engine",
        "whisper-pipe",
    ];
    let mut candidates = Vec::new();

    if let Ok(resource_dir) = app.path().resource_dir() {
        for name in names {
            candidates.push(resource_dir.join(name));
            candidates.push(resource_dir.join("whisper").join(name));
            candidates.push(resource_dir.join("whisper").join("pipe").join(name));
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        for name in names {
            candidates.push(cwd.join(name));
            candidates.push(cwd.join("whisper-bin-x64").join("Release").join(name));
            candidates.push(cwd.join("install").join("pipe").join(name));
            candidates.push(cwd.join("src-tauri").join(name));
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in names {
                candidates.push(dir.join(name));
                candidates.push(dir.join("whisper").join(name));
            }
        }
    }

    candidates.into_iter().find(|path| path.exists())
}

fn resolve_whisper_pipe_model_path(app: &AppHandle, config: &AsrConfig) -> Option<PathBuf> {
    let mut raws = Vec::new();
    if let Some(raw) = config
        .whisper_cpp_model_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        raws.push(raw.to_string());
    }
    raws.push("resources/models/ggml-base.bin".to_string());
    raws.push("models/ggml-small-q5_1.bin".to_string());

    for raw in raws {
        if let Some(path) = resolve_path_with_context(app, &raw) {
            return Some(path);
        }
    }
    None
}

fn resolve_path_with_context(app: &AppHandle, raw: &str) -> Option<PathBuf> {
    let candidate = PathBuf::from(raw.trim());
    if candidate.as_os_str().is_empty() {
        return None;
    }
    if candidate.is_absolute() {
        return candidate.exists().then_some(candidate);
    }

    let mut candidates = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join(&candidate));
        if let Some(parent) = resource_dir.parent() {
            candidates.push(parent.join(&candidate));
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(&candidate));
        candidates.push(cwd.join("src-tauri").join(&candidate));
        if let Some(parent) = cwd.parent() {
            candidates.push(parent.join(&candidate));
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(&candidate));
            if let Some(parent) = dir.parent() {
                candidates.push(parent.join(&candidate));
            }
        }
    }

    candidates.into_iter().find(|path| path.exists())
}

fn read_pipe_text<R: Read>(mut reader: R) -> Result<String, String> {
    let mut buffer = String::new();
    reader
        .read_to_string(&mut buffer)
        .map_err(|err| err.to_string())?;
    Ok(buffer)
}

fn join_reader(
    handle: thread::JoinHandle<Result<String, String>>,
    stream_label: &str,
) -> Result<String, String> {
    match handle.join() {
        Ok(result) => {
            result.map_err(|err| format!("failed to read whisper pipe {stream_label}: {err}"))
        }
        Err(_) => Err(format!(
            "whisper pipe {stream_label} reader thread panicked"
        )),
    }
}

fn extract_pipe_transcript(stdout_text: &str) -> Option<String> {
    let trimmed = stdout_text.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(text) = value
            .get("text")
            .and_then(|field| field.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(text.to_string());
        }
        if let Some(text) = value
            .get("transcript")
            .and_then(|field| field.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(text.to_string());
        }
    }

    Some(trimmed.to_string())
}

fn compact_error_text(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "<empty>".to_string();
    }
    let mut compact = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() > PIPE_ERROR_SNIPPET_CHARS {
        compact = compact
            .chars()
            .take(PIPE_ERROR_SNIPPET_CHARS)
            .collect::<String>();
        compact.push_str("...");
    }
    compact
}
