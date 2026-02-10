use std::ptr;

use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVE_FORMAT_PCM,
};
use windows::Win32::Media::KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE};
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

struct ComGuard;

impl ComGuard {
    fn new() -> Result<Self, String> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }
            .map_err(|err| err.to_string())?;
        Ok(Self)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

pub struct LoopbackCapture {
    _com: ComGuard,
    audio_client: IAudioClient,
    capture_client: IAudioCaptureClient,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    is_float: bool,
}

impl LoopbackCapture {
    pub fn new() -> Result<Self, String> {
        let com = ComGuard::new()?;

        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|err| err.to_string())?;
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
            .map_err(|err| err.to_string())?;
        let audio_client: IAudioClient =
            unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|err| err.to_string())?;

        let mix_ptr = unsafe { audio_client.GetMixFormat() }.map_err(|err| err.to_string())?;
        if mix_ptr.is_null() {
            return Err("WASAPI mix format is null".to_string());
        }

        let mix = unsafe { ptr::read_unaligned(mix_ptr) };
        let format_tag = mix.wFormatTag as u32;
        let (bits_per_sample, is_float) = if format_tag == WAVE_FORMAT_EXTENSIBLE {
            let extensible = unsafe { ptr::read_unaligned(mix_ptr as *const WAVEFORMATEXTENSIBLE) };
            let format = unsafe { ptr::read_unaligned(ptr::addr_of!(extensible.Format)) };
            let subformat = unsafe { ptr::read_unaligned(ptr::addr_of!(extensible.SubFormat)) };
            if subformat == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
                (format.wBitsPerSample, true)
            } else if subformat == KSDATAFORMAT_SUBTYPE_PCM {
                (format.wBitsPerSample, false)
            } else {
                unsafe {
                    CoTaskMemFree(Some(mix_ptr as _));
                }
                return Err("Unsupported WASAPI subformat".to_string());
            }
        } else if format_tag == WAVE_FORMAT_IEEE_FLOAT {
            (mix.wBitsPerSample, true)
        } else if format_tag == WAVE_FORMAT_PCM {
            (mix.wBitsPerSample, false)
        } else {
            unsafe {
                CoTaskMemFree(Some(mix_ptr as _));
            }
            return Err(format!("Unsupported WASAPI format tag: {format_tag}"));
        };

        let sample_rate = mix.nSamplesPerSec;
        let channels = mix.nChannels;

        unsafe {
            audio_client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK,
                    10_000_000,
                    0,
                    mix_ptr as *const WAVEFORMATEX,
                    None,
                )
                .map_err(|err| err.to_string())?;
        }

        unsafe {
            CoTaskMemFree(Some(mix_ptr as _));
        }

        let capture_client: IAudioCaptureClient =
            unsafe { audio_client.GetService() }.map_err(|err| err.to_string())?;

        unsafe { audio_client.Start() }.map_err(|err| err.to_string())?;

        if !(bits_per_sample == 16 || bits_per_sample == 32) {
            return Err(format!(
                "Unsupported WASAPI bits_per_sample: {bits_per_sample}"
            ));
        }

        Ok(Self {
            _com: com,
            audio_client,
            capture_client,
            channels,
            sample_rate,
            bits_per_sample,
            is_float,
        })
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn read(&mut self) -> Result<Vec<f32>, String> {
        let mut packet_size =
            unsafe { self.capture_client.GetNextPacketSize() }.map_err(|err| err.to_string())?;
        if packet_size == 0 {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        while packet_size != 0 {
            let mut data_ptr: *mut u8 = ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;

            unsafe {
                self.capture_client
                    .GetBuffer(&mut data_ptr, &mut frames, &mut flags, None, None)
                    .map_err(|err| err.to_string())?;
            }

            let samples = frames as usize * self.channels as usize;
            if samples == 0 {
                unsafe {
                    self.capture_client
                        .ReleaseBuffer(frames)
                        .map_err(|err| err.to_string())?;
                }
                packet_size = unsafe { self.capture_client.GetNextPacketSize() }
                    .map_err(|err| err.to_string())?;
                continue;
            }

            let is_silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
            if is_silent || data_ptr.is_null() {
                out.extend(std::iter::repeat(0.0).take(samples));
            } else if self.is_float && self.bits_per_sample == 32 {
                let slice = unsafe { std::slice::from_raw_parts(data_ptr as *const f32, samples) };
                out.extend_from_slice(slice);
            } else if !self.is_float && self.bits_per_sample == 16 {
                let slice = unsafe { std::slice::from_raw_parts(data_ptr as *const i16, samples) };
                out.extend(slice.iter().map(|value| *value as f32 / 32768.0));
            } else {
                unsafe {
                    self.capture_client
                        .ReleaseBuffer(frames)
                        .map_err(|err| err.to_string())?;
                }
                return Err("Unsupported WASAPI sample format".to_string());
            }

            unsafe {
                self.capture_client
                    .ReleaseBuffer(frames)
                    .map_err(|err| err.to_string())?;
            }
            packet_size = unsafe { self.capture_client.GetNextPacketSize() }
                .map_err(|err| err.to_string())?;
        }

        Ok(out)
    }
}

impl Drop for LoopbackCapture {
    fn drop(&mut self) {
        unsafe {
            let _ = self.audio_client.Stop();
        }
    }
}
