// ABOUTME: Audio engine coordinator managing playback, caching, and state.
// ABOUTME: Handles sample loading, voice management, and mixer state updates.

use crate::audio::decoder;
use crate::audio::mixer::{ActiveSample, MixerState};
use crate::audio::types::DeviceConfig;
use crate::config::ResamplerQuality;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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
        if name.starts_with(prefix) {
            let rest = &name[prefix.len()..];

            // Try "CARD=name" format first
            if rest.starts_with("CARD=") {
                let card_part = &rest[5..];
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

/// Find a stream config for the device with the requested channel count
/// If requested_channels is None, uses the maximum available (capped at 32 for sanity)
/// If requested_sample_rate is None, uses the device's preferred sample rate
pub fn find_output_config(
    device: &cpal::Device,
    requested_channels: Option<usize>,
    requested_sample_rate: Option<u32>,
) -> Result<cpal::StreamConfig, Box<dyn std::error::Error>> {
    use cpal::SampleRate;

    // Get all supported configs
    let supported_configs: Vec<_> = device.supported_output_configs()?.collect();

    if supported_configs.is_empty() {
        return Err("No supported output configurations found".into());
    }

    // Determine target channels
    let target_channels = match requested_channels {
        Some(ch) => ch as u16,
        None => {
            // Find maximum available channels, but cap at 32 for sanity
            // (ALSA plugins may report absurdly high values)
            supported_configs
                .iter()
                .map(|c| c.channels())
                .filter(|&ch| ch <= 32)
                .max()
                .unwrap_or(2)
        }
    };

    // Find configs that match the requested channel count
    let matching_configs: Vec<_> = supported_configs
        .iter()
        .filter(|c| c.channels() == target_channels)
        .collect();

    if matching_configs.is_empty() {
        // No exact match - list available channel counts (capped for display)
        let available: Vec<_> = supported_configs
            .iter()
            .map(|c| c.channels())
            .filter(|&ch| ch <= 32)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        return Err(format!(
            "No configuration found for {} channels. Available: {:?}",
            target_channels, available
        )
        .into());
    }

    // Determine target sample rate
    let target_sample_rate = requested_sample_rate.unwrap_or_else(|| {
        // Use the default config's sample rate if possible, otherwise pick a common rate
        device
            .default_output_config()
            .map(|c| c.sample_rate().0)
            .unwrap_or(48000)
    });

    // Find the best matching config for sample rate
    // Prefer configs with reasonable sample rate ranges, but accept any if needed
    let best_config = matching_configs.iter().find(|c| {
        let min = c.min_sample_rate().0;
        let max = c.max_sample_rate().0;
        target_sample_rate >= min && target_sample_rate <= max
    });

    match best_config {
        Some(config_range) => {
            let config = config_range.with_sample_rate(SampleRate(target_sample_rate));
            Ok(cpal::StreamConfig {
                channels: config.channels(),
                sample_rate: config.sample_rate(),
                buffer_size: cpal::BufferSize::Default,
            })
        }
        None => {
            // Sample rate not directly supported - clamp to valid range
            let config_range = matching_configs[0];
            let min_rate = config_range.min_sample_rate().0;
            let max_rate = config_range.max_sample_rate().0;
            // Clamp target to valid range, but also cap max at 384kHz for sanity
            let capped_max = max_rate.min(384000);
            let sample_rate = target_sample_rate.max(min_rate).min(capped_max);
            let config = config_range.with_sample_rate(SampleRate(sample_rate));
            Ok(cpal::StreamConfig {
                channels: config.channels(),
                sample_rate: config.sample_rate(),
                buffer_size: cpal::BufferSize::Default,
            })
        }
    }
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
            let mut state = mixer_state_clone.lock().unwrap();
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

    let num_samples = mixer_state.lock().unwrap().active_samples.len();
    tracing::info!("Mixer started with {} simultaneous samples", num_samples);

    Ok(stream)
}
