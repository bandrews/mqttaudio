// ABOUTME: Library interface exposing mqttaudio modules for testing and benchmarking.
// ABOUTME: Provides public access to core audio processing and command structures.

pub mod audio {
    pub mod bass_management;
    pub mod chunked_resampler;
    pub mod decoder;
    pub mod device;
    pub mod device_select;
    pub mod ducking;
    pub mod engine;
    pub mod input;
    pub mod mixer;
    pub mod pitch_correction;
    pub mod rebuild;
    pub mod resampler;
    pub mod streamed_source;
    pub mod streaming;
    pub mod streaming_decoder;
    pub mod types;

    /// Offline render harness and signal-analysis helpers, available to tests/benches.
    pub mod test_support;

    #[cfg(target_os = "linux")]
    pub mod alsa_probe;
}

pub mod cache;

pub mod mqtt {
    pub mod client;
    pub mod commands;
}

pub mod http;

pub mod config;
pub mod rt_engine;
pub mod voice;
