use crate::app_config::load_config;
use std::sync::Mutex;

pub struct AsrState {
    provider: Mutex<String>,
    fallback_to_openai: Mutex<bool>,
    language: Mutex<String>,
}

impl AsrState {
    pub fn new() -> Self {
        let config = load_config()
            .ok()
            .and_then(|cfg| cfg.asr)
            .unwrap_or_default();
        let provider = config
            .provider
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "whisperserver".to_string());
        let fallback = config.fallback_to_openai.unwrap_or(true);
        let language = config
            .language
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "ja".to_string());
        let _ = config;
        Self {
            provider: Mutex::new(normalize_provider(&provider)),
            fallback_to_openai: Mutex::new(fallback),
            language: Mutex::new(normalize_language(&language)),
        }
    }

    pub fn provider(&self) -> String {
        self.provider
            .lock()
            .map(|value| value.clone())
            .unwrap_or_else(|_| "whisperserver".to_string())
    }

    pub fn set_provider(&self, provider: String) -> String {
        let normalized = normalize_provider(&provider);
        if let Ok(mut guard) = self.provider.lock() {
            *guard = normalized.clone();
        }
        normalized
    }

    pub fn fallback_to_openai(&self) -> bool {
        *self
            .fallback_to_openai
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_fallback_to_openai(&self, value: bool) -> bool {
        if let Ok(mut guard) = self.fallback_to_openai.lock() {
            *guard = value;
            return *guard;
        }
        value
    }

    pub fn language(&self) -> String {
        self.language
            .lock()
            .map(|value| value.clone())
            .unwrap_or_else(|_| "ja".to_string())
    }

    pub fn set_language(&self, language: String) -> String {
        let normalized = normalize_language(&language);
        if let Ok(mut guard) = self.language.lock() {
            *guard = normalized.clone();
        }
        normalized
    }
}

fn normalize_provider(raw: &str) -> String {
    let trimmed = raw.trim().to_lowercase();
    match trimmed.as_str() {
        "openai" => "openai".to_string(),
        "whispercpp" | "whisper.cpp" | "whisper" => "whisperserver".to_string(),
        "whisperserver" | "whisper-server" | "whisper_server" | "server" => {
            "whisperserver".to_string()
        }
        _ => "whisperserver".to_string(),
    }
}

fn normalize_language(raw: &str) -> String {
    let trimmed = raw.trim().to_lowercase();
    match trimmed.as_str() {
        "zh" | "zh-cn" | "zh-hans" | "chinese" => "zh".to_string(),
        "en" | "en-us" | "en-gb" | "english" => "en".to_string(),
        "ja" | "ja-jp" | "japanese" => "ja".to_string(),
        "" => "ja".to_string(),
        other => other.to_string(),
    }
}
