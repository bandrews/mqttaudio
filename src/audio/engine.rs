// ABOUTME: Audio engine coordinator managing playback, caching, and state.
// ABOUTME: Handles sample loading, voice management, and mixer state updates.

use crate::audio::types::DeviceConfig;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::sync::atomic::{AtomicU32, Ordering};

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
