use crate::app_config::AsrConfig;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WhisperTranscribeBackend {
    Server,
    Pipe,
}

impl WhisperTranscribeBackend {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Pipe => "pipe",
        }
    }
}

pub fn resolve_whisper_transcribe_backend(config: &AsrConfig) -> WhisperTranscribeBackend {
    let raw = config
        .whisper_backend
        .as_deref()
        .map(str::trim)
        .unwrap_or("");
    match raw.to_ascii_lowercase().as_str() {
        "pipe" | "pipe-ipc" | "pipeipc" | "ipc" | "stdio" | "stdin" | "stdout" => {
            WhisperTranscribeBackend::Pipe
        }
        _ => WhisperTranscribeBackend::Server,
    }
}

pub fn should_start_whisper_server(config: &AsrConfig) -> bool {
    if !provider_uses_whisper(config) {
        return false;
    }
    resolve_whisper_transcribe_backend(config) == WhisperTranscribeBackend::Server
}

fn provider_uses_whisper(config: &AsrConfig) -> bool {
    let provider = config
        .provider
        .as_deref()
        .map(str::trim)
        .unwrap_or("whisperserver");
    matches!(
        provider.to_ascii_lowercase().as_str(),
        "whisperserver"
            | "whisper-server"
            | "whisper_server"
            | "server"
            | "whispercpp"
            | "whisper.cpp"
            | "whisper"
    )
}
