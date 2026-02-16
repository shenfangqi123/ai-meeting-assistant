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

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalGptConfig {
    pub enabled: Option<bool>,
    pub base_url: Option<String>,
    pub timeout_secs: Option<u64>,
    #[serde(alias = "project-id", alias = "project_id")]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub openai: OpenAiConfig,
    #[allow(dead_code)]
    pub ollama: Option<OllamaConfig>,
    #[allow(dead_code)]
    #[serde(alias = "localGpt", alias = "local-gpt")]
    pub local_gpt: Option<LocalGptConfig>,
    pub translate: Option<TranslateConfig>,
    pub speaker: Option<SpeakerConfig>,
    pub asr: Option<AsrConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateConfig {
    pub enabled: Option<bool>,
    pub provider: Option<String>,
    pub target_language: Option<String>,
    pub segment_batch_size: Option<usize>,
    pub segment_single_prompt: Option<String>,
    pub segment_batch_prompt: Option<String>,
    pub live_prompt: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerConfig {
    pub enabled: Option<bool>,
    pub model_path: Option<String>,
    pub similarity_threshold: Option<f32>,
    pub update_threshold: Option<f32>,
    pub max_speakers: Option<u32>,
    pub window_ms: Option<u64>,
    pub hop_ms: Option<u64>,
    pub min_gap_ms: Option<u64>,
    pub consecutive_hits: Option<u32>,
    pub min_rms_db: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrConfig {
    pub provider: Option<String>,
    pub whisper_backend: Option<String>,
    pub whisper_pipe_path: Option<String>,
    pub whisper_pipe_args: Option<Vec<String>>,
    pub whisper_pipe_timeout_secs: Option<u64>,
    pub whisper_cpp_model_path: Option<String>,
    pub whisper_server_path: Option<String>,
    pub whisper_server_gpu_path: Option<String>,
    pub whisper_server_cpu_path: Option<String>,
    pub whisper_server_device: Option<String>,
    pub whisper_server_url: Option<String>,
    pub whisper_server_timeout_secs: Option<u64>,
    pub language: Option<String>,
    pub fallback_to_openai: Option<bool>,
    pub use_whisper_vad: Option<bool>,
    pub whisper_cpp_vad_path: Option<String>,
    pub whisper_cpp_vad_model_path: Option<String>,
    pub use_whisper_stream: Option<bool>,
    pub whisper_cpp_stream_path: Option<String>,
    pub whisper_cpp_stream_step_ms: Option<u64>,
    pub whisper_context_enabled: Option<bool>,
    pub whisper_context_max_chars: Option<usize>,
    pub whisper_context_short_segment_ms: Option<u64>,
    pub whisper_context_boundary_gap_ms: Option<u64>,
    pub whisper_context_reset_silence_ms: Option<u64>,
    pub whisper_vad_min_speech_ratio: Option<f32>,
    pub whisper_vad_min_speech_ms: Option<u64>,
    pub transcript_post_filter_enabled: Option<bool>,
    pub transcript_noise_max_meaningful_chars: Option<usize>,
    pub transcript_repeat_char_ratio: Option<f32>,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            provider: Some("whisperserver".to_string()),
            whisper_backend: Some("server".to_string()),
            whisper_pipe_path: None,
            whisper_pipe_args: None,
            whisper_pipe_timeout_secs: None,
            whisper_cpp_model_path: Some("resources/models/ggml-base.bin".to_string()),
            whisper_server_path: None,
            whisper_server_gpu_path: None,
            whisper_server_cpu_path: None,
            whisper_server_device: Some("auto".to_string()),
            whisper_server_url: None,
            whisper_server_timeout_secs: None,
            language: Some("ja".to_string()),
            fallback_to_openai: Some(true),
            use_whisper_vad: Some(false),
            whisper_cpp_vad_path: Some("whisper-vad-speech-segments.exe".to_string()),
            whisper_cpp_vad_model_path: None,
            use_whisper_stream: Some(false),
            whisper_cpp_stream_path: Some("whisper-stream.exe".to_string()),
            whisper_cpp_stream_step_ms: Some(1000),
            whisper_context_enabled: Some(true),
            whisper_context_max_chars: Some(100),
            whisper_context_short_segment_ms: Some(2500),
            whisper_context_boundary_gap_ms: Some(1200),
            whisper_context_reset_silence_ms: Some(4000),
            whisper_vad_min_speech_ratio: Some(0.25),
            whisper_vad_min_speech_ms: Some(350),
            transcript_post_filter_enabled: Some(true),
            transcript_noise_max_meaningful_chars: Some(10),
            transcript_repeat_char_ratio: Some(0.72),
        }
    }
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
    Err(format!("ai-interview.config not found. Tried: {attempted}"))
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
