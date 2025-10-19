// ABOUTME: Audio engine coordinator managing playback, caching, and state.
// ABOUTME: Handles sample loading, voice management, and mixer state updates.

use crate::audio::types::DeviceConfig;
use cpal::traits::{DeviceTrait, HostTrait};

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
