// ABOUTME: Audio input device handling for microphone capture.
// ABOUTME: Manages input streams and routes audio to mixer via ring buffers.

use crate::config::ResamplerQuality;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, SupportedStreamConfig};
use std::sync::atomic::{AtomicU32, AtomicU64};
const MAX_REASONABLE_SAMPLE_RATE: u32 = 384000;
const MAX_REASONABLE_CHANNELS: u16 = 64;
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
        let Ok(name) = device.description().map(|d| {
            super::device::output_device_identifier(&d, cfg!(target_os = "linux")).to_string()
        }) else {
            continue;
        };

        // Query all supported configs to find max channels
        if name == "null" {
            continue;
        }
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
            let count = devices.len();
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
            if count == 0 {
                println!("  (none — devices that cannot be opened for capture right now,");
                println!("   because they are busy or inaccessible to this user, are not listed)");
            }
        }
        Err(e) => eprintln!("Error listing input devices: {}", e),
    }
}

/// Interpret a configured device name as an index into the enumerated input
/// device list, matching the numbering printed by --list-inputs. Only whole
/// numbers within range qualify.
fn parse_device_index(requested: &str, device_count: usize) -> Option<usize> {
    let index: usize = requested.trim().parse().ok()?;
    if index < device_count {
        Some(index)
    } else {
        None
    }
}

/// Build the device-not-found message: what was requested, which devices could
/// be opened for capture at that moment, and any direct-ALSA probe diagnosis.
/// Enumeration only yields devices that open for capture, so a device that is
/// busy or inaccessible is invisible here even though --list-inputs may have
/// shown it earlier.
fn device_not_found_message(requested: &str, available: &[String], probe: Option<&str>) -> String {
    let mut msg = format!("Input device not found: {}", requested);
    if available.is_empty() {
        msg.push_str(
            "\n  No devices could be opened for capture at this moment; \
             busy or inaccessible devices are not enumerable.",
        );
    } else {
        msg.push_str(&format!(
            "\n  Devices currently available for capture: {}",
            available.join(", ")
        ));
    }
    if let Some(probe) = probe {
        msg.push_str(&format!("\n  {}", probe));
    }
    msg
}

/// Ask ALSA directly why a capture open of this name fails, to explain why a
/// device is missing from enumeration (which silently skips any device that
/// cannot be opened for capture at that moment).
#[cfg(target_os = "linux")]
fn probe_capture_open(name: &str) -> Option<String> {
    use alsa::pcm::PCM;
    use alsa::Direction;

    match PCM::new(name, Direction::Capture, true) {
        Ok(_) => Some(format!(
            "ALSA opens '{}' for capture directly, but its configurations were \
             rejected during enumeration; its native sample format may be \
             unsupported. Try the plughw: form of the name.",
            name
        )),
        Err(e) => {
            let advice = match e.errno() {
                libc::EBUSY => {
                    "another process holds this device for capture (a sound \
                     server such as PipeWire or PulseAudio, or another capture \
                     application)"
                }
                libc::EACCES | libc::EPERM => {
                    "permission denied; when running as a service, check that \
                     the service user is in the 'audio' group"
                }
                libc::ENOENT | libc::ENODEV | libc::ENXIO => {
                    "no ALSA device has this name; compare against 'arecord -L'"
                }
                _ => "see the ALSA error for details",
            };
            Some(format!(
                "Direct ALSA capture open of '{}' failed: {} — {}",
                name, e, advice
            ))
        }
    }
}

/// Decide which enumerated input device a configured name selects: the exact
/// name first, then the --list-inputs index, then an ALSA card match.
/// Returns the position in the enumerated name list.
fn resolve_requested_device(requested: &str, names: &[String]) -> Option<usize> {
    if let Some(pos) = names.iter().position(|n| n == requested) {
        return Some(pos);
    }
    if let Some(index) = parse_device_index(requested, names.len()) {
        return Some(index);
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(pos) = names
            .iter()
            .position(|n| crate::audio::device::try_match_alsa_device(requested, n) == Some(true))
        {
            return Some(pos);
        }
    }
    None
}

/// Get an input device by name, by --list-inputs index, or the default if
/// name is None
///
/// Falls back to ALSA card matching so that names written by hand
/// ("hw:CARD=UMC1820, DEV=0") still resolve to the enumerated device, the same
/// way output devices are resolved.
pub fn get_input_device(name: Option<&str>) -> Result<Device, InputError> {
    let host = cpal::default_host();

    match name {
        Some(device_name) => {
            let names: Vec<String> = host
                .input_devices()
                .map_err(|e| InputError::DeviceEnumeration(e.to_string()))?
                .filter_map(|device| {
                    device.description().ok().map(|d| {
                        super::device::output_device_identifier(&d, cfg!(target_os = "linux"))
                            .to_string()
                    })
                })
                .filter(|name| name != "null")
                .collect();
            if let Some(pos) = resolve_requested_device(device_name, &names) {
                let target = &names[pos];
                let found = host
                    .input_devices()
                    .map_err(|e| InputError::DeviceEnumeration(e.to_string()))?
                    .find(|device| {
                        device
                            .description()
                            .map(|d| {
                                super::device::output_device_identifier(
                                    &d,
                                    cfg!(target_os = "linux"),
                                ) == target
                            })
                            .unwrap_or(false)
                    });
                if let Some(device) = found {
                    return Ok(device);
                }
            }

            #[cfg(target_os = "linux")]
            let probe = probe_capture_open(device_name);
            #[cfg(not(target_os = "linux"))]
            let probe = None;

            Err(InputError::DeviceNotFound {
                requested: device_name.to_string(),
                available: names,
                probe,
            })
        }
        None => host
            .default_input_device()
            .ok_or(InputError::NoDefaultDevice),
    }
}

/// Lowest input channel count that can serve every routed source channel
pub fn required_channels(channel_map: &[(usize, usize)]) -> usize {
    channel_map
        .iter()
        .map(|(src, _)| src + 1)
        .max()
        .unwrap_or(0)
}

/// Choose the channel count to open a capture stream with.
///
/// `exact` forces a specific count, for hardware whose capabilities cpal
/// reports incorrectly. Otherwise the smallest supported count that covers
/// every routed source channel is used, so a device offering a range is not
/// opened wider than the routing needs while a device with a fixed layout
/// (ALSA `hw:` on a multichannel interface) still matches. Returns None when
/// no supported count satisfies the request.
pub fn select_channel_count(
    available: &[u16],
    exact: Option<usize>,
    minimum: usize,
) -> Option<u16> {
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
/// Supported native formats are converted to f32 by the typed capture callback.
/// Choose the capture rate from a reported range. ALSA plug devices report a
/// continuous range with an implausible maximum; that is capability-report
/// noise, not a reason to disqualify the configuration, so the ceiling is
/// clamped and the preferred rate wins whenever the range covers it.
fn choose_capture_rate(min_rate: u32, max_rate: u32, preferred: u32) -> u32 {
    let max_rate = max_rate.min(MAX_REASONABLE_SAMPLE_RATE);
    preferred.max(min_rate).min(max_rate)
}

pub fn find_input_config(
    device: &Device,
    requested_channels: Option<usize>,
    minimum_channels: usize,
    preferred_sample_rate: u32,
) -> Result<SupportedStreamConfig, InputError> {
    let supported: Vec<_> = device
        .supported_input_configs()
        .map_err(|e| InputError::ConfigError(e.to_string()))?
        .filter(|c| c.min_sample_rate() <= MAX_REASONABLE_SAMPLE_RATE)
        .filter(|c| c.channels() <= MAX_REASONABLE_CHANNELS)
        .collect();

    // Plugin devices often enumerate nothing usable; fall back to whatever the
    // device reports as its default and let the stream build succeed or fail.
    if supported.is_empty() {
        return get_input_config(device);
    }

    let float_configs: Vec<_> = supported
        .iter()
        .filter(|c| input_stream_builder(c.sample_format()).is_some())
        .collect();

    if float_configs.is_empty() {
        let formats: Vec<String> = supported
            .iter()
            .map(|c| format!("{:?}", c.sample_format()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        return Err(InputError::ConfigError(format!(
            "device offers no supported capture format (has: {}); use the 'plughw:' alias for this card",
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
        choose_capture_rate(
            c.min_sample_rate(),
            c.max_sample_rate(),
            preferred_sample_rate,
        ) == preferred_sample_rate
    });

    match exact_rate {
        Some(c) => Ok(c.with_sample_rate(preferred_sample_rate)),
        None => {
            let closest = matching[0];
            let rate = choose_capture_rate(
                closest.min_sample_rate(),
                closest.max_sample_rate(),
                preferred_sample_rate,
            );
            Ok(closest.with_sample_rate(rate))
        }
    }
}

/// Push whole frames into the ring buffer, returning the number of frames written.
///
/// A partially written frame would permanently shift the channel interleaving
/// seen by the mixer, sending each microphone to the wrong output and leaving a
/// residue that grows until the buffer is full. Frames that do not fit are
/// counted as dropped instead.
#[cfg(test)]
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
#[derive(Clone)]
pub struct InputStreamConfig {
    pub device_name: Option<String>,
    pub latency_ms: u32,
    /// Exact channel count to open (None = smallest count covering `min_channels`)
    pub channels: Option<usize>,
    /// Lowest channel count the configured routing needs
    pub min_channels: usize,
    /// Capture rate to request (None = match the output rate when supported)
    pub sample_rate: Option<u32>,
    /// Quality preset for capture-rate conversion, from advanced.resampler_quality
    pub resampler_quality: ResamplerQuality,
    /// Stream buffer size in frames, matched to the output stream so both
    /// directions of a shared-clock USB interface run compatible parameters
    /// (None = device default)
    pub buffer_size: Option<u32>,
}

impl Default for InputStreamConfig {
    fn default() -> Self {
        Self {
            device_name: None,
            latency_ms: 20, // 20ms default latency buffer
            channels: None,
            min_channels: 1,
            sample_rate: None,
            resampler_quality: ResamplerQuality::default(),
            buffer_size: None,
        }
    }
}

/// Active input stream with its ring buffer consumer
pub struct ActiveInput {
    #[allow(dead_code)] // Stream must be kept alive for audio to flow
    stream: Stream,
    consumers: Vec<HeapConsumer<f32>>,
    pub channels: usize,
    /// Capture-path counters (D57), drained/logged off-RT and surfaced on /metrics.
    pub telemetry: Arc<InputTelemetry>,
    pub sample_rate: u32,
    /// Backlog the mixer tolerates before trimming, from the ring and resampler
    /// geometry.
    pub max_backlog_frames: usize,
}

impl ActiveInput {
    /// Take ownership of the ring buffer consumer
    /// Returns None if already taken
    pub fn take_consumer(&mut self) -> Option<HeapConsumer<f32>> {
        self.consumers.pop()
    }

    /// Take every logical-strip consumer fed by this one physical capture.
    pub fn take_consumers(&mut self) -> Vec<HeapConsumer<f32>> {
        std::mem::take(&mut self.consumers)
    }
}

/// Create an input stream with a ring buffer for audio transfer
/// Returns the stream and a consumer for reading audio samples
pub fn create_input_stream(
    config: InputStreamConfig,
    target_sample_rate: u32,
) -> Result<ActiveInput, InputError> {
    create_input_stream_with_fanout(config, target_sample_rate, 1)
}

/// Create one physical capture stream and fan its resampled frames into a
/// bounded ring for each logical input strip. This avoids opening a single-open
/// ALSA device once per team microphone while keeping existing one-consumer
/// callers source-compatible.
pub fn create_input_stream_with_fanout(
    config: InputStreamConfig,
    target_sample_rate: u32,
    consumer_count: usize,
) -> Result<ActiveInput, InputError> {
    if consumer_count == 0 {
        return Err(InputError::ConfigError(
            "consumer_count must be positive".to_string(),
        ));
    }
    let device = get_input_device(config.device_name.as_deref())?;
    let device_name = device
        .description()
        .map(|d| super::device::output_device_identifier(&d, cfg!(target_os = "linux")).to_string())
        .unwrap_or_else(|_| "Unknown".to_string());
    let supported_config = find_input_config(
        &device,
        config.channels,
        config.min_channels,
        config.sample_rate.unwrap_or(target_sample_rate),
    )?;

    let input_sample_rate = supported_config.sample_rate();
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

    let minimum_latency = minimum_latency_ms(input_sample_rate, target_sample_rate);
    let latency_ms = if config.latency_ms < minimum_latency {
        tracing::warn!(
            "Input device '{}' latency_ms {} is below the {} ms its ring needs to hold one \
             resampler burst; using {} ms",
            device_name,
            config.latency_ms,
            minimum_latency,
            minimum_latency
        );
        minimum_latency
    } else {
        config.latency_ms
    };
    // Calculate ring buffer size based on OUTPUT sample rate (after potential resampling)
    let buffer_size = calculate_ring_buffer_size(target_sample_rate, channels, latency_ms);

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
        latency_ms
    );

    // Every input runs through async sample-rate conversion so two independently
    // clocked devices stay drift-bounded, even at equal nominal rates (D33). The
    // device sample format is handled orthogonally: a typed callback converts the
    // native format to f32 before the resampler sees it (D37).
    let build = |requested_buffer: Option<u32>| -> Result<_, InputError> {
        let rings: Vec<(HeapProducer<f32>, HeapConsumer<f32>)> = (0..consumer_count)
            .map(|_| create_ring_buffer(buffer_size))
            .collect();
        let (producers, consumers): (Vec<_>, Vec<_>) = rings.into_iter().unzip();
        let mut options = config.clone();
        options.buffer_size = requested_buffer;
        let (stream, telemetry, max_backlog_frames) = build_resampling_input_stream(
            &device,
            supported_config,
            sample_format,
            producers,
            input_sample_rate,
            target_sample_rate,
            channels,
            &options,
        )?;
        Ok((stream, telemetry, max_backlog_frames, consumers))
    };
    let (stream, telemetry, max_backlog_frames, consumers) = match build(config.buffer_size) {
        Ok(built) => built,
        Err(error) if config.buffer_size.is_some() => {
            tracing::warn!(
                "Capture rejected the requested buffer size: {}; retrying the device default",
                error
            );
            build(None)?
        }
        Err(error) => return Err(error),
    };

    stream
        .play()
        .map_err(|e| InputError::StreamError(e.to_string()))?;

    Ok(ActiveInput {
        stream,
        consumers,
        channels,
        sample_rate: input_sample_rate,
        telemetry,
        max_backlog_frames,
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
    pub dropped_frames: Arc<AtomicU64>,
    pub peak_levels: Vec<AtomicU32>,
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
    /// Output frames one resampler chunk publishes at the nominal ratio, the burst
    /// the ring absorbs on top of its steady fill.
    burst_frames: usize,
    /// Ring fill (interleaved samples) the drift-control loop steers toward: the
    /// middle of the span a burst still fits above.
    fill_target: usize,
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
        Self::with_quality(
            input_rate,
            output_rate,
            channels,
            ring_capacity,
            ResamplerQuality::Maximum,
        )
    }

    pub fn with_quality(
        input_rate: u32,
        output_rate: u32,
        channels: usize,
        ring_capacity: usize,
        quality: ResamplerQuality,
    ) -> Result<Self, InputError> {
        use rubato::{
            Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType,
            WindowFunction,
        };

        let params = SincInterpolationParameters {
            sinc_len: quality.sinc_len(),
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: quality.oversampling_factor(),
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

        let burst_frames = resample_burst_frames(input_rate, output_rate);
        let fill_target = ring_capacity.saturating_sub(burst_frames * channels) / 2;

        Ok(Self {
            resampler,
            channels,
            deinterleave,
            filled: 0,
            output,
            nominal_ratio,
            burst_frames,
            fill_target,
            smoothed_fill: -1.0,
            last_ratio: nominal_ratio,
            telemetry: Arc::new(InputTelemetry {
                peak_levels: (0..channels).map(|_| AtomicU32::new(0)).collect(),
                ..Default::default()
            }),
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

    /// Backlog the mixer tolerates before trimming: the highest fill the loop
    /// parks at while its authority still covers the drift, plus one burst landing
    /// on top. Beyond this the drift exceeds what steering can absorb and trimming
    /// is the right response.
    pub fn backlog_ceiling_frames(&self) -> usize {
        let target_frames = self.fill_target / self.channels;
        let parked_max = target_frames as f64 * (1.0 + STEER_MAX_DEVIATION / STEER_GAIN);
        parked_max.ceil() as usize + self.burst_frames
    }
}

/// Output frames one resampler chunk publishes at the nominal ratio.
fn resample_burst_frames(input_rate: u32, output_rate: u32) -> usize {
    (RESAMPLE_CHUNK_SIZE as f64 * output_rate as f64 / input_rate as f64).ceil() as usize
}

/// Smallest `latency_ms` whose ring holds the steering target plus one resampler
/// burst. Below this the capture side drops at the ring on every chunk, so a
/// configured value under it is raised.
pub fn minimum_latency_ms(input_rate: u32, output_rate: u32) -> u32 {
    let burst = resample_burst_frames(input_rate, output_rate);
    let frames_per_ms = (output_rate / 1000).max(1) as usize;
    // The ring holds 4 * latency_ms * frames_per_ms frames; two bursts leave the
    // target one burst of headroom on each side.
    (2 * burst).div_ceil(4 * frames_per_ms) as u32
}

/// Resample one block of interleaved capture `data` into `producer`.
///
/// RT-safe: pre-sized accumulators receive de-interleaved samples by index (no
/// `Vec` growth), `process_into_buffer` writes into a reusable output buffer (no
/// allocation), and the ratio is steered toward a half-full ring via
/// `set_resample_ratio` (which only updates two floats). Nothing here allocates,
/// frees, or locks.
#[allow(dead_code)]
pub fn resample_block(state: &mut ResampleState, data: &[f32], producer: &mut HeapProducer<f32>) {
    resample_block_fanout(state, data, std::slice::from_mut(producer));
}

/// Resample one block and publish the interleaved result to every logical-strip
/// ring backed by one physical capture. All producers are preallocated during
/// setup; this function only performs bounded pushes and relaxed counters.
pub fn resample_block_fanout(
    state: &mut ResampleState,
    data: &[f32],
    producers: &mut [HeapProducer<f32>],
) {
    use rubato::Resampler;

    if producers.is_empty() {
        return;
    }

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
        steer_ratio(state, &producers[0]);

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
        for producer in producers.iter_mut() {
            let writable = out_frames.min(producer.free_len() / channels);
            dropped += (out_frames - writable) * channels;
            for frame_idx in 0..writable {
                for ch in 0..channels {
                    let _ = producer.push(state.output[ch][frame_idx]);
                }
            }
        }
        if dropped > 0 {
            state
                .telemetry
                .dropped_frames
                .fetch_add((dropped / channels) as u64, Ordering::Relaxed);
            state
                .telemetry
                .overflow_dropped_samples
                .fetch_add(dropped as u64, Ordering::Relaxed);
        }
    }
}

/// Nudge the resampler ratio toward keeping the ring buffer at its fill target
/// (D33).
///
/// Two independently-clocked devices drift; left alone the ring monotonically
/// fills or drains until it clicks (overflow) or starves (underrun silence). A
/// slow proportional loop on the smoothed fill error corrects that: when the
/// ring runs above the target the consumer is slower than the producer, so we
/// reduce the output ratio (emit fewer frames) and vice versa. The commanded
/// deviation is clamped to [`STEER_MAX_DEVIATION`], well inside the resampler's
/// `RESAMPLE_MAX_RELATIVE`, so it is inaudible as pitch.
fn steer_ratio(state: &mut ResampleState, producer: &HeapProducer<f32>) {
    use rubato::Resampler;

    if state.fill_target == 0 {
        return;
    }

    let fill = producer.len() as f64;
    state.smoothed_fill = if state.smoothed_fill < 0.0 {
        fill
    } else {
        state.smoothed_fill + STEER_FILL_SMOOTHING * (fill - state.smoothed_fill)
    };

    let target = state.fill_target as f64;
    // Normalized error: -1.0 when empty, +1.0 at twice the target.
    let err = (state.smoothed_fill - target) / target;
    // Above the target => slow the output (ratio < nominal); below => speed it up.
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
#[allow(clippy::too_many_arguments)]
fn build_resampling_input_stream(
    device: &Device,
    config: SupportedStreamConfig,
    sample_format: cpal::SampleFormat,
    producers: Vec<HeapProducer<f32>>,
    input_rate: u32,
    output_rate: u32,
    channels: usize,
    options: &InputStreamConfig,
) -> Result<(Stream, Arc<InputTelemetry>, usize), InputError> {
    match input_stream_builder(sample_format) {
        Some(InputSampleHandling::F32) => build_typed_resampling_input_stream::<f32>(
            device,
            config,
            producers,
            input_rate,
            output_rate,
            channels,
            options,
        ),
        Some(InputSampleHandling::I16) => build_typed_resampling_input_stream::<i16>(
            device,
            config,
            producers,
            input_rate,
            output_rate,
            channels,
            options,
        ),
        Some(InputSampleHandling::U16) => build_typed_resampling_input_stream::<u16>(
            device,
            config,
            producers,
            input_rate,
            output_rate,
            channels,
            options,
        ),
        Some(InputSampleHandling::I32) => build_typed_resampling_input_stream::<i32>(
            device,
            config,
            producers,
            input_rate,
            output_rate,
            channels,
            options,
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
#[allow(clippy::too_many_arguments)]
fn build_typed_resampling_input_stream<T>(
    device: &Device,
    config: SupportedStreamConfig,
    mut producers: Vec<HeapProducer<f32>>,
    input_rate: u32,
    output_rate: u32,
    channels: usize,
    options: &InputStreamConfig,
) -> Result<(Stream, Arc<InputTelemetry>, usize), InputError>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let ring_capacity = producers
        .first()
        .map(|producer| producer.capacity())
        .unwrap_or(0);
    let mut state = ResampleState::with_quality(
        input_rate,
        output_rate,
        channels,
        ring_capacity,
        options.resampler_quality,
    )?;
    let telemetry = state.telemetry();
    let callback_telemetry = Arc::clone(&telemetry);
    let max_backlog_frames = state.backlog_ceiling_frames();

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

    let mut stream_config: cpal::StreamConfig = config.into();
    if let Some(frames) = options.buffer_size {
        stream_config.buffer_size = cpal::BufferSize::Fixed(frames);
    }
    let stream = device
        .build_input_stream(
            stream_config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                if data.len() > scratch.capacity() {
                    // Pathological device: a block beyond the advertised maximum
                    // is about to regrow the scratch on the capture thread (D58).
                    callback_telemetry
                        .scratch_regrows
                        .fetch_add(1, Ordering::Relaxed);
                }
                convert_input_block::<T>(data, &mut scratch, |f32s| {
                    for (i, &value) in f32s.iter().enumerate() {
                        callback_telemetry.peak_levels[i % channels]
                            .fetch_max(value.abs().to_bits(), Ordering::Relaxed);
                    }
                    resample_block_fanout(&mut state, f32s, &mut producers);
                });
            },
            move |err| {
                tracing::error!("Input stream error: {}", err);
            },
            None,
        )
        .map_err(|e| InputError::StreamError(e.to_string()))?;

    Ok((stream, telemetry, max_backlog_frames))
}

/// Error types for input operations
#[derive(Debug)]
pub enum InputError {
    DeviceEnumeration(String),
    DeviceNotFound {
        requested: String,
        available: Vec<String>,
        probe: Option<String>,
    },
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
            InputError::DeviceNotFound {
                requested,
                available,
                probe,
            } => {
                write!(
                    f,
                    "{}",
                    device_not_found_message(requested, available, probe.as_deref())
                )
            }
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

    #[test]
    fn test_resample_fanout_feeds_each_logical_strip_ring() {
        let channels = 2;
        let mut state = ResampleState::new(48000, 48000, channels, 8192).unwrap();
        let (first, mut first_consumer) = create_ring_buffer(8192);
        let (second, mut second_consumer) = create_ring_buffer(8192);
        let data = vec![0.25_f32; 1024 * channels];
        resample_block_fanout(&mut state, &data, &mut [first, second]);
        let mut first_out = vec![0.0; 2048];
        let mut second_out = vec![0.0; 2048];
        let first_count = first_consumer.pop_slice(&mut first_out);
        let second_count = second_consumer.pop_slice(&mut second_out);
        assert!(first_count > 0);
        assert_eq!(first_count, second_count);
        assert_eq!(&first_out[..first_count], &second_out[..second_count]);
    }

    // Note: Device enumeration tests require actual hardware and are skipped in CI
    // The following tests are integration tests that need real devices:
    // - test_list_input_devices
    // - test_get_default_input_device
    // - test_create_input_stream

    #[test]
    fn test_parse_device_index() {
        assert_eq!(parse_device_index("0", 3), Some(0));
        assert_eq!(parse_device_index("2", 3), Some(2));
        assert_eq!(parse_device_index(" 1 ", 3), Some(1));
        assert_eq!(parse_device_index("3", 3), None, "out of range");
        assert_eq!(parse_device_index("0", 0), None, "no devices");
        assert_eq!(parse_device_index("-1", 3), None);
        assert_eq!(parse_device_index("hw:CARD=UMC1820,DEV=0", 3), None);
        assert_eq!(parse_device_index("", 3), None);
    }

    #[test]
    fn test_resolve_requested_device_prefers_exact_name() {
        let names = vec![
            "hw:CARD=UMC1820,DEV=0".to_string(),
            "plughw:CARD=UMC1820,DEV=0".to_string(),
        ];
        assert_eq!(
            resolve_requested_device("plughw:CARD=UMC1820,DEV=0", &names),
            Some(1)
        );
        assert_eq!(
            resolve_requested_device("hw:CARD=UMC1820,DEV=0", &names),
            Some(0)
        );
    }

    #[test]
    fn test_resolve_requested_device_by_index() {
        let names = vec![
            "hw:CARD=UMC1820,DEV=0".to_string(),
            "plughw:CARD=UMC1820,DEV=0".to_string(),
        ];
        assert_eq!(resolve_requested_device("1", &names), Some(1));
        assert_eq!(resolve_requested_device("2", &names), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_resolve_requested_device_by_alsa_card() {
        let names = vec![
            "hw:CARD=Headset,DEV=0".to_string(),
            "hw:CARD=UMC1820,DEV=0".to_string(),
        ];
        assert_eq!(
            resolve_requested_device("hw:CARD=UMC1820, DEV=0", &names),
            Some(1)
        );
    }

    #[test]
    fn test_resolve_requested_device_unmatched() {
        let names = vec!["hw:CARD=UMC1820,DEV=0".to_string()];
        assert_eq!(
            resolve_requested_device("hw:CARD=Missing,DEV=0", &names),
            None
        );
        assert_eq!(resolve_requested_device("anything", &[]), None);
    }

    #[test]
    fn test_choose_capture_rate_honors_preferred_within_plugin_range() {
        // ALSA plug devices report a continuous range with an absurd maximum;
        // the preferred rate inside the plausible part of the range wins.
        assert_eq!(choose_capture_rate(4000, 4294967295, 48000), 48000);
        assert_eq!(choose_capture_rate(4000, 4294967295, 44100), 44100);
    }

    #[test]
    fn test_choose_capture_rate_clamps_to_range() {
        assert_eq!(choose_capture_rate(44100, 44100, 48000), 44100);
        assert_eq!(choose_capture_rate(48000, 192000, 44100), 48000);
        assert_eq!(choose_capture_rate(8000, 4294967295, 500000), 384000);
    }

    #[test]
    fn test_device_not_found_message_lists_available_devices() {
        let available = vec!["hw:CARD=UMC1820,DEV=0".to_string(), "default".to_string()];
        let msg = device_not_found_message("plughw:CARD=UMC1820,DEV=0", &available, None);
        assert!(msg.contains("plughw:CARD=UMC1820,DEV=0"));
        assert!(msg.contains("hw:CARD=UMC1820,DEV=0"));
        assert!(msg.contains("default"));
    }

    #[test]
    fn test_device_not_found_message_explains_empty_enumeration() {
        let msg = device_not_found_message("hw:CARD=X,DEV=0", &[], None);
        assert!(msg.contains("hw:CARD=X,DEV=0"));
        assert!(msg.to_lowercase().contains("no devices"));
    }

    #[test]
    fn test_device_not_found_message_includes_probe_result() {
        let msg = device_not_found_message(
            "hw:CARD=X,DEV=0",
            &[],
            Some("Direct ALSA capture open failed: EBUSY"),
        );
        assert!(msg.contains("Direct ALSA capture open failed: EBUSY"));
    }

    #[test]
    fn test_device_not_found_error_display_carries_details() {
        let err = InputError::DeviceNotFound {
            requested: "plughw:CARD=UMC1820,DEV=0".to_string(),
            available: vec!["hw:CARD=UMC1820,DEV=0".to_string()],
            probe: Some("probe detail".to_string()),
        };
        let msg = err.to_string();
        assert!(msg.contains("Input device not found"));
        assert!(msg.contains("plughw:CARD=UMC1820,DEV=0"));
        assert!(msg.contains("hw:CARD=UMC1820,DEV=0"));
        assert!(msg.contains("probe detail"));
    }

    #[test]
    fn test_get_input_device_error_reports_requested_name_and_probe() {
        // A card name no system has, so this fails everywhere. The error must
        // carry the requested name and, on Linux, a direct-ALSA diagnosis of
        // why the capture open fails.
        match get_input_device(Some("hw:CARD=NoSuchCardExists,DEV=0")) {
            Err(InputError::DeviceNotFound {
                requested, probe, ..
            }) => {
                assert_eq!(requested, "hw:CARD=NoSuchCardExists,DEV=0");
                #[cfg(target_os = "linux")]
                assert!(
                    probe.is_some(),
                    "Linux probe should always produce a diagnosis"
                );
                #[cfg(not(target_os = "linux"))]
                assert!(probe.is_none());
            }
            Err(other) => panic!("expected DeviceNotFound, got: {}", other),
            Ok(_) => panic!("nonexistent device resolved"),
        }
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
                consumer.len() % channels,
                0,
                "ring buffer must always hold a whole number of frames"
            );
        }

        assert!(
            dropped.load(Ordering::Relaxed) > 0,
            "saturation should be counted"
        );
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
}
