use crate::audio::manager::SegmentInfo;
use chrono::Local;
use hound::{SampleFormat, WavSpec, WavWriter};
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

pub struct SegmentWriter {
    writer: WavWriter<BufWriter<File>>,
    path: PathBuf,
    created_at: String,
    sample_rate: u32,
    channels: u16,
    samples_written: u64,
}

impl SegmentWriter {
    pub fn start_new(dir: &Path, sample_rate: u32, channels: u16) -> Result<Self, String> {
        let now = Local::now();
        let name = format!("segment_{}.wav", now.format("%Y%m%d_%H%M%S_%3f"));
        let path = dir.join(&name);
        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let writer = WavWriter::create(&path, spec).map_err(|err| err.to_string())?;
        Ok(Self {
            writer,
            path,
            created_at: now.to_rfc3339(),
            sample_rate,
            channels,
            samples_written: 0,
        })
    }

    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        for sample in samples {
            self.writer
                .write_sample(*sample)
                .map_err(|err| err.to_string())?;
        }
        self.samples_written += samples.len() as u64;
        Ok(())
    }

    pub fn finalize(mut self) -> Result<SegmentInfo, String> {
        self.writer.flush().map_err(|err| err.to_string())?;
        self.writer.finalize().map_err(|err| err.to_string())?;

        let frames = self.samples_written / self.channels as u64;
        let duration_ms = if self.sample_rate == 0 {
            0
        } else {
            frames.saturating_mul(1000) / self.sample_rate as u64
        };

        let name = self
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("segment.wav")
            .to_string();

        Ok(SegmentInfo {
            name,
            duration_ms,
            created_at: self.created_at,
            sample_rate: self.sample_rate,
            channels: self.channels,
            transcript: None,
            translation: None,
            transcript_at: None,
            translation_at: None,
            transcript_ms: None,
            translation_ms: None,
            speaker_id: None,
            speaker_changed: None,
            speaker_similarity: None,
            speaker_switches_ms: None,
            transcript_cleared: Some(false),
        })
    }
}
