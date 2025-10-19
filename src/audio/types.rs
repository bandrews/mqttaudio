// ABOUTME: Core audio data types and structures.
// ABOUTME: Defines buffers, samples, and audio configuration types.

use std::sync::Arc;

/// Decoded audio buffer in f32 format, ready for playback
#[derive(Clone, Debug)]
pub struct DecodedBuffer {
    /// Interleaved PCM samples (f32 format)
    pub data: Vec<f32>,
    /// Number of audio channels
    pub channels: usize,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Total number of frames
    pub frames: usize,
}

impl DecodedBuffer {
    pub fn new(data: Vec<f32>, channels: usize, sample_rate: u32) -> Self {
        let frames = data.len() / channels;
        Self {
            data,
            channels,
            sample_rate,
            frames,
        }
    }
}

/// Device configuration
#[derive(Clone, Debug)]
pub struct DeviceConfig {
    pub sample_rate: u32,
    pub channels: usize,
    pub buffer_size: usize,
}
