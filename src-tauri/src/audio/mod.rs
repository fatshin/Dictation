use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct AudioCapture {
    stream: Option<cpal::Stream>,
    is_recording: Arc<AtomicBool>,
}

pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u16,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            channels: 1,
        }
    }
}

impl AudioCapture {
    pub fn new() -> Self {
        Self {
            stream: None,
            is_recording: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(
        &mut self,
        producer: rtrb::Producer<f32>,
        config: AudioConfig,
    ) -> Result<()> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("no input device available")?;

        let supported = device.default_input_config()?;
        let sample_rate = cpal::SampleRate(config.sample_rate);
        let stream_config = cpal::StreamConfig {
            channels: config.channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        let is_recording = self.is_recording.clone();
        is_recording.store(true, Ordering::Relaxed);

        let mut prod = producer;
        let needs_resample = supported.sample_rate().0 != config.sample_rate;

        let stream = if needs_resample {
            let source_rate = supported.sample_rate().0;
            let target_rate = config.sample_rate;
            let mut resampler = create_resampler(source_rate, target_rate)?;

            let native_config = cpal::StreamConfig {
                channels: config.channels,
                sample_rate: supported.sample_rate(),
                buffer_size: cpal::BufferSize::Default,
            };

            device.build_input_stream(
                &native_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if let Some(resampled) = resample(&mut resampler, data) {
                        let _ = prod.write_chunk_uninit(resampled.len()).map(|mut chunk| {
                            for (slot, &sample) in chunk.as_mut_slices().0.iter_mut().zip(resampled.iter()) {
                                slot.write(sample);
                            }
                            unsafe { chunk.commit_all() };
                        });
                    }
                },
                |err| log::error!("audio capture error: {err}"),
                None,
            )?
        } else {
            device.build_input_stream(
                &stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let _ = prod.write_chunk_uninit(data.len()).map(|mut chunk| {
                        for (slot, &sample) in chunk.as_mut_slices().0.iter_mut().zip(data.iter()) {
                            slot.write(sample);
                        }
                        unsafe { chunk.commit_all() };
                    });
                },
                |err| log::error!("audio capture error: {err}"),
                None,
            )?
        };

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

fn create_resampler(
    source_rate: u32,
    target_rate: u32,
) -> Result<rubato::Fft<f32>> {
    let resampler = rubato::Fft::new(
        source_rate as usize,
        target_rate as usize,
        1024,
        1,
        1,
        rubato::FixedSync::Input,
    )?;
    Ok(resampler)
}

fn resample(
    resampler: &mut rubato::Fft<f32>,
    data: &[f32],
) -> Option<Vec<f32>> {
    use rubato::Resampler;
    use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
    let input_data = vec![data.to_vec()];
    let input = match SequentialSliceOfVecs::new(&input_data, 1, data.len()) {
        Ok(inp) => inp,
        Err(_) => return None,
    };
    match resampler.process(&input, 0, None) {
        Ok(output) => {
            let buf = output.take_data();
            Some(buf)
        }
        Err(_) => None,
    }
}
