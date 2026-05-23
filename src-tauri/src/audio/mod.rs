use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct AudioCapture {
    stream: Option<cpal::Stream>,
    is_recording: Arc<AtomicBool>,
}

// cpal::Stream is !Send but we only access it under a std::sync::Mutex
unsafe impl Send for AudioCapture {}
unsafe impl Sync for AudioCapture {}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
}

pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub device_name: Option<String>,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            channels: 1,
            device_name: None,
        }
    }
}

pub fn list_input_devices() -> Result<Vec<AudioDeviceInfo>> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|d| d.name().ok());
    let mut devices = Vec::new();
    for device in host.input_devices()? {
        if let Ok(name) = device.name() {
            devices.push(AudioDeviceInfo {
                is_default: default_name.as_deref() == Some(&name),
                name,
            });
        }
    }
    Ok(devices)
}

impl AudioCapture {
    pub fn new() -> Self {
        Self {
            stream: None,
            is_recording: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&mut self, producer: rtrb::Producer<f32>, config: AudioConfig) -> Result<()> {
        let host = cpal::default_host();
        let device = match &config.device_name {
            Some(name) => host
                .input_devices()?
                .find(|d| d.name().map(|n| n == *name).unwrap_or(false))
                .or_else(|| {
                    log::warn!(
                        "audio: requested device {:?} not found, falling back to default",
                        name
                    );
                    host.default_input_device()
                })
                .context("no input device available")?,
            None => host
                .default_input_device()
                .context("no input device available")?,
        };

        let supported = device.default_input_config()?;
        let source_rate = supported.sample_rate().0;
        let source_channels = supported.channels() as u32;
        let target_rate = config.sample_rate;

        log::info!(
            "audio: device={:?} source_rate={source_rate} channels={source_channels} target_rate={target_rate}",
            device.name().unwrap_or_default()
        );

        let is_recording = self.is_recording.clone();
        is_recording.store(true, Ordering::Relaxed);

        let mut prod = producer;
        let needs_resample = source_rate != target_rate;
        let ratio = target_rate as f64 / source_rate as f64;

        let native_config = cpal::StreamConfig {
            channels: supported.channels(),
            sample_rate: supported.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        };

        let stream = device.build_input_stream(
            &native_config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // Downmix to mono if stereo
                let mono: Vec<f32> = if source_channels > 1 {
                    data.chunks(source_channels as usize)
                        .map(|ch| ch.iter().sum::<f32>() / source_channels as f32)
                        .collect()
                } else {
                    data.to_vec()
                };

                // Resample if needed (linear interpolation)
                let output = if needs_resample {
                    let out_len = (mono.len() as f64 * ratio).ceil() as usize;
                    let mut resampled = Vec::with_capacity(out_len);
                    for i in 0..out_len {
                        let src_pos = i as f64 / ratio;
                        let idx = src_pos as usize;
                        let frac = (src_pos - idx as f64) as f32;
                        let s0 = mono.get(idx).copied().unwrap_or(0.0);
                        let s1 = mono.get(idx + 1).copied().unwrap_or(s0);
                        resampled.push(s0 + frac * (s1 - s0));
                    }
                    resampled
                } else {
                    mono
                };

                // Write to ring buffer (drop samples if full)
                let to_write = output.len().min(prod.slots());
                if to_write > 0 {
                    if let Ok(mut chunk) = prod.write_chunk_uninit(to_write) {
                        let (first, second) = chunk.as_mut_slices();
                        let first_len = first.len().min(to_write);
                        for (slot, &sample) in first.iter_mut().zip(output[..first_len].iter()) {
                            slot.write(sample);
                        }
                        let remaining = to_write - first_len;
                        if remaining > 0 {
                            for (slot, &sample) in second
                                .iter_mut()
                                .zip(output[first_len..first_len + remaining].iter())
                            {
                                slot.write(sample);
                            }
                        }
                        unsafe { chunk.commit_all() };
                    }
                }
            },
            |err| log::error!("audio capture error: {err}"),
            None,
        )?;

        stream.play()?;
        self.stream = Some(stream);
        Ok(())
    }

    pub fn stop(&mut self) {
        self.is_recording.store(false, Ordering::Relaxed);
        self.stream = None;
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::Relaxed)
    }
}
