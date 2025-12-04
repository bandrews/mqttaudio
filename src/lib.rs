// ABOUTME: Library interface exposing mqttaudio modules for testing and benchmarking.
// ABOUTME: Provides public access to core audio processing and command structures.

pub mod audio {
    pub mod bass_management;
    pub mod decoder;
    pub mod device;
    pub mod ducking;
    pub mod engine;
    pub mod input;
    pub mod mixer;
    pub mod pitch_correction;
    pub mod resampler;
    pub mod types;

    #[cfg(target_os = "linux")]
    pub mod alsa_probe;
}

pub mod cache {
    pub mod disk;
    pub mod memory;
}

pub mod mqtt {
    pub mod commands;
    pub mod client;
}

pub mod voice;
pub mod config;
