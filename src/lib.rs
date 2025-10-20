// ABOUTME: Library interface exposing mqttaudio modules for testing and benchmarking.
// ABOUTME: Provides public access to core audio processing and command structures.

pub mod audio {
    pub mod mixer;
    pub mod types;
    pub mod decoder;
    pub mod resampler;
    pub mod engine;
    pub mod ducking;
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
