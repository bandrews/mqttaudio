// ABOUTME: Audio subsystem module for mqttaudio.
// ABOUTME: Manages audio playback, mixing, decoding, and resampling.

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
