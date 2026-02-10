use crate::app_config::load_config;
use ndarray::Array3;
use ort::session::Session;
use ort::value::TensorRef;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const TARGET_SAMPLE_RATE: u32 = 16_000;
const TARGET_WINDOW_SAMPLES: usize = 16_000;
const DEFAULT_NEW_SPEAKER_THRESHOLD: f32 = 0.75;
const DEFAULT_UPDATE_THRESHOLD: f32 = 0.80;
const DEFAULT_MAX_SPEAKERS: u32 = 8;
const DEFAULT_WINDOW_MS: u64 = 2_000;
const DEFAULT_STEP_MS: u64 = 1_000;
const DEFAULT_MIN_RMS_DB: f32 = -45.0;
const DEFAULT_CONSECUTIVE_HITS: u32 = 3;
const DEFAULT_MIN_GAP_MS: u64 = 3_000;
const DEFAULT_UPDATE_ALPHA: f32 = 0.8;

#[derive(Debug, Clone)]
pub struct SpeakerDecision {
    pub speaker_id: Option<u32>,
    pub similarity: Option<f32>,
    pub mixed: bool,
}

pub struct SpeakerDiarizer {
    embedder: SpeakerEmbedder,
    clusterer: SpeakerClusterer,
    config: DiarizerConfig,
    last_processed: Option<Instant>,
}

impl SpeakerDiarizer {
    pub fn new(app: &AppHandle) -> Option<Self> {
        let config = match load_config() {
            Ok(config) => config,
            Err(err) => {
                eprintln!("speaker config unavailable: {err}");
                return None;
            }
        };

        let speaker = match config.speaker {
            Some(config) => config,
            None => return None,
        };

        if speaker.enabled == Some(false) {
            return None;
        }

        let resource_dir = app.path().resource_dir().ok();
        let model_path = resolve_model_path(
            speaker
                .model_path
                .as_deref()
                .or(Some("resources/models/pyannote_embedding.onnx")),
            resource_dir,
        );
        let model_path = match model_path {
            Some(path) => path,
            None => {
                eprintln!("speaker model path not set");
                return None;
            }
        };
        if !model_path.exists() {
            eprintln!("speaker model not found: {}", model_path.display());
            return None;
        }

        let new_threshold = speaker
            .similarity_threshold
            .unwrap_or(DEFAULT_NEW_SPEAKER_THRESHOLD);
        let update_threshold = speaker
            .update_threshold
            .unwrap_or(DEFAULT_UPDATE_THRESHOLD)
            .max(new_threshold);
        let max_speakers = speaker.max_speakers.or(Some(DEFAULT_MAX_SPEAKERS));
        let window_ms = speaker.window_ms.unwrap_or(DEFAULT_WINDOW_MS);
        let step_ms = speaker.hop_ms.unwrap_or(DEFAULT_STEP_MS).max(200);
        let min_rms_db = speaker.min_rms_db.unwrap_or(DEFAULT_MIN_RMS_DB);

        let switch_window_ms = window_ms.min(1_000).max(500);
        let switch_hop_ms = (step_ms.min(switch_window_ms)).max(200);

        let switch_params = SwitchParams {
            threshold: new_threshold,
            window_ms: switch_window_ms,
            hop_ms: switch_hop_ms,
            min_gap_ms: speaker.min_gap_ms.unwrap_or(DEFAULT_MIN_GAP_MS),
            consecutive_hits: speaker
                .consecutive_hits
                .unwrap_or(DEFAULT_CONSECUTIVE_HITS)
                .max(1),
            min_rms_db,
        };

        let embedder = match SpeakerEmbedder::new(&model_path) {
            Ok(embedder) => embedder,
            Err(err) => {
                eprintln!("speaker embedder init failed: {err}");
                return None;
            }
        };

        Some(Self {
            embedder,
            clusterer: SpeakerClusterer::new(),
            config: DiarizerConfig {
                new_threshold,
                update_threshold,
                max_speakers,
                window_ms,
                step_ms,
                min_rms_db,
                update_alpha: DEFAULT_UPDATE_ALPHA,
                switch_params,
            },
            last_processed: None,
        })
    }

    pub fn process_window(
        &mut self,
        samples: &[f32],
        sample_rate: u32,
        channels: u16,
    ) -> Option<SpeakerDecision> {
        if let Some(last) = self.last_processed {
            if last.elapsed() < Duration::from_millis(self.config.step_ms) {
                return None;
            }
        }

        let mono = mix_to_mono(samples, channels);
        let resampled = resample_to_16k(&mono, sample_rate);
        let window_samples = ms_to_samples(self.config.window_ms, TARGET_SAMPLE_RATE);
        if window_samples == 0 || resampled.len() < window_samples {
            return None;
        }

        self.last_processed = Some(Instant::now());

        let start = resampled.len().saturating_sub(window_samples);
        let window = &resampled[start..];

        if rms_db(window) < self.config.min_rms_db {
            return Some(SpeakerDecision {
                speaker_id: None,
                similarity: None,
                mixed: true,
            });
        }

        if let Ok(switches) = self
            .embedder
            .detect_switches(window, &self.config.switch_params)
        {
            if !switches.is_empty() {
                return Some(SpeakerDecision {
                    speaker_id: None,
                    similarity: None,
                    mixed: true,
                });
            }
        }

        let embed_window = extract_window(window);
        let embedding = match self.embedder.embedding_from_window(&embed_window) {
            Ok(embedding) => embedding,
            Err(err) => {
                eprintln!("speaker embedding failed: {err}");
                return None;
            }
        };

        let decision = self.clusterer.classify(embedding, &self.config);
        Some(decision)
    }
}

struct DiarizerConfig {
    new_threshold: f32,
    update_threshold: f32,
    max_speakers: Option<u32>,
    window_ms: u64,
    step_ms: u64,
    min_rms_db: f32,
    update_alpha: f32,
    switch_params: SwitchParams,
}

struct SpeakerProfile {
    id: u32,
    centroid: Vec<f32>,
}

struct SpeakerClusterer {
    speakers: Vec<SpeakerProfile>,
    next_id: u32,
}

impl SpeakerClusterer {
    fn new() -> Self {
        Self {
            speakers: Vec::new(),
            next_id: 1,
        }
    }

    fn classify(&mut self, embedding: Vec<f32>, config: &DiarizerConfig) -> SpeakerDecision {
        if self.speakers.is_empty() {
            let id = self.next_id;
            self.next_id = self.next_id.saturating_add(1);
            self.speakers.push(SpeakerProfile {
                id,
                centroid: embedding,
            });
            return SpeakerDecision {
                speaker_id: Some(id),
                similarity: None,
                mixed: false,
            };
        }

        let mut best_idx = 0usize;
        let mut best_sim = f32::NEG_INFINITY;
        for (idx, speaker) in self.speakers.iter().enumerate() {
            let sim = cosine_similarity(&speaker.centroid, &embedding);
            if sim > best_sim {
                best_sim = sim;
                best_idx = idx;
            }
        }

        if best_sim < config.new_threshold {
            let at_max = config
                .max_speakers
                .map(|limit| self.speakers.len() as u32 >= limit)
                .unwrap_or(false);
            if at_max {
                return SpeakerDecision {
                    speaker_id: None,
                    similarity: Some(best_sim),
                    mixed: false,
                };
            }
            let id = self.next_id;
            self.next_id = self.next_id.saturating_add(1);
            self.speakers.push(SpeakerProfile {
                id,
                centroid: embedding,
            });
            return SpeakerDecision {
                speaker_id: Some(id),
                similarity: Some(best_sim),
                mixed: false,
            };
        }

        if best_sim >= config.update_threshold {
            let centroid = &mut self.speakers[best_idx].centroid;
            update_centroid(centroid, &embedding, config.update_alpha);
        }

        SpeakerDecision {
            speaker_id: Some(self.speakers[best_idx].id),
            similarity: Some(best_sim),
            mixed: false,
        }
    }
}

struct SpeakerEmbedder {
    session: Session,
}

struct SwitchParams {
    threshold: f32,
    window_ms: u64,
    hop_ms: u64,
    min_gap_ms: u64,
    consecutive_hits: u32,
    min_rms_db: f32,
}

impl SpeakerEmbedder {
    fn new(model_path: &Path) -> Result<Self, String> {
        let session = Session::builder()
            .map_err(|err| err.to_string())?
            .commit_from_file(model_path)
            .map_err(|err| err.to_string())?;
        Ok(Self { session })
    }

    fn embedding_from_window(&mut self, window: &[f32]) -> Result<Vec<f32>, String> {
        let input = Array3::<f32>::from_shape_vec((1, 1, TARGET_WINDOW_SAMPLES), window.to_vec())
            .map_err(|err| err.to_string())?;
        let input_tensor = TensorRef::from_array_view(&input).map_err(|err| err.to_string())?;
        let outputs = self
            .session
            .run(ort::inputs!["audio_input" => input_tensor])
            .map_err(|err| err.to_string())?;
        let output = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|err| err.to_string())?;
        let mut embedding: Vec<f32> = output.iter().copied().collect();
        normalize_embedding(&mut embedding);
        Ok(embedding)
    }

    fn embedding_from_samples(&mut self, samples: &[f32]) -> Result<Vec<f32>, String> {
        let window = extract_window(samples);
        self.embedding_from_window(&window)
    }

    fn detect_switches(
        &mut self,
        samples: &[f32],
        params: &SwitchParams,
    ) -> Result<Vec<u64>, String> {
        let window_samples = ms_to_samples(params.window_ms, TARGET_SAMPLE_RATE);
        if window_samples == 0 || samples.len() < window_samples {
            return Ok(Vec::new());
        }
        let hop_samples = ms_to_samples(params.hop_ms, TARGET_SAMPLE_RATE)
            .max(1)
            .min(window_samples);
        let mut switches = Vec::new();
        let mut prev_embedding: Option<Vec<f32>> = None;
        let mut last_switch_ms: Option<u64> = None;
        let mut start = 0usize;
        let mut consecutive_hits: u32 = 0;
        while start + window_samples <= samples.len() {
            let window = &samples[start..start + window_samples];
            if rms_db(window) < params.min_rms_db {
                start = start.saturating_add(hop_samples);
                continue;
            }
            let embedding = self.embedding_from_samples(window)?;
            if let Some(prev) = prev_embedding.as_ref() {
                let similarity = cosine_similarity(prev, &embedding);
                if similarity < params.threshold {
                    consecutive_hits = consecutive_hits.saturating_add(1);
                } else {
                    consecutive_hits = 0;
                }
                if consecutive_hits >= params.consecutive_hits {
                    let time_ms = ((start + window_samples / 2) as f32 / TARGET_SAMPLE_RATE as f32
                        * 1000.0)
                        .round() as u64;
                    let allow = match last_switch_ms {
                        Some(last) => time_ms.saturating_sub(last) >= params.min_gap_ms,
                        None => true,
                    };
                    if allow {
                        switches.push(time_ms);
                        last_switch_ms = Some(time_ms);
                        consecutive_hits = 0;
                    }
                }
            }
            prev_embedding = Some(embedding);
            start = start.saturating_add(hop_samples);
        }
        Ok(switches)
    }
}

fn mix_to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    if channels == 1 {
        return samples.to_vec();
    }
    let mut mono = Vec::with_capacity(samples.len() / channels);
    let mut sum = 0.0f32;
    let mut count = 0usize;
    for sample in samples {
        sum += *sample;
        count += 1;
        if count == channels {
            mono.push(sum / channels as f32);
            sum = 0.0;
            count = 0;
        }
    }
    if count > 0 {
        mono.push(sum / count as f32);
    }
    mono
}

fn resample_to_16k(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    if sample_rate == TARGET_SAMPLE_RATE {
        return samples.to_vec();
    }
    let ratio = sample_rate as f32 / TARGET_SAMPLE_RATE as f32;
    let output_len = (samples.len() as f32 / ratio).floor().max(0.0) as usize;
    let mut output = Vec::with_capacity(output_len);
    for i in 0..output_len {
        let src_index = (i as f32 * ratio).floor() as usize;
        if let Some(sample) = samples.get(src_index) {
            output.push(*sample);
        }
    }
    output
}

fn extract_window(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return vec![0.0; TARGET_WINDOW_SAMPLES];
    }
    if samples.len() >= TARGET_WINDOW_SAMPLES {
        let start = (samples.len() - TARGET_WINDOW_SAMPLES) / 2;
        return samples[start..start + TARGET_WINDOW_SAMPLES].to_vec();
    }
    let mut window = vec![0.0f32; TARGET_WINDOW_SAMPLES];
    window[..samples.len()].copy_from_slice(samples);
    window
}

fn normalize_embedding(embedding: &mut [f32]) {
    let mut sum = 0.0f32;
    for value in embedding.iter() {
        sum += value * value;
    }
    let norm = sum.sqrt().max(1e-9);
    for value in embedding.iter_mut() {
        *value /= norm;
    }
}

fn update_centroid(centroid: &mut [f32], embedding: &[f32], alpha: f32) {
    let alpha = alpha.clamp(0.0, 1.0);
    for (value, update) in centroid.iter_mut().zip(embedding.iter()) {
        *value = *value * alpha + *update * (1.0 - alpha);
    }
    normalize_embedding(centroid);
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    let len = left.len().min(right.len());
    if len == 0 {
        return 0.0;
    }
    let mut dot = 0.0f32;
    for i in 0..len {
        dot += left[i] * right[i];
    }
    dot
}

fn rms_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return f32::NEG_INFINITY;
    }
    let mut sum = 0.0f32;
    for sample in samples {
        sum += sample * sample;
    }
    let mean = sum / samples.len() as f32;
    let rms = mean.sqrt();
    20.0 * rms.max(1e-9).log10()
}

fn ms_to_samples(ms: u64, sample_rate: u32) -> usize {
    if sample_rate == 0 {
        return 0;
    }
    (ms.saturating_mul(sample_rate as u64) / 1000) as usize
}

fn resolve_model_path(path: Option<&str>, resource_dir: Option<PathBuf>) -> Option<PathBuf> {
    let raw = path?.trim();
    if raw.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(raw);
    if candidate.is_absolute() {
        return Some(candidate);
    }

    let mut candidates = Vec::new();
    if let Some(dir) = resource_dir {
        candidates.push(dir.join(&candidate));
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

    candidates
        .into_iter()
        .find(|path| path.exists())
        .or(Some(candidate))
}
