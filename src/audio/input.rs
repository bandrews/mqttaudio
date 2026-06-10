// ABOUTME: Audio input device handling for microphone capture.
// ABOUTME: Manages input streams and routes audio to mixer via ring buffers.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, SupportedStreamConfig};
use ringbuf::{HeapConsumer, HeapProducer, HeapRb};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Capabilities of an input device, as far as they could be determined.
pub enum InputCaps {
    /// Capabilities probed from the device's supported configurations.
    Probed {
        max_channels: u16,
        /// Unique (min, max) sample-rate ranges across the supported configurations.
        sample_rates: Vec<(u32, u32)>,
    },
    /// All supported configurations were implausible (ALSA plugin devices report
    /// fake rates), so this reflects the default configuration instead.
    PluginFallback { channels: u16, sample_rate: u32 },
    /// Supported configurations could not be enumerated; this reflects the
    /// default configuration.
    DefaultOnly { channels: u16, sample_rate: u32 },
    /// No capability information could be obtained.
    Unknown,
}

/// An input device discovered by enumeration.
pub struct InputDeviceInfo {
    /// Device name (used for the `inputs[].device` config field)
    pub name: String,
    pub caps: InputCaps,
}

impl InputDeviceInfo {
    /// Channel count from whichever capability source was available, if any.
    pub fn channels(&self) -> Option<u16> {
        match self.caps {
            InputCaps::Probed { max_channels, .. } => Some(max_channels),
            InputCaps::PluginFallback { channels, .. } => Some(channels),
            InputCaps::DefaultOnly { channels, .. } => Some(channels),
            InputCaps::Unknown => None,
        }
    }
}

/// Enumerate available audio input devices with their capabilities.
/// Errors during enumeration are returned as Err with a description.
pub fn input_device_list() -> Result<Vec<InputDeviceInfo>, String> {
    let host = cpal::default_host();

    let devices = host.input_devices().map_err(|e| e.to_string())?;
    let mut list = Vec::new();
    for device in devices {
        let Ok(name) = device.description().map(|d| d.name().to_string()) else {
            continue;
        };

        // Query all supported configs to find max channels
        let caps = if let Ok(configs) = device.supported_input_configs() {
            let mut max_channels = 0u16;
            let mut sample_rates: Vec<(u32, u32)> = Vec::new();

            // Filter configs: ignore those with absurd sample rates
            // (ALSA plugins report 4294967295 Hz which is clearly fake)
            const MAX_REASONABLE_SAMPLE_RATE: u32 = 384000;

            for config in configs {
                let max_rate = config.max_sample_rate();
                // Skip configs from ALSA plugins that claim unrealistic capabilities
                if max_rate > MAX_REASONABLE_SAMPLE_RATE {
                    continue;
                }

                max_channels = max_channels.max(config.channels());
                let min_rate = config.min_sample_rate();
                // Collect unique sample rate ranges
                if !sample_rates
                    .iter()
                    .any(|(min, max)| *min == min_rate && *max == max_rate)
                {
                    sample_rates.push((min_rate, max_rate));
                }
            }

            if max_channels > 0 {
                InputCaps::Probed {
                    max_channels,
                    sample_rates,
                }
            } else if let Ok(config) = device.default_input_config() {
                // All configs were filtered out - fall back to default
                // This happens with ALSA plugin devices
                InputCaps::PluginFallback {
                    channels: config.channels(),
                    sample_rate: config.sample_rate(),
                }
            } else {
                InputCaps::Unknown
            }
        } else if let Ok(config) = device.default_input_config() {
            // Fallback to default config if supported_input_configs fails
            InputCaps::DefaultOnly {
                channels: config.channels(),
                sample_rate: config.sample_rate(),
            }
        } else {
            InputCaps::Unknown
        };

        list.push(InputDeviceInfo { name, caps });
    }
    Ok(list)
}

/// List available audio input devices
pub fn list_input_devices() {
    println!("Available audio input devices:");
    match input_device_list() {
        Ok(devices) => {
            for (i, device) in devices.iter().enumerate() {
                println!("  {}. {}", i, device.name);
                match &device.caps {
                    InputCaps::Probed {
                        max_channels,
                        sample_rates,
                    } => {
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
                    }
                    InputCaps::PluginFallback {
                        channels,
                        sample_rate,
                    } => {
                        println!("     Sample rate: {} Hz (plugin)", sample_rate);
                        println!("     Channels: {} (plugin)", channels);
                    }
                    InputCaps::DefaultOnly {
                        channels,
                        sample_rate,
                    } => {
                        println!("     Sample rate: {} Hz", sample_rate);
                        println!("     Channels: {}", channels);
                    }
                    InputCaps::Unknown => {}
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
                if let Ok(n) = device.description().map(|d| d.name().to_string()) {
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
    /// Capture-path counters (D57), drained/logged off-RT and surfaced on /metrics.
    pub telemetry: Arc<InputTelemetry>,
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
    let device_name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "Unknown".to_string());
    let supported_config = get_input_config(&device)?;

    let input_sample_rate = supported_config.sample_rate();
    let channels = supported_config.channels() as usize;

    // Calculate ring buffer size based on OUTPUT sample rate (after potential resampling)
    let buffer_size = calculate_ring_buffer_size(target_sample_rate, channels, config.latency_ms);
    let (producer, consumer) = create_ring_buffer(buffer_size);

    if input_sample_rate != target_sample_rate {
        tracing::warn!(
            "Input device '{}' sample rate ({} Hz) differs from output ({} Hz) - resampling will add latency",
            device_name,
            input_sample_rate,
            target_sample_rate
        );
    }

    let sample_format = supported_config.sample_format();
    tracing::info!(
        "Opening input device: {} ({} Hz, {} channels, {:?}, {}ms buffer)",
        device_name,
        input_sample_rate,
        channels,
        sample_format,
        config.latency_ms
    );

    // Every input runs through async sample-rate conversion so two independently
    // clocked devices stay drift-bounded, even at equal nominal rates (D33). The
    // device sample format is handled orthogonally: a typed callback converts the
    // native format to f32 before the resampler sees it (D37).
    let (stream, telemetry) = build_resampling_input_stream(
        &device,
        supported_config,
        sample_format,
        producer,
        input_sample_rate,
        target_sample_rate,
        channels,
    )?;

    stream
        .play()
        .map_err(|e| InputError::StreamError(e.to_string()))?;

    Ok(ActiveInput {
        stream,
        consumer: Some(consumer),
        channels,
        telemetry,
    })
}

/// Convert one block of typed cpal input samples to f32 and hand the f32 slice to
/// `forward`. `scratch` is reused across calls and grows only when a larger block
/// arrives, so in steady state (a stable cpal block size) this does no heap work.
pub fn convert_input_block<T>(data: &[T], scratch: &mut Vec<f32>, forward: impl FnOnce(&[f32]))
where
    T: cpal::Sample,
    f32: cpal::FromSample<T>,
{
    use cpal::Sample as _;
    scratch.resize(data.len(), 0.0);
    for (dst, &src) in scratch.iter_mut().zip(data.iter()) {
        *dst = f32::from_sample(src);
    }
    forward(scratch);
}

/// Which typed input handler a device sample format maps to. Mirrors Sprint 1's
/// output dispatch; any other format is unsupported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSampleHandling {
    F32,
    I16,
    U16,
    I32,
}

/// Map a cpal sample format to the typed input handler that converts it to f32,
/// or `None` if this app does not support capturing that format. This is the
/// single source of truth for the format dispatch in [`build_resampling_input_stream`].
pub fn input_stream_builder(format: cpal::SampleFormat) -> Option<InputSampleHandling> {
    use cpal::SampleFormat;
    match format {
        SampleFormat::F32 => Some(InputSampleHandling::F32),
        SampleFormat::I16 => Some(InputSampleHandling::I16),
        SampleFormat::U16 => Some(InputSampleHandling::U16),
        SampleFormat::I32 => Some(InputSampleHandling::I32),
        _ => None,
    }
}

/// Number of input frames the resampler consumes per chunk.
const RESAMPLE_CHUNK_SIZE: usize = 1024;

/// Maximum ratio deviation the steered resampler may apply relative to the
/// nominal ratio. The `SincFixedIn` constructor's `max_resample_ratio_relative`
/// is set generously (the resampler sizes its internal buffers from it), while
/// the control loop itself only ever nudges within [`STEER_MAX_DEVIATION`].
const RESAMPLE_MAX_RELATIVE: f64 = 2.0;

/// Largest fractional deviation from the nominal ratio the drift-control loop is
/// allowed to command. Drift between two device clocks is tens of ppm, so a 2%
/// authority is far more than enough to track it while staying well inside
/// `RESAMPLE_MAX_RELATIVE` and small enough to be inaudible as pitch wobble.
const STEER_MAX_DEVIATION: f64 = 0.02;

/// Proportional gain of the drift-control loop, applied to the normalized fill
/// error (where ±1.0 spans empty..full). Kept small so the ratio moves gently.
const STEER_GAIN: f64 = 0.05;

/// Smoothing factor for the measured ring-buffer fill. The instantaneous fill
/// jitters by a chunk each time the resampler emits; this EMA rejects that so the
/// loop steers on the slow trend, not the burst.
const STEER_FILL_SMOOTHING: f64 = 0.05;

/// Drives a `SincFixedIn` resampler from interleaved capture frames into a ring
/// buffer, steering the resample ratio from the measured ring fill so two
/// independently-clocked devices stay drift-bounded (D33). All working storage is
/// pre-allocated so [`resample_block`] does no heap work on the RT capture thread.
/// Capture-path telemetry counters (D57). The capture callback only does relaxed
/// `fetch_add`s here — never `tracing`, whose cost depends on the installed
/// subscriber — and the control thread drains/logs deltas off-RT and surfaces
/// totals on `/metrics`.
#[derive(Default)]
pub struct InputTelemetry {
    /// Resampler `process_into_buffer` failures (the chunk was dropped).
    pub resample_errors: std::sync::atomic::AtomicU64,
    /// Interleaved samples dropped because the ring was full (overflow).
    pub overflow_dropped_samples: std::sync::atomic::AtomicU64,
    /// `set_resample_ratio` rejections from the drift-control loop.
    pub ratio_rejects: std::sync::atomic::AtomicU64,
    /// Capture blocks larger than the pre-sized conversion scratch (D58): each
    /// one cost a reallocation on the capture thread. Persistently non-zero means
    /// the device delivers blocks beyond its advertised maximum.
    pub scratch_regrows: std::sync::atomic::AtomicU64,
}

pub struct ResampleState {
    resampler: rubato::SincFixedIn<f32>,
    channels: usize,
    /// Per-channel de-interleave accumulators, each pre-sized to one full chunk.
    deinterleave: Vec<Vec<f32>>,
    /// Frames currently accumulated in `deinterleave` (0..RESAMPLE_CHUNK_SIZE).
    filled: usize,
    /// Reusable resampler output buffer from `output_buffer_allocate(true)`.
    output: Vec<Vec<f32>>,
    /// Nominal output/input ratio; the steered ratio is a small deviation of this.
    nominal_ratio: f64,
    /// Capacity of the destination ring buffer in samples (interleaved).
    ring_capacity: usize,
    /// Smoothed ring fill in interleaved samples; `-1.0` until first measured.
    smoothed_fill: f64,
    /// Most recent ratio commanded by the drift-control loop (== nominal until
    /// the first steer). Exposed for tests/observability via [`ResampleState::current_ratio`].
    last_ratio: f64,
    /// Capture-path counters (D57), shared with the control thread.
    telemetry: Arc<InputTelemetry>,
}

impl ResampleState {
    /// The capture-path telemetry counters this state increments (D57). The
    /// builder hands the clone to the control thread for draining and `/metrics`.
    pub fn telemetry(&self) -> Arc<InputTelemetry> {
        Arc::clone(&self.telemetry)
    }

    /// Build a resampler+accumulators sized for `channels` and the given nominal
    /// `output_rate / input_rate` ratio, feeding a ring of `ring_capacity`
    /// interleaved samples. All buffers are allocated here, off the RT thread.
    pub fn new(
        input_rate: u32,
        output_rate: u32,
        channels: usize,
        ring_capacity: usize,
    ) -> Result<Self, InputError> {
        use rubato::{
            Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType,
            WindowFunction,
        };

        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };

        let nominal_ratio = output_rate as f64 / input_rate as f64;

        let resampler = SincFixedIn::<f32>::new(
            nominal_ratio,
            RESAMPLE_MAX_RELATIVE,
            params,
            RESAMPLE_CHUNK_SIZE,
            channels,
        )
        .map_err(|e| InputError::ResamplerError(format!("{:?}", e)))?;

        // Pre-size de-interleave accumulators to exactly one chunk per channel.
        let deinterleave = vec![vec![0.0f32; RESAMPLE_CHUNK_SIZE]; channels];
        // Reusable output buffer with capacity for the largest possible chunk.
        let output = resampler.output_buffer_allocate(true);

        Ok(Self {
            resampler,
            channels,
            deinterleave,
            filled: 0,
            output,
            nominal_ratio,
            ring_capacity,
            smoothed_fill: -1.0,
            last_ratio: nominal_ratio,
            telemetry: Arc::new(InputTelemetry::default()),
        })
    }

    /// The ratio the drift-control loop last commanded (output frames per input
    /// frame). Settles toward the true `r_out / r_in` of the two device clocks.
    #[allow(dead_code)] // Observability/testing accessor; not read on the hot path.
    pub fn current_ratio(&self) -> f64 {
        self.last_ratio
    }

    /// The nominal `output_rate / input_rate` ratio the resampler was built with.
    #[allow(dead_code)] // Observability/testing accessor; not read on the hot path.
    pub fn nominal_ratio(&self) -> f64 {
        self.nominal_ratio
    }
}

/// Resample one block of interleaved capture `data` into `producer`.
///
/// RT-safe: pre-sized accumulators receive de-interleaved samples by index (no
/// `Vec` growth), `process_into_buffer` writes into a reusable output buffer (no
/// allocation), and the ratio is steered toward a half-full ring via
/// `set_resample_ratio` (which only updates two floats). Nothing here allocates,
/// frees, or locks.
pub fn resample_block(state: &mut ResampleState, data: &[f32], producer: &mut HeapProducer<f32>) {
    use rubato::Resampler;

    let channels = state.channels;
    // cpal delivers whole frames; a non-frame-aligned block would desync the
    // de-interleave below (F6). This cannot trigger in release in practice.
    debug_assert!(
        data.len().is_multiple_of(channels),
        "capture block not frame-aligned: {} samples, {} channels",
        data.len(),
        channels
    );

    let mut offset = 0;
    while offset < data.len() {
        // Fill the accumulators frame-by-frame up to one chunk.
        let frames_in_data = (data.len() - offset) / channels;
        let room = RESAMPLE_CHUNK_SIZE - state.filled;
        let take = frames_in_data.min(room);
        for f in 0..take {
            let base = offset + f * channels;
            for ch in 0..channels {
                state.deinterleave[ch][state.filled + f] = data[base + ch];
            }
        }
        state.filled += take;
        offset += take * channels;

        if state.filled < RESAMPLE_CHUNK_SIZE {
            // Not enough for a chunk yet; wait for the next callback.
            break;
        }

        // Steer the ratio toward a half-full ring before processing this chunk.
        steer_ratio(state, producer);

        // Resample one full chunk into the reusable output buffer.
        let (_in_frames, out_frames) =
            match state
                .resampler
                .process_into_buffer(&state.deinterleave, &mut state.output, None)
            {
                Ok(counts) => counts,
                Err(_) => {
                    // A relaxed counter, never tracing, on the capture thread
                    // (D57); the control thread logs the delta off-RT.
                    state
                        .telemetry
                        .resample_errors
                        .fetch_add(1, Ordering::Relaxed);
                    state.filled = 0;
                    continue;
                }
            };
        state.filled = 0;

        // Interleave the produced frames into the ring buffer.
        let mut dropped = 0usize;
        for frame_idx in 0..out_frames {
            for ch in 0..channels {
                if producer.push(state.output[ch][frame_idx]).is_err() {
                    dropped += 1;
                }
            }
        }
        if dropped > 0 {
            state
                .telemetry
                .overflow_dropped_samples
                .fetch_add(dropped as u64, Ordering::Relaxed);
        }
    }
}

/// Nudge the resampler ratio toward keeping the ring buffer ~half-full (D33).
///
/// Two independently-clocked devices drift; left alone the ring monotonically
/// fills or drains until it clicks (overflow) or starves (underrun silence). A
/// slow proportional loop on the smoothed fill error corrects that: when the
/// ring runs above half-full the consumer is slower than the producer, so we
/// reduce the output ratio (emit fewer frames) and vice versa. The commanded
/// deviation is clamped to [`STEER_MAX_DEVIATION`], well inside the resampler's
/// `RESAMPLE_MAX_RELATIVE`, so it is inaudible as pitch.
fn steer_ratio(state: &mut ResampleState, producer: &HeapProducer<f32>) {
    use rubato::Resampler;

    if state.ring_capacity == 0 {
        return;
    }

    let fill = producer.len() as f64;
    state.smoothed_fill = if state.smoothed_fill < 0.0 {
        fill
    } else {
        state.smoothed_fill + STEER_FILL_SMOOTHING * (fill - state.smoothed_fill)
    };

    let half = state.ring_capacity as f64 / 2.0;
    // Normalized error: +1.0 when full, -1.0 when empty.
    let err = (state.smoothed_fill - half) / half;
    // Above half-full => slow the output (ratio < nominal); below => speed it up.
    let deviation = (-STEER_GAIN * err).clamp(-STEER_MAX_DEVIATION, STEER_MAX_DEVIATION);
    let new_ratio = state.nominal_ratio * (1.0 + deviation);

    // Ramp so the change is spread across the chunk (no per-chunk step in pitch).
    match state.resampler.set_resample_ratio(new_ratio, true) {
        Ok(()) => state.last_ratio = new_ratio,
        Err(_) => {
            state
                .telemetry
                .ratio_rejects
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Build the async-SRC input stream, dispatching on the device sample format so
/// a non-f32 device opens (D37). The format choice is orthogonal to resampling:
/// the typed callback converts the native samples to f32, then the same
/// [`resample_block`] drives the drift-steered resampler regardless of format.
fn build_resampling_input_stream(
    device: &Device,
    config: SupportedStreamConfig,
    sample_format: cpal::SampleFormat,
    producer: HeapProducer<f32>,
    input_rate: u32,
    output_rate: u32,
    channels: usize,
) -> Result<(Stream, Arc<InputTelemetry>), InputError> {
    match input_stream_builder(sample_format) {
        Some(InputSampleHandling::F32) => build_typed_resampling_input_stream::<f32>(
            device,
            config,
            producer,
            input_rate,
            output_rate,
            channels,
        ),
        Some(InputSampleHandling::I16) => build_typed_resampling_input_stream::<i16>(
            device,
            config,
            producer,
            input_rate,
            output_rate,
            channels,
        ),
        Some(InputSampleHandling::U16) => build_typed_resampling_input_stream::<u16>(
            device,
            config,
            producer,
            input_rate,
            output_rate,
            channels,
        ),
        Some(InputSampleHandling::I32) => build_typed_resampling_input_stream::<i32>(
            device,
            config,
            producer,
            input_rate,
            output_rate,
            channels,
        ),
        None => Err(InputError::UnsupportedFormat(format!(
            "{:?}",
            sample_format
        ))),
    }
}

/// Build the async-SRC input stream for a specific native sample type `T`. The
/// callback converts `T` to f32 into a reused scratch buffer and feeds it to the
/// resampler. RT-safe in steady state: the scratch grows only on the first (or a
/// larger) block, and `resample_block` itself does no heap work.
fn build_typed_resampling_input_stream<T>(
    device: &Device,
    config: SupportedStreamConfig,
    mut producer: HeapProducer<f32>,
    input_rate: u32,
    output_rate: u32,
    channels: usize,
) -> Result<(Stream, Arc<InputTelemetry>), InputError>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let ring_capacity = producer.capacity();
    let mut state = ResampleState::new(input_rate, output_rate, channels, ring_capacity)?;
    let telemetry = state.telemetry();
    let callback_telemetry = Arc::clone(&telemetry);

    // Pre-size the conversion scratch to the largest block the device says it can
    // deliver (D58), clamped to a sane cap; `Unknown` gets the cap. The resize in
    // `convert_input_block` then never reallocates for in-range blocks, and an
    // out-of-range block is counted instead of silently reallocating.
    const MAX_PRESIZE_FRAMES: usize = 8192;
    let max_block_frames = match config.buffer_size() {
        cpal::SupportedBufferSize::Range { max, .. } => (*max as usize).min(MAX_PRESIZE_FRAMES),
        cpal::SupportedBufferSize::Unknown => MAX_PRESIZE_FRAMES,
    };
    let mut scratch: Vec<f32> = Vec::with_capacity(max_block_frames.max(1) * channels);

    let stream = device
        .build_input_stream(
            &config.into(),
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                if data.len() > scratch.capacity() {
                    // Pathological device: a block beyond the advertised maximum
                    // is about to regrow the scratch on the capture thread (D58).
                    callback_telemetry
                        .scratch_regrows
                        .fetch_add(1, Ordering::Relaxed);
                }
                convert_input_block::<T>(data, &mut scratch, |f32s| {
                    resample_block(&mut state, f32s, &mut producer);
                });
            },
            move |err| {
                tracing::error!("Input stream error: {}", err);
            },
            None,
        )
        .map_err(|e| InputError::StreamError(e.to_string()))?;

    Ok((stream, telemetry))
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
    UnsupportedFormat(String),
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
            InputError::UnsupportedFormat(fmt) => {
                write!(f, "Unsupported input sample format: {}", fmt)
            }
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
