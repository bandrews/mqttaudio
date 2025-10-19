// ABOUTME: Audio engine coordinator managing playback, caching, and state.
// ABOUTME: Handles sample loading, voice management, and mixer state updates.

use crate::audio::decoder;
use crate::audio::mixer::{ActiveSample, MixerState};
use crate::audio::types::DeviceConfig;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// List available audio output devices
pub fn list_devices() {
    let host = cpal::default_host();

    println!("Available audio output devices:");
    match host.output_devices() {
        Ok(devices) => {
            for (i, device) in devices.enumerate() {
                if let Ok(name) = device.name() {
                    println!("  {}. {}", i, name);

                    if let Ok(config) = device.default_output_config() {
                        println!("     Sample rate: {} Hz", config.sample_rate().0);
                        println!("     Channels: {}", config.channels());
                    }
                }
            }
        }
        Err(e) => eprintln!("Error listing devices: {}", e),
    }
}

/// Get default device configuration
pub fn get_default_device_config() -> Result<DeviceConfig, Box<dyn std::error::Error>> {
    let host = cpal::default_host();
    let device = host.default_output_device()
        .ok_or("No default output device available")?;

    let config = device.default_output_config()?;

    Ok(DeviceConfig {
        sample_rate: config.sample_rate().0,
        channels: config.channels() as usize,
        buffer_size: 512, // Default buffer size
    })
}

/// Initialize audio output stream with a test sine wave
pub fn init_test_sine_wave() -> Result<Stream, Box<dyn std::error::Error>> {
    let host = cpal::default_host();
    let device = host.default_output_device()
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
                    PHASE.fetch_sub((2.0 * std::f32::consts::PI * 1000.0) as u32, Ordering::Relaxed);
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
    let device = host.default_output_device()
        .ok_or("No default output device available")?;

    let config = device.default_output_config()?;
    let output_sample_rate = config.sample_rate().0;

    // Decode the audio file with automatic resampling to device sample rate
    tracing::info!("Loading audio file: {}", path);
    let buffer = decoder::decode_file(path, Some(output_sample_rate))?;

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
    let device = host.default_output_device()
        .ok_or("No default output device available")?;

    let config = device.default_output_config()?;
    let output_sample_rate = config.sample_rate().0;
    let output_channels = config.channels() as usize;

    tracing::info!("Initializing mixer test:");
    tracing::info!("  Device: {}", device.name()?);
    tracing::info!("  Sample rate: {} Hz", output_sample_rate);
    tracing::info!("  Channels: {}", output_channels);

    // Load test files if they exist, otherwise generate test tones
    let test_files = vec![
        "/Users/bandrews/src/mqttaudio/tests/audio/test_440hz_2s.wav",
        "/Users/bandrews/src/mqttaudio/tests/audio/test_880hz_48khz.wav",
    ];

    let mut buffers = Vec::new();
    for (i, file_path) in test_files.iter().enumerate() {
        match decoder::decode_file(file_path, Some(output_sample_rate)) {
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
        );
        active_samples.push(sample);
        tracing::info!("Added sample {} to mixer at {}% volume", i + 1, (volume * 100.0) as u32);
    }

    // Create mixer state
    let mixer_state = Arc::new(Mutex::new(MixerState {
        active_samples,
        output_channels,
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
