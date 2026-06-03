// ABOUTME: Audio input device handling for microphone capture.
// ABOUTME: Manages input streams and routes audio to mixer via ring buffers.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, SupportedStreamConfig};
use ringbuf::{HeapConsumer, HeapProducer, HeapRb};

/// List available audio input devices
pub fn list_input_devices() {
    let host = cpal::default_host();

    println!("Available audio input devices:");
    match host.input_devices() {
        Ok(devices) => {
            for (i, device) in devices.enumerate() {
                if let Ok(name) = device.name() {
                    println!("  {}. {}", i, name);

                    // Query all supported configs to find max channels
                    if let Ok(configs) = device.supported_input_configs() {
                        let mut max_channels = 0u16;
                        let mut sample_rates: Vec<(u32, u32)> = Vec::new();

                        // Filter configs: ignore those with absurd sample rates
                        // (ALSA plugins report 4294967295 Hz which is clearly fake)
                        const MAX_REASONABLE_SAMPLE_RATE: u32 = 384000;

                        for config in configs {
                            let max_rate = config.max_sample_rate().0;
                            // Skip configs from ALSA plugins that claim unrealistic capabilities
                            if max_rate > MAX_REASONABLE_SAMPLE_RATE {
                                continue;
                            }

                            max_channels = max_channels.max(config.channels());
                            let min_rate = config.min_sample_rate().0;
                            // Collect unique sample rate ranges
                            if !sample_rates
                                .iter()
                                .any(|(min, max)| *min == min_rate && *max == max_rate)
                            {
                                sample_rates.push((min_rate, max_rate));
                            }
                        }

                        if max_channels > 0 {
                            // Show sample rate range(s)
                            if sample_rates.len() == 1 {
                                let (min, max) = sample_rates[0];
                                if min == max {
                                    println!("     Sample rate: {} Hz", min);
                                } else {
                                    println!("     Sample rate: {}-{} Hz", min, max);
                                }
                            } else if !sample_rates.is_empty() {
                                // Multiple ranges, just show common rates
                                println!("     Sample rates: (multiple configurations)");
                            }
                            println!("     Max channels: {}", max_channels);
                        } else if let Ok(config) = device.default_input_config() {
                            // All configs were filtered out - fall back to default
                            // This happens with ALSA plugin devices
                            println!("     Sample rate: {} Hz (plugin)", config.sample_rate().0);
                            println!("     Channels: {} (plugin)", config.channels());
                        }
                    } else if let Ok(config) = device.default_input_config() {
                        // Fallback to default config if supported_input_configs fails
                        println!("     Sample rate: {} Hz", config.sample_rate().0);
                        println!("     Channels: {}", config.channels());
                    }
                }
            }
        }
        Err(e) => eprintln!("Error listing input devices: {}", e),
    }
}

/// Get an input device by name, or the default if name is None
pub fn get_input_device(name: Option<&str>) -> Result<Device, InputError> {
    let host = cpal::default_host();

    match name {
        Some(device_name) => {
            let devices = host
                .input_devices()
                .map_err(|e| InputError::DeviceEnumeration(e.to_string()))?;

            for device in devices {
                if let Ok(n) = device.name() {
                    if n == device_name {
                        return Ok(device);
                    }
                }
            }
            Err(InputError::DeviceNotFound(device_name.to_string()))
        }
        None => host
            .default_input_device()
            .ok_or(InputError::NoDefaultDevice),
    }
}

/// Get the default input configuration for a device
pub fn get_input_config(device: &Device) -> Result<SupportedStreamConfig, InputError> {
    device
        .default_input_config()
        .map_err(|e| InputError::ConfigError(e.to_string()))
}

/// Ring buffer size calculation based on latency
/// Uses 4x the nominal latency to handle resampler chunk bursts and timing jitter
pub fn calculate_ring_buffer_size(sample_rate: u32, channels: usize, latency_ms: u32) -> usize {
    let samples_per_ms = sample_rate as usize / 1000;
    // 4x buffer provides headroom for resampler chunks and callback timing variations
    samples_per_ms * latency_ms as usize * channels * 4
}

/// Create a ring buffer pair for audio transfer
pub fn create_ring_buffer(size: usize) -> (HeapProducer<f32>, HeapConsumer<f32>) {
    let rb = HeapRb::<f32>::new(size);
    rb.split()
}

/// Input stream configuration
pub struct InputStreamConfig {
    pub device_name: Option<String>,
    pub latency_ms: u32,
}

impl Default for InputStreamConfig {
    fn default() -> Self {
        Self {
            device_name: None,
            latency_ms: 20, // 20ms default latency buffer
        }
    }
}

/// Active input stream with its ring buffer consumer
pub struct ActiveInput {
    #[allow(dead_code)] // Stream must be kept alive for audio to flow
    stream: Stream,
    consumer: Option<HeapConsumer<f32>>,
    pub channels: usize,
}

impl ActiveInput {
    /// Take ownership of the ring buffer consumer
    /// Returns None if already taken
    pub fn take_consumer(&mut self) -> Option<HeapConsumer<f32>> {
        self.consumer.take()
    }
}

/// Create an input stream with a ring buffer for audio transfer
/// Returns the stream and a consumer for reading audio samples
pub fn create_input_stream(
    config: InputStreamConfig,
    target_sample_rate: u32,
) -> Result<ActiveInput, InputError> {
    let device = get_input_device(config.device_name.as_deref())?;
    let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());
    let supported_config = get_input_config(&device)?;

    let input_sample_rate = supported_config.sample_rate().0;
    let channels = supported_config.channels() as usize;

    // Calculate ring buffer size based on OUTPUT sample rate (after potential resampling)
    let buffer_size = calculate_ring_buffer_size(target_sample_rate, channels, config.latency_ms);
    let (producer, consumer) = create_ring_buffer(buffer_size);

    // Check if we need resampling
    let needs_resampling = input_sample_rate != target_sample_rate;
    if needs_resampling {
        tracing::warn!(
            "Input device '{}' sample rate ({} Hz) differs from output ({} Hz) - resampling will add latency",
            device_name,
            input_sample_rate,
            target_sample_rate
        );
    }

    tracing::info!(
        "Opening input device: {} ({} Hz, {} channels, {}ms buffer)",
        device_name,
        input_sample_rate,
        channels,
        config.latency_ms
    );

    // Build the input stream
    let stream = if needs_resampling {
        // Create resampler for converting input to output sample rate
        create_resampling_input_stream(
            &device,
            supported_config,
            producer,
            input_sample_rate,
            target_sample_rate,
            channels,
        )?
    } else {
        // Direct passthrough - no resampling needed
        create_passthrough_input_stream(&device, supported_config, producer)?
    };

    stream
        .play()
        .map_err(|e| InputError::StreamError(e.to_string()))?;

    Ok(ActiveInput {
        stream,
        consumer: Some(consumer),
        channels,
    })
}

/// Create a passthrough input stream (no resampling)
fn create_passthrough_input_stream(
    device: &Device,
    config: SupportedStreamConfig,
    mut producer: HeapProducer<f32>,
) -> Result<Stream, InputError> {
    let stream = device
        .build_input_stream(
            &config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // Write samples to ring buffer
                let written = producer.push_slice(data);
                if written < data.len() {
                    // Ring buffer overflow - samples were dropped
                    tracing::debug!(
                        "Input buffer overflow: {} samples dropped",
                        data.len() - written
                    );
                }
            },
            move |err| {
                tracing::error!("Input stream error: {}", err);
            },
            None,
        )
        .map_err(|e| InputError::StreamError(e.to_string()))?;

    Ok(stream)
}

/// Create a resampling input stream
fn create_resampling_input_stream(
    device: &Device,
    config: SupportedStreamConfig,
    mut producer: HeapProducer<f32>,
    input_rate: u32,
    output_rate: u32,
    channels: usize,
) -> Result<Stream, InputError> {
    use rubato::{
        Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
    };

    // Create resampler
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };

    let resample_ratio = output_rate as f64 / input_rate as f64;

    // We need to handle resampling in the callback, but rubato's resampler isn't Send
    // So we'll use a simple approach: accumulate samples and resample in chunks
    let chunk_size = 1024; // Process in chunks
    let mut resampler = SincFixedIn::<f32>::new(
        resample_ratio,
        2.0, // Max relative ratio deviation
        params,
        chunk_size,
        channels,
    )
    .map_err(|e| InputError::ResamplerError(format!("{:?}", e)))?;

    // Buffer for accumulating input samples before resampling
    let mut input_buffer: Vec<Vec<f32>> = (0..channels)
        .map(|_| Vec::with_capacity(chunk_size * 2))
        .collect();

    let stream = device
        .build_input_stream(
            &config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // De-interleave input data into channel buffers
                for (i, sample) in data.iter().enumerate() {
                    let channel = i % channels;
                    input_buffer[channel].push(*sample);
                }

                // Process when we have enough samples
                while input_buffer[0].len() >= chunk_size {
                    // Extract chunk from each channel
                    let input_chunk: Vec<Vec<f32>> = input_buffer
                        .iter_mut()
                        .map(|ch| ch.drain(..chunk_size).collect())
                        .collect();

                    // Resample
                    match resampler.process(&input_chunk, None) {
                        Ok(output) => {
                            // Interleave and write to ring buffer
                            if !output.is_empty() && !output[0].is_empty() {
                                let num_frames = output[0].len();
                                let mut dropped = 0usize;
                                for frame_idx in 0..num_frames {
                                    for ch_buf in &output[..channels] {
                                        if producer.push(ch_buf[frame_idx]).is_err() {
                                            dropped += 1;
                                        }
                                    }
                                }
                                if dropped > 0 {
                                    tracing::debug!(
                                        "Resampler output overflow: {} samples dropped",
                                        dropped
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Resampling error: {:?}", e);
                        }
                    }
                }
            },
            move |err| {
                tracing::error!("Input stream error: {}", err);
            },
            None,
        )
        .map_err(|e| InputError::StreamError(e.to_string()))?;

    Ok(stream)
}

/// Error types for input operations
#[derive(Debug)]
pub enum InputError {
    DeviceEnumeration(String),
    DeviceNotFound(String),
    NoDefaultDevice,
    ConfigError(String),
    StreamError(String),
    ResamplerError(String),
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputError::DeviceEnumeration(e) => write!(f, "Failed to enumerate devices: {}", e),
            InputError::DeviceNotFound(name) => write!(f, "Input device not found: {}", name),
            InputError::NoDefaultDevice => write!(f, "No default input device available"),
            InputError::ConfigError(e) => write!(f, "Configuration error: {}", e),
            InputError::StreamError(e) => write!(f, "Stream error: {}", e),
            InputError::ResamplerError(e) => write!(f, "Resampler error: {}", e),
        }
    }
}

impl std::error::Error for InputError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_size_calculation() {
        // 48kHz, stereo, 20ms = 48 * 20 * 2 * 4 = 7680 samples (4x for headroom)
        let size = calculate_ring_buffer_size(48000, 2, 20);
        assert_eq!(size, 7680);

        // 44.1kHz, mono, 10ms = 44 * 10 * 1 * 4 = 1760 samples (4x for headroom)
        let size = calculate_ring_buffer_size(44100, 1, 10);
        assert_eq!(size, 1760);

        // 96kHz, 8 channels, 50ms = 96 * 50 * 8 * 4 = 153600 samples (4x for headroom)
        let size = calculate_ring_buffer_size(96000, 8, 50);
        assert_eq!(size, 153600);
    }

    #[test]
    fn test_ring_buffer_creation() {
        let (mut producer, mut consumer) = create_ring_buffer(1024);

        // Test write and read
        let data = vec![0.5_f32; 100];
        let written = producer.push_slice(&data);
        assert_eq!(written, 100);

        let mut output = vec![0.0_f32; 100];
        let read = consumer.pop_slice(&mut output);
        assert_eq!(read, 100);

        for sample in &output {
            assert_eq!(*sample, 0.5);
        }
    }

    #[test]
    fn test_ring_buffer_overflow() {
        let (mut producer, _consumer) = create_ring_buffer(100);

        // Fill the buffer
        let data = vec![1.0_f32; 100];
        let written = producer.push_slice(&data);
        assert_eq!(written, 100);

        // Try to write more - should fail
        let extra = vec![2.0_f32; 50];
        let written = producer.push_slice(&extra);
        assert_eq!(written, 0); // Buffer is full
    }

    #[test]
    fn test_ring_buffer_underflow() {
        let (_producer, mut consumer) = create_ring_buffer(100);

        // Try to read from empty buffer
        let mut output = vec![0.0_f32; 50];
        let read = consumer.pop_slice(&mut output);
        assert_eq!(read, 0); // Buffer is empty
    }

    #[test]
    fn test_default_input_stream_config() {
        let config = InputStreamConfig::default();

        assert!(config.device_name.is_none());
        assert_eq!(config.latency_ms, 20);
    }

    // Note: Device enumeration tests require actual hardware and are skipped in CI
    // The following tests are integration tests that need real devices:
    // - test_list_input_devices
    // - test_get_default_input_device
    // - test_create_input_stream
}
