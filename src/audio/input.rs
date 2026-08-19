// ABOUTME: Audio input device handling for microphone capture.
// ABOUTME: Manages input streams and routes audio to mixer via ring buffers.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SampleRate, Stream, SupportedStreamConfig};
use ringbuf::{HeapRb, HeapConsumer, HeapProducer};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// ALSA plugin devices advertise absurd rates (4294967295 Hz); anything above
/// this is treated as a bogus capability report.
const MAX_REASONABLE_SAMPLE_RATE: u32 = 384000;

/// Upper bound on channel counts accepted from device capability reports
const MAX_REASONABLE_CHANNELS: u16 = 64;

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

                        for config in configs {
                            let max_rate = config.max_sample_rate().0;
                            // Skip configs from ALSA plugins that claim unrealistic capabilities
                            if max_rate > MAX_REASONABLE_SAMPLE_RATE {
                                continue;
                            }

                            max_channels = max_channels.max(config.channels());
                            let min_rate = config.min_sample_rate().0;
                            // Collect unique sample rate ranges
                            if !sample_rates.iter().any(|(min, max)| *min == min_rate && *max == max_rate) {
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
///
/// Falls back to ALSA card matching so that names written by hand
/// ("hw:CARD=UMC1820, DEV=0") still resolve to the enumerated device, the same
/// way output devices are resolved.
pub fn get_input_device(name: Option<&str>) -> Result<Device, InputError> {
    let host = cpal::default_host();

    match name {
        Some(device_name) => {
            let devices: Vec<Device> = host.input_devices()
                .map_err(|e| InputError::DeviceEnumeration(e.to_string()))?
                .collect();

            for device in &devices {
                if let Ok(n) = device.name() {
                    if n == device_name {
                        return Ok(device.clone());
                    }
                }
            }

            #[cfg(target_os = "linux")]
            {
                for device in &devices {
                    if let Ok(n) = device.name() {
                        if crate::audio::device::try_match_alsa_device(device_name, &n)
                            == Some(true)
                        {
                            tracing::info!("Matched ALSA input device '{}' to '{}'", device_name, n);
                            return Ok(device.clone());
                        }
                    }
                }
            }

            Err(InputError::DeviceNotFound(device_name.to_string()))
        }
        None => {
            host.default_input_device()
                .ok_or_else(|| InputError::NoDefaultDevice)
        }
    }
}

/// Lowest input channel count that can serve every routed source channel
pub fn required_channels(channel_map: &[(usize, usize)]) -> usize {
    channel_map.iter().map(|(src, _)| src + 1).max().unwrap_or(0)
}

/// Choose the channel count to open a capture stream with.
///
/// `exact` forces a specific count, for hardware whose capabilities cpal
/// reports incorrectly. Otherwise the smallest supported count that covers
/// every routed source channel is used, so a device offering a range is not
/// opened wider than the routing needs while a device with a fixed layout
/// (ALSA `hw:` on a multichannel interface) still matches. Returns None when
/// no supported count satisfies the request.
pub fn select_channel_count(available: &[u16], exact: Option<usize>, minimum: usize) -> Option<u16> {
    match exact {
        Some(requested) => available
            .iter()
            .copied()
            .find(|&ch| ch as usize == requested),
        None => available
            .iter()
            .copied()
            .filter(|&ch| ch as usize >= minimum.max(1))
            .min(),
    }
}

/// Find a capture configuration for the device.
///
/// Only f32 configurations are considered: the capture callback reads f32
/// samples, and ALSA `hw:` devices that expose integer formats only must be
/// opened through their `plughw:` alias instead.
pub fn find_input_config(
    device: &Device,
    requested_channels: Option<usize>,
    minimum_channels: usize,
    preferred_sample_rate: u32,
) -> Result<SupportedStreamConfig, InputError> {
    let supported: Vec<_> = device
        .supported_input_configs()
        .map_err(|e| InputError::ConfigError(e.to_string()))?
        .filter(|c| c.max_sample_rate().0 <= MAX_REASONABLE_SAMPLE_RATE)
        .filter(|c| c.channels() <= MAX_REASONABLE_CHANNELS)
        .collect();

    // Plugin devices often enumerate nothing usable; fall back to whatever the
    // device reports as its default and let the stream build succeed or fail.
    if supported.is_empty() {
        return get_input_config(device);
    }

    let float_configs: Vec<_> = supported
        .iter()
        .filter(|c| c.sample_format() == SampleFormat::F32)
        .collect();

    if float_configs.is_empty() {
        let formats: Vec<String> = supported
            .iter()
            .map(|c| format!("{:?}", c.sample_format()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        return Err(InputError::ConfigError(format!(
            "device offers no f32 capture format (has: {}); use the 'plughw:' alias for this card",
            formats.join(", ")
        )));
    }

    let available: Vec<u16> = float_configs.iter().map(|c| c.channels()).collect();
    let channels = select_channel_count(&available, requested_channels, minimum_channels)
        .ok_or_else(|| {
            let mut counts: Vec<u16> = available.clone();
            counts.sort_unstable();
            counts.dedup();
            match requested_channels {
                Some(ch) => InputError::ConfigError(format!(
                    "device does not support {} capture channels (supports: {:?})",
                    ch, counts
                )),
                None => InputError::ConfigError(format!(
                    "routing needs {} capture channels but device supports at most {:?}",
                    minimum_channels, counts
                )),
            }
        })?;

    let matching: Vec<_> = float_configs
        .iter()
        .filter(|c| c.channels() == channels)
        .collect();

    // Matching the output rate keeps the resampler out of the signal path
    let exact_rate = matching.iter().find(|c| {
        c.min_sample_rate().0 <= preferred_sample_rate
            && preferred_sample_rate <= c.max_sample_rate().0
    });

    match exact_rate {
        Some(c) => Ok(c.with_sample_rate(SampleRate(preferred_sample_rate))),
        None => {
            let closest = matching[0];
            let rate = preferred_sample_rate
                .max(closest.min_sample_rate().0)
                .min(closest.max_sample_rate().0);
            Ok(closest.with_sample_rate(SampleRate(rate)))
        }
    }
}

/// Push whole frames into the ring buffer, returning the number of frames written.
///
/// A partially written frame would permanently shift the channel interleaving
/// seen by the mixer, sending each microphone to the wrong output and leaving a
/// residue that grows until the buffer is full. Frames that do not fit are
/// counted as dropped instead.
fn push_frames(
    producer: &mut HeapProducer<f32>,
    data: &[f32],
    channels: usize,
    dropped_frames: &AtomicU64,
) -> usize {
    if channels == 0 {
        return 0;
    }

    let frames_in = data.len() / channels;
    let frames_free = producer.free_len() / channels;
    let frames_out = frames_in.min(frames_free);

    if frames_out > 0 {
        producer.push_slice(&data[..frames_out * channels]);
    }
    if frames_out < frames_in {
        dropped_frames.fetch_add((frames_in - frames_out) as u64, Ordering::Relaxed);
    }

    frames_out
}

/// Get the default input configuration for a device
pub fn get_input_config(device: &Device) -> Result<SupportedStreamConfig, InputError> {
    device.default_input_config()
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
    /// Exact channel count to open (None = smallest count covering `min_channels`)
    pub channels: Option<usize>,
    /// Lowest channel count the configured routing needs
    pub min_channels: usize,
    /// Capture rate to request (None = match the output rate when supported)
    pub sample_rate: Option<u32>,
}

impl Default for InputStreamConfig {
    fn default() -> Self {
        Self {
            device_name: None,
            latency_ms: 20, // 20ms default latency buffer
            channels: None,
            min_channels: 1,
            sample_rate: None,
        }
    }
}

/// Active input stream with its ring buffer consumer
pub struct ActiveInput {
    #[allow(dead_code)] // Stream must be kept alive for audio to flow
    stream: Stream,
    consumer: Option<HeapConsumer<f32>>,
    pub channels: usize,
    pub sample_rate: u32,
    /// Frames the capture callback could not hand over because the ring buffer
    /// was full. Shared with the mixer so input health can be reported.
    pub dropped_frames: Arc<AtomicU64>,
    /// Peak absolute sample level (as f32 bits) seen since the last reader
    /// reset it. Written lock-free by the capture callback; the activity
    /// detector swaps it back to zero on each poll.
    pub peak_level: Arc<AtomicU32>,
}

/// Fold a chunk's peak absolute level into the shared atomic.
/// Non-negative f32 bit patterns order like the floats themselves, so
/// fetch_max on the bits is a lock-free running maximum.
fn update_peak_level(peak_level: &AtomicU32, data: &[f32]) {
    let mut max = 0.0f32;
    for sample in data {
        let a = sample.abs();
        if a > max {
            max = a;
        }
    }
    peak_level.fetch_max(max.to_bits(), Ordering::Relaxed);
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

    let preferred_rate = config.sample_rate.unwrap_or(target_sample_rate);
    let supported_config = find_input_config(
        &device,
        config.channels,
        config.min_channels,
        preferred_rate,
    )?;

    let input_sample_rate = supported_config.sample_rate().0;
    let channels = supported_config.channels() as usize;

    if channels == 0 {
        return Err(InputError::ConfigError(format!(
            "device '{}' reported zero capture channels",
            device_name
        )));
    }

    if channels < config.min_channels {
        return Err(InputError::ConfigError(format!(
            "routing needs {} capture channels but '{}' opened with {}",
            config.min_channels, device_name, channels
        )));
    }

    // Calculate ring buffer size based on OUTPUT sample rate (after potential resampling)
    let buffer_size = calculate_ring_buffer_size(target_sample_rate, channels, config.latency_ms);
    let (producer, consumer) = create_ring_buffer(buffer_size);
    let dropped_frames = Arc::new(AtomicU64::new(0));
    let peak_level = Arc::new(AtomicU32::new(0));

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
            dropped_frames.clone(),
            peak_level.clone(),
        )?
    } else {
        // Direct passthrough - no resampling needed
        create_passthrough_input_stream(
            &device,
            supported_config,
            producer,
            channels,
            dropped_frames.clone(),
            peak_level.clone(),
        )?
    };

    stream.play().map_err(|e| InputError::StreamError(e.to_string()))?;

    Ok(ActiveInput {
        stream,
        consumer: Some(consumer),
        channels,
        sample_rate: input_sample_rate,
        dropped_frames,
        peak_level,
    })
}

/// Create a passthrough input stream (no resampling)
///
/// The callback runs on the capture thread and must not allocate, lock, or log:
/// stalling it costs whole capture periods and causes the very overruns it
/// would be reporting. Dropped frames are counted for the caller to report.
fn create_passthrough_input_stream(
    device: &Device,
    config: SupportedStreamConfig,
    mut producer: HeapProducer<f32>,
    channels: usize,
    dropped_frames: Arc<AtomicU64>,
    peak_level: Arc<AtomicU32>,
) -> Result<Stream, InputError> {
    let stream = device.build_input_stream(
        &config.into(),
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            update_peak_level(&peak_level, data);
            push_frames(&mut producer, data, channels, &dropped_frames);
        },
        move |err| {
            tracing::error!("Input stream error: {}", err);
        },
        None,
    ).map_err(|e| InputError::StreamError(e.to_string()))?;

    Ok(stream)
}

/// Create a resampling input stream
///
/// All working buffers are allocated up front: the callback runs on the capture
/// thread, where an allocation or a log write costs capture periods and causes
/// overruns.
#[allow(clippy::too_many_arguments)]
fn create_resampling_input_stream(
    device: &Device,
    config: SupportedStreamConfig,
    mut producer: HeapProducer<f32>,
    input_rate: u32,
    output_rate: u32,
    channels: usize,
    dropped_frames: Arc<AtomicU64>,
    peak_level: Arc<AtomicU32>,
) -> Result<Stream, InputError> {
    use rubato::{SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction, Resampler};

    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };

    let resample_ratio = output_rate as f64 / input_rate as f64;

    // Resample in fixed-size chunks so the resampler's own buffers stay fixed
    let chunk_size = 1024;
    let mut resampler = SincFixedIn::<f32>::new(
        resample_ratio,
        2.0, // Max relative ratio deviation
        params,
        chunk_size,
        channels,
    ).map_err(|e| InputError::ResamplerError(format!("{:?}", e)))?;

    let mut resampler_input = resampler.input_buffer_allocate(true);
    let mut resampler_output = resampler.output_buffer_allocate(true);
    let max_output_frames = resampler_output.first().map_or(0, |ch| ch.len());

    // Deinterleaved samples accumulated until a full chunk is available
    let mut pending: Vec<Vec<f32>> = (0..channels)
        .map(|_| Vec::with_capacity(chunk_size * 2))
        .collect();

    // Interleaved resampler output, staged here before going into the ring buffer
    let mut interleaved = vec![0.0f32; max_output_frames * channels];

    let stream = device.build_input_stream(
        &config.into(),
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            update_peak_level(&peak_level, data);
            for (i, sample) in data.iter().enumerate() {
                pending[i % channels].push(*sample);
            }

            while pending[0].len() >= chunk_size {
                for ch in 0..channels {
                    resampler_input[ch].clear();
                    resampler_input[ch].extend(pending[ch].drain(..chunk_size));
                }

                let frames_out = match resampler
                    .process_into_buffer(&resampler_input, &mut resampler_output, None)
                {
                    Ok((_, frames_out)) => frames_out,
                    Err(_) => {
                        // Count the chunk as lost rather than logging from the
                        // capture thread
                        dropped_frames.fetch_add(chunk_size as u64, Ordering::Relaxed);
                        continue;
                    }
                };

                for frame_idx in 0..frames_out {
                    for ch in 0..channels {
                        interleaved[frame_idx * channels + ch] = resampler_output[ch][frame_idx];
                    }
                }

                push_frames(
                    &mut producer,
                    &interleaved[..frames_out * channels],
                    channels,
                    &dropped_frames,
                );
            }
        },
        move |err| {
            tracing::error!("Input stream error: {}", err);
        },
        None,
    ).map_err(|e| InputError::StreamError(e.to_string()))?;

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

    #[test]
    fn test_push_frames_writes_only_whole_frames() {
        // Room for 2 whole 4-channel frames plus 2 stray samples. Writing the
        // stray samples would rotate the channel order for every later read.
        let dropped = Arc::new(AtomicU64::new(0));
        let (mut producer, consumer) = create_ring_buffer(10);

        let data = vec![1.0f32; 16]; // 4 frames
        let written = push_frames(&mut producer, &data, 4, &dropped);

        assert_eq!(written, 2, "only whole frames are written");
        assert_eq!(consumer.len(), 8, "ring buffer holds whole frames only");
        assert_eq!(dropped.load(Ordering::Relaxed), 2, "two frames did not fit");
    }

    #[test]
    fn test_push_frames_writes_everything_when_it_fits() {
        let dropped = Arc::new(AtomicU64::new(0));
        let (mut producer, consumer) = create_ring_buffer(64);

        let data = vec![0.25f32; 16]; // 4 frames of 4 channels
        let written = push_frames(&mut producer, &data, 4, &dropped);

        assert_eq!(written, 4);
        assert_eq!(consumer.len(), 16);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_push_frames_keeps_alignment_when_saturated() {
        // A device that keeps producing while the reader is stalled must never
        // leave the buffer holding a partial frame.
        let dropped = Arc::new(AtomicU64::new(0));
        let (mut producer, consumer) = create_ring_buffer(27);
        let channels = 5;
        let data = vec![1.0f32; 5 * channels];

        for _ in 0..10 {
            push_frames(&mut producer, &data, channels, &dropped);
            assert_eq!(
                consumer.len() % channels, 0,
                "ring buffer must always hold a whole number of frames"
            );
        }

        assert!(dropped.load(Ordering::Relaxed) > 0, "saturation should be counted");
    }

    #[test]
    fn test_push_frames_ignores_trailing_partial_input() {
        let dropped = Arc::new(AtomicU64::new(0));
        let (mut producer, consumer) = create_ring_buffer(64);

        let data = vec![0.5f32; 14]; // 3 whole 4-channel frames plus 2 samples
        let written = push_frames(&mut producer, &data, 4, &dropped);

        assert_eq!(written, 3);
        assert_eq!(consumer.len(), 12);
    }

    #[test]
    fn test_select_channel_count_uses_smallest_that_covers_routing() {
        // An interface offering a range should not be opened wider than the
        // routing needs.
        assert_eq!(select_channel_count(&[1, 2, 8, 18], None, 3), Some(8));
        assert_eq!(select_channel_count(&[1, 2, 8, 18], None, 1), Some(1));
    }

    #[test]
    fn test_select_channel_count_accepts_fixed_hardware_layout() {
        // ALSA hw: devices expose only their native layout
        assert_eq!(select_channel_count(&[18, 20], None, 2), Some(18));
        assert_eq!(select_channel_count(&[18, 20], None, 19), Some(20));
    }

    #[test]
    fn test_select_channel_count_rejects_unreachable_routing() {
        assert_eq!(select_channel_count(&[1, 2], None, 4), None);
        assert_eq!(select_channel_count(&[], None, 1), None);
    }

    #[test]
    fn test_select_channel_count_honours_explicit_request() {
        assert_eq!(select_channel_count(&[2, 8, 18], Some(8), 1), Some(8));
        assert_eq!(select_channel_count(&[2, 8, 18], Some(4), 1), None);
    }

    #[test]
    fn test_required_channels_covers_highest_routed_source() {
        assert_eq!(required_channels(&[(0, 0), (17, 3), (2, 1)]), 18);
        assert_eq!(required_channels(&[]), 0);
    }

    // Note: Device enumeration tests require actual hardware and are skipped in CI
    // The following tests are integration tests that need real devices:
    // - test_list_input_devices
    // - test_get_default_input_device
    // - test_create_input_stream
}
