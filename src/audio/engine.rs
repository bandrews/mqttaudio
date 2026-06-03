// ABOUTME: Audio engine coordinator managing playback, caching, and state.
// ABOUTME: Handles sample loading, voice management, and mixer state updates.

use crate::audio::decoder;
use crate::audio::mixer::{ActiveSample, MixerState};
use crate::audio::types::DeviceConfig;
use crate::config::ResamplerQuality;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

/// List available audio output devices
pub fn list_devices() {
    #[cfg(target_os = "linux")]
    {
        list_devices_linux();
    }

    #[cfg(not(target_os = "linux"))]
    {
        list_devices_cpal_only();
    }
}

/// Linux-specific device listing with ALSA probing
#[cfg(target_os = "linux")]
fn list_devices_linux() {
    use super::alsa_probe::probe_alsa_devices;
    use super::device::format_device_list;

    let list = probe_alsa_devices();
    print!("{}", format_device_list(&list));
}

/// Fallback device listing using only cpal (for macOS, Windows, etc.)
#[cfg(not(target_os = "linux"))]
fn list_devices_cpal_only() {
    use super::device::{format_device_list, DeviceCategory, DeviceInfo, DeviceList};

    let host = cpal::default_host();
    let mut list = DeviceList::new();

    match host.output_devices() {
        Ok(devices) => {
            for device in devices {
                if let Ok(name) = device.name() {
                    let mut info = DeviceInfo::new(name.clone(), DeviceCategory::Hardware);

                    // Query supported configs
                    if let Ok(configs) = device.supported_output_configs() {
                        let mut max_channels = 0u16;
                        let mut min_rate = u32::MAX;
                        let mut max_rate = 0u32;

                        const MAX_REASONABLE_SAMPLE_RATE: u32 = 384000;

                        for config in configs {
                            let config_max_rate = config.max_sample_rate().0;
                            if config_max_rate > MAX_REASONABLE_SAMPLE_RATE {
                                continue;
                            }

                            max_channels = max_channels.max(config.channels());
                            min_rate = min_rate.min(config.min_sample_rate().0);
                            max_rate = max_rate.max(config_max_rate);
                        }

                        if max_channels > 0 {
                            info.cpal_channels = Some(max_channels);
                            info.cpal_sample_rate_min = Some(min_rate);
                            info.cpal_sample_rate_max = Some(max_rate);
                        }
                    }

                    // Fallback to default config
                    if info.cpal_channels.is_none() {
                        if let Ok(config) = device.default_output_config() {
                            info.cpal_channels = Some(config.channels());
                            info.cpal_sample_rate_min = Some(config.sample_rate().0);
                            info.cpal_sample_rate_max = Some(config.sample_rate().0);
                        }
                    }

                    list.devices.push(info);
                }
            }
        }
        Err(e) => {
            list.discovery_notes
                .push(format!("Error listing devices: {}", e));
        }
    }

    print!("{}", format_device_list(&list));
}

/// Get default device configuration
pub fn get_default_device_config() -> Result<DeviceConfig, Box<dyn std::error::Error>> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No default output device available")?;

    let config = device.default_output_config()?;

    Ok(DeviceConfig {
        sample_rate: config.sample_rate().0,
        channels: config.channels() as usize,
        buffer_size: 512, // Default buffer size
    })
}

/// Find an output device by name (returns default if name is None)
/// On ALSA, devices may be openable even if not enumerated, so we try
/// both enumeration and direct construction.
pub fn find_output_device(name: Option<&str>) -> Result<cpal::Device, Box<dyn std::error::Error>> {
    let host = cpal::default_host();

    match name {
        Some(device_name) => {
            // First try to find in enumerated devices
            if let Ok(devices) = host.output_devices() {
                for device in devices {
                    if let Ok(n) = device.name() {
                        if n == device_name {
                            return Ok(device);
                        }
                    }
                }
            }

            // On ALSA, try partial matching for device names
            // This handles cases like "hw:1,0" matching "hw:CARD=UMC1820,DEV=0"
            // or "plughw:1,0" matching "plughw:CARD=UMC1820,DEV=0"
            #[cfg(target_os = "linux")]
            {
                if let Ok(devices) = host.output_devices() {
                    for device in devices {
                        if let Ok(n) = device.name() {
                            // Try matching ALSA device names by card number
                            // Supports hw:, plughw:, sysdefault:
                            if let Some(matched) = try_match_alsa_device(device_name, &n) {
                                if matched {
                                    tracing::info!(
                                        "Matched ALSA device '{}' to '{}'",
                                        device_name,
                                        n
                                    );
                                    return Ok(device);
                                }
                            }
                        }
                    }
                }
            }

            Err(format!("Output device not found: {}", device_name).into())
        }
        None => host
            .default_output_device()
            .ok_or_else(|| "No default output device available".into()),
    }
}

/// Try to match two ALSA device names
/// Returns Some(true) if they match, Some(false) if same prefix but different card, None if not comparable
#[cfg(target_os = "linux")]
fn try_match_alsa_device(requested: &str, enumerated: &str) -> Option<bool> {
    // Get prefix (hw:, plughw:, sysdefault:, etc.)
    let prefixes = [
        "plughw:",
        "hw:",
        "sysdefault:",
        "dmix:",
        "front:",
        "surround",
    ];

    for prefix in prefixes {
        if requested.starts_with(prefix) && enumerated.starts_with(prefix) {
            // Both have the same prefix, compare by card number
            let req_card = extract_alsa_card_from_name(requested);
            let enum_card = extract_alsa_card_from_name(enumerated);

            match (req_card, enum_card) {
                (Some(r), Some(e)) => return Some(r == e),
                _ => continue,
            }
        }
    }

    None
}

/// Extract card identifier from ALSA device name
/// Handles both numeric (hw:1,0) and named (hw:CARD=UMC1820,DEV=0) formats
#[cfg(target_os = "linux")]
fn extract_alsa_card_from_name(name: &str) -> Option<AlsaCardId> {
    // Find prefix end
    let prefixes = [
        "plughw:",
        "hw:",
        "sysdefault:",
        "dmix:",
        "front:",
        "surround",
    ];

    for prefix in prefixes {
        if let Some(rest) = name.strip_prefix(prefix) {
            // Try "CARD=name" format first
            if let Some(card_part) = rest.strip_prefix("CARD=") {
                let card_name = if let Some(comma_pos) = card_part.find(',') {
                    &card_part[..comma_pos]
                } else {
                    card_part
                };
                return Some(AlsaCardId::Name(card_name.to_string()));
            }

            // Try numeric format "N,M" or just "N"
            let num_part = if let Some(comma_pos) = rest.find(',') {
                &rest[..comma_pos]
            } else {
                rest
            };

            if let Ok(num) = num_part.parse::<u32>() {
                return Some(AlsaCardId::Index(num));
            }
        }
    }

    None
}

/// ALSA card identifier - can be either index or name
#[cfg(target_os = "linux")]
#[derive(Debug)]
enum AlsaCardId {
    Index(u32),
    Name(String),
}

#[cfg(target_os = "linux")]
impl PartialEq for AlsaCardId {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (AlsaCardId::Index(a), AlsaCardId::Index(b)) => a == b,
            (AlsaCardId::Name(a), AlsaCardId::Name(b)) => a == b,
            // Cross-compare by looking up card index from name
            (AlsaCardId::Index(idx), AlsaCardId::Name(name))
            | (AlsaCardId::Name(name), AlsaCardId::Index(idx)) => {
                // Try to match card name to index by checking /proc/asound/cards
                if let Ok(cards) = std::fs::read_to_string("/proc/asound/cards") {
                    for line in cards.lines() {
                        // Format: " 1 [UMC1820        ]: USB-Audio - UMC1820"
                        if let Some(bracket_start) = line.find('[') {
                            if let Some(bracket_end) = line.find(']') {
                                let card_id = line[bracket_start + 1..bracket_end].trim();
                                if card_id == name {
                                    // Found the card name, extract index
                                    let idx_str = line[..bracket_start].trim();
                                    if let Ok(card_idx) = idx_str.parse::<u32>() {
                                        return card_idx == *idx;
                                    }
                                }
                            }
                        }
                    }
                }
                false
            }
        }
    }
}

/// The output configuration chosen for a device: the stream config plus the
/// sample format the typed callback must produce.
pub struct OutputConfig {
    pub stream_config: cpal::StreamConfig,
    pub sample_format: cpal::SampleFormat,
}

/// Find an output configuration for the device — channel count, sample format,
/// sample rate, and buffer size — delegating the choices to the pure helpers in
/// `device_select` and validating the result against the device's supported
/// configs before returning. This negotiates non-f32 and discrete-rate devices
/// correctly instead of crashing at stream-build time.
///
/// `requested_channels`/`requested_sample_rate` of `None` use the device's
/// maximum/preferred values. `requested_buffer_size` is honored when the device
/// supports it, otherwise the device default is used.
pub fn find_output_config(
    device: &cpal::Device,
    requested_channels: Option<usize>,
    requested_sample_rate: Option<u32>,
    requested_buffer_size: u32,
) -> Result<OutputConfig, Box<dyn std::error::Error>> {
    use crate::audio::device_select::{
        select_buffer_size, select_channels, select_sample_format, select_sample_rate,
        BufferLimits, ConfigOption, SelectError,
    };

    let supported: Vec<_> = device.supported_output_configs()?.collect();
    if supported.is_empty() {
        return Err("No supported output configurations found".into());
    }

    let options: Vec<ConfigOption> = supported
        .iter()
        .map(|c| ConfigOption {
            channels: c.channels(),
            sample_format: c.sample_format(),
            min_rate: c.min_sample_rate().0,
            max_rate: c.max_sample_rate().0,
            buffer: match c.buffer_size() {
                cpal::SupportedBufferSize::Range { min, max } => BufferLimits::Range {
                    min: *min,
                    max: *max,
                },
                cpal::SupportedBufferSize::Unknown => BufferLimits::Unknown,
            },
        })
        .collect();

    let channels = select_channels(&options, requested_channels)?;
    let sample_format =
        select_sample_format(&options, channels).ok_or(SelectError::NoFormat { channels })?;

    // Rate spans for the chosen (channels, format).
    let ranges: Vec<(u32, u32)> = options
        .iter()
        .filter(|o| o.channels == channels && o.sample_format == sample_format)
        .map(|o| (o.min_rate, o.max_rate))
        .collect();

    let requested_rate = requested_sample_rate.unwrap_or_else(|| {
        device
            .default_output_config()
            .map(|c| c.sample_rate().0)
            .unwrap_or(48000)
    });

    // On ALSA a raw `hw:` device may advertise a continuous range but accept
    // only discrete rates; prefer the probed discrete set when available.
    #[cfg(target_os = "linux")]
    let discrete = crate::audio::alsa_probe::discrete_rates_for(&device.name().unwrap_or_default());
    #[cfg(not(target_os = "linux"))]
    let discrete: Option<Vec<u32>> = None;

    let sample_rate = select_sample_rate(&ranges, discrete.as_deref(), requested_rate);

    let buffer_limits = options
        .iter()
        .find(|o| {
            o.channels == channels
                && o.sample_format == sample_format
                && sample_rate >= o.min_rate
                && sample_rate <= o.max_rate
        })
        .map(|o| o.buffer)
        .unwrap_or(BufferLimits::Unknown);
    let buffer_size = select_buffer_size(buffer_limits, requested_buffer_size);

    // Validate the chosen config against the device's real supported configs.
    let supported_here = supported.iter().any(|c| {
        c.channels() == channels
            && c.sample_format() == sample_format
            && sample_rate >= c.min_sample_rate().0
            && sample_rate <= c.max_sample_rate().0
    });
    if !supported_here {
        return Err(format!(
            "selected output config ({} ch, {:?}, {} Hz) is not supported by the device",
            channels, sample_format, sample_rate
        )
        .into());
    }

    Ok(OutputConfig {
        stream_config: cpal::StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size,
        },
        sample_format,
    })
}

/// Run one audio callback's worth of mixing into the f32 bus, plus the
/// voice-activity bookkeeping the control side reads. Shared by every typed
/// output stream so the device sample format never changes the mix logic.
fn run_mix_callback(
    bus: &mut [f32],
    mixer_state: &Arc<Mutex<MixerState>>,
    active_voices: &Arc<Mutex<std::collections::HashSet<String>>>,
) {
    use std::collections::HashSet;

    let mut state = mixer_state.lock();
    crate::audio::mixer::mix_audio(bus, &mut state);

    let voices_before: HashSet<String> = state
        .active_samples
        .iter()
        .map(|s| s.voice_id.clone())
        .collect();
    state.active_samples.retain(|s| !s.is_finished());
    let voices_after: HashSet<String> = state
        .active_samples
        .iter()
        .map(|s| s.voice_id.clone())
        .collect();

    if let Some(ref mut engine) = state.ducking_engine {
        for voice in voices_before.difference(&voices_after) {
            engine.notify_voice_active(voice, false);
        }
    }

    *active_voices.lock() = voices_after;
}

/// Build an output stream of element type `T`, mixing into an f32 scratch bus and
/// converting each sample to `T`. The scratch bus is reused across callbacks.
fn build_typed_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mixer_state: Arc<Mutex<MixerState>>,
    active_voices: Arc<Mutex<std::collections::HashSet<String>>>,
    error_flag: Arc<AtomicBool>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + cpal::FromSample<f32> + Send + 'static,
{
    let mut scratch: Vec<f32> = Vec::new();
    device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            scratch.resize(data.len(), 0.0);
            run_mix_callback(&mut scratch, &mixer_state, &active_voices);
            for (out, &s) in data.iter_mut().zip(scratch.iter()) {
                *out = T::from_sample(s);
            }
        },
        move |err| {
            tracing::error!("Audio stream error: {}", err);
            // Signal the supervisor to rebuild; the cpal callback must not do it.
            error_flag.store(true, Ordering::Relaxed);
        },
        None,
    )
}

/// Dispatch over the device's native sample format and build the matching typed
/// output stream. The mixer always works in f32; only the device-facing
/// conversion differs, so non-f32 devices no longer crash at build time.
pub fn build_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    mixer_state: Arc<Mutex<MixerState>>,
    active_voices: Arc<Mutex<std::collections::HashSet<String>>>,
    error_flag: Arc<AtomicBool>,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    use cpal::SampleFormat;
    let stream = match sample_format {
        SampleFormat::F32 => build_typed_output_stream::<f32>(
            device,
            config,
            mixer_state,
            active_voices,
            error_flag,
        )?,
        SampleFormat::I16 => build_typed_output_stream::<i16>(
            device,
            config,
            mixer_state,
            active_voices,
            error_flag,
        )?,
        SampleFormat::U16 => build_typed_output_stream::<u16>(
            device,
            config,
            mixer_state,
            active_voices,
            error_flag,
        )?,
        SampleFormat::I32 => build_typed_output_stream::<i32>(
            device,
            config,
            mixer_state,
            active_voices,
            error_flag,
        )?,
        other => return Err(format!("unsupported device sample format: {:?}", other).into()),
    };
    Ok(stream)
}

/// Own and supervise the output stream on a dedicated thread. On a fatal stream
/// error the supervisor rebuilds it — re-resolving the device by name and reusing
/// the original negotiated config (so the mixer's channel count and resample
/// target stay valid) — with exponential backoff. If the device cannot be
/// rebuilt after the backoff is exhausted, the process exits so a service manager
/// (e.g. systemd) can restart with a fresh full setup. Returns when `shutdown`
/// is set.
pub fn spawn_output_supervisor(
    device_name: Option<String>,
    config: cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    mixer_state: Arc<Mutex<MixerState>>,
    active_voices: Arc<Mutex<std::collections::HashSet<String>>>,
    shutdown: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    use crate::audio::rebuild::RebuildPolicy;
    use std::time::{Duration, Instant};

    std::thread::spawn(move || {
        let mut policy = RebuildPolicy::new();
        let mut ever_succeeded = false;
        loop {
            if shutdown.load(Ordering::Relaxed) {
                return;
            }

            let error_flag = Arc::new(AtomicBool::new(false));
            let built = (|| -> Result<cpal::Stream, Box<dyn std::error::Error>> {
                let device = find_output_device(device_name.as_deref())?;
                let stream = build_output_stream(
                    &device,
                    &config,
                    sample_format,
                    mixer_state.clone(),
                    active_voices.clone(),
                    error_flag.clone(),
                )?;
                stream.play()?;
                Ok(stream)
            })();

            match built {
                Ok(stream) => {
                    tracing::info!("Audio stream started ({:?})", sample_format);
                    ever_succeeded = true;
                    let started = Instant::now();
                    // Hold the stream alive until a fatal error or shutdown.
                    while !error_flag.load(Ordering::Relaxed) && !shutdown.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    drop(stream);
                    if shutdown.load(Ordering::Relaxed) {
                        return;
                    }
                    // A stable run that then errors is a transient blip: reset the
                    // backoff and rebuild immediately. Rapid failures fall through
                    // to the growing backoff below.
                    if started.elapsed() >= Duration::from_secs(5) {
                        policy.reset();
                        tracing::warn!(
                            "Audio stream error after a stable run; rebuilding output..."
                        );
                        continue;
                    }
                    tracing::warn!("Audio stream failed shortly after start; backing off...");
                }
                Err(e) => {
                    tracing::error!("Audio output unavailable: {}", e);
                    if !ever_succeeded {
                        // A startup misconfiguration won't self-heal; fail fast.
                        #[cfg(target_os = "linux")]
                        tracing::error!(
                            "On ALSA, a raw 'hw:' device may require its native format; try a \
                             'plughw:' or 'default' device, which converts formats automatically."
                        );
                        std::process::exit(1);
                    }
                }
            }

            match policy.next_delay() {
                Some(delay) => {
                    tracing::info!("Retrying output device in {:?}", delay);
                    std::thread::sleep(delay);
                }
                None => {
                    tracing::error!(
                        "Output device could not be (re)built after several attempts; \
                         exiting for a service-manager restart."
                    );
                    std::process::exit(1);
                }
            }
        }
    })
}

/// Initialize audio output stream with a test sine wave
pub fn init_test_sine_wave() -> Result<Stream, Box<dyn std::error::Error>> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No default output device available")?;

    let config = device.default_output_config()?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;

    tracing::info!("Initializing audio stream:");
    tracing::info!("  Device: {}", device.name()?);
    tracing::info!("  Sample rate: {} Hz", sample_rate);
    tracing::info!("  Channels: {}", channels);
    tracing::info!("  Format: {:?}", config.sample_format());

    // Phase accumulator for sine wave (stored as atomic for thread safety)
    // Note: This is a simplified approach for Phase 1 testing only
    static PHASE: AtomicU32 = AtomicU32::new(0);
    let phase_increment = (440.0 * 2.0 * std::f32::consts::PI / sample_rate as f32) * 1000.0;

    let stream = device.build_output_stream(
        &config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            // Audio callback - generates 440Hz sine wave
            for frame in data.chunks_mut(channels) {
                // Get current phase and increment
                let phase_int = PHASE.fetch_add(phase_increment as u32, Ordering::Relaxed);
                let phase = (phase_int as f32) / 1000.0;

                // Generate sine wave sample at low volume (0.2)
                let sample = (phase.sin() * 0.2).clamp(-1.0, 1.0);

                // Write same sample to all channels
                for channel_sample in frame {
                    *channel_sample = sample;
                }

                // Wrap phase to prevent overflow
                if phase > 2.0 * std::f32::consts::PI {
                    PHASE.fetch_sub(
                        (2.0 * std::f32::consts::PI * 1000.0) as u32,
                        Ordering::Relaxed,
                    );
                }
            }
        },
        move |err| {
            tracing::error!("Stream error: {}", err);
        },
        None,
    )?;

    stream.play()?;
    tracing::info!("Audio stream started - you should hear a 440Hz tone");

    Ok(stream)
}

/// Play an audio file
pub fn play_file(path: &str) -> Result<Stream, Box<dyn std::error::Error>> {
    // Get audio device first to determine target sample rate
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No default output device available")?;

    let config = device.default_output_config()?;
    let output_sample_rate = config.sample_rate().0;

    // Decode the audio file with automatic resampling to device sample rate
    tracing::info!("Loading audio file: {}", path);
    let buffer = decoder::decode_file(path, Some(output_sample_rate), ResamplerQuality::default())?;

    tracing::info!(
        "Loaded: {} channels, {} Hz, {} frames ({:.2}s)",
        buffer.channels,
        buffer.sample_rate,
        buffer.frames,
        buffer.frames as f32 / buffer.sample_rate as f32
    );

    let output_channels = config.channels() as usize;

    tracing::info!("Initializing audio stream:");
    tracing::info!("  Device: {}", device.name()?);
    tracing::info!("  Sample rate: {} Hz", output_sample_rate);
    tracing::info!("  Channels: {}", output_channels);

    // Wrap buffer in Arc for sharing with callback
    let buffer = Arc::new(buffer);
    let buffer_clone = buffer.clone();

    // Atomic position tracker (frames, not samples)
    let position = Arc::new(AtomicUsize::new(0));
    let position_clone = position.clone();

    let stream = device.build_output_stream(
        &config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            // Audio callback - play from buffer
            let frames_needed = data.len() / output_channels;
            let current_pos = position_clone.load(Ordering::Relaxed);

            for frame_idx in 0..frames_needed {
                let buffer_frame = current_pos + frame_idx;

                if buffer_frame >= buffer_clone.frames {
                    // End of buffer - output silence
                    for ch in 0..output_channels {
                        data[frame_idx * output_channels + ch] = 0.0;
                    }
                    continue;
                }

                // Read frame from buffer
                // Handle channel mismatch by simple mapping
                for out_ch in 0..output_channels {
                    // Map output channel to input channel (wrap if necessary)
                    let in_ch = out_ch % buffer_clone.channels;
                    let src_idx = buffer_frame * buffer_clone.channels + in_ch;
                    let dst_idx = frame_idx * output_channels + out_ch;

                    if src_idx < buffer_clone.data.len() {
                        data[dst_idx] = buffer_clone.data[src_idx];
                    } else {
                        data[dst_idx] = 0.0;
                    }
                }
            }

            // Update position
            let new_pos = current_pos + frames_needed;
            position_clone.store(new_pos, Ordering::Relaxed);
        },
        move |err| {
            tracing::error!("Stream error: {}", err);
        },
        None,
    )?;

    stream.play()?;

    // Log when playback should finish
    let duration_secs = buffer.frames as f32 / buffer.sample_rate as f32;
    tracing::info!("Playback started - duration: {:.2}s", duration_secs);

    Ok(stream)
}

/// Test mixer with multiple simultaneous samples
pub fn test_mixer() -> Result<Stream, Box<dyn std::error::Error>> {
    // Get audio device
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No default output device available")?;

    let config = device.default_output_config()?;
    let output_sample_rate = config.sample_rate().0;
    let output_channels = config.channels() as usize;

    tracing::info!("Initializing mixer test:");
    tracing::info!("  Device: {}", device.name()?);
    tracing::info!("  Sample rate: {} Hz", output_sample_rate);
    tracing::info!("  Channels: {}", output_channels);

    // Load test files if they exist, otherwise generate test tones
    let test_files = [
        "/Users/bandrews/src/mqttaudio/tests/audio/test_440hz_2s.wav",
        "/Users/bandrews/src/mqttaudio/tests/audio/test_880hz_48khz.wav",
    ];

    let mut buffers = Vec::new();
    for (i, file_path) in test_files.iter().enumerate() {
        match decoder::decode_file(
            file_path,
            Some(output_sample_rate),
            ResamplerQuality::default(),
        ) {
            Ok(buffer) => {
                tracing::info!(
                    "Loaded sample {}: {} channels, {} frames ({:.2}s)",
                    i + 1,
                    buffer.channels,
                    buffer.frames,
                    buffer.frames as f32 / buffer.sample_rate as f32
                );
                buffers.push(Arc::new(buffer));
            }
            Err(e) => {
                tracing::warn!("Could not load {}: {}", file_path, e);
            }
        }
    }

    if buffers.is_empty() {
        return Err("No test files could be loaded for mixer test".into());
    }

    // Create active samples with different volumes to demonstrate mixing
    let mut active_samples = Vec::new();
    for (i, buffer) in buffers.iter().enumerate() {
        let volume = 0.3; // Reduce volume to avoid clipping when mixing
        let sample = ActiveSample::new(
            (i + 1) as u64,
            "test_mixer".to_string(),
            buffer.clone(),
            volume,
            1.0, // Voice volume
            format!("test_file_{}.wav", i + 1),
        );
        active_samples.push(sample);
        tracing::info!(
            "Added sample {} to mixer at {}% volume",
            i + 1,
            (volume * 100.0) as u32
        );
    }

    // Create mixer state
    let mixer_state = Arc::new(Mutex::new(MixerState {
        active_samples,
        live_inputs: Vec::new(),
        output_channels,
        ducking_engine: None,
        bass_management: None,
    }));

    let mixer_state_clone = mixer_state.clone();

    // Build audio stream with mixer
    let stream = device.build_output_stream(
        &config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            // Audio callback - mix all active samples
            let mut state = mixer_state_clone.lock();
            crate::audio::mixer::mix_audio(data, &mut state);

            // Remove finished samples
            state.active_samples.retain(|s| !s.is_finished());
        },
        move |err| {
            tracing::error!("Stream error: {}", err);
        },
        None,
    )?;

    stream.play()?;

    let num_samples = mixer_state.lock().active_samples.len();
    tracing::info!("Mixer started with {} simultaneous samples", num_samples);

    Ok(stream)
}
