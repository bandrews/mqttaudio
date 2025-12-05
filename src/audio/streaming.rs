// ABOUTME: Streaming audio buffer types for progressive loading.
// ABOUTME: Enables playback to begin before full file is loaded.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::Notify;

use crate::audio::types::DecodedBuffer;

/// Buffer type that can hold either complete or streaming audio.
#[derive(Clone)]
pub enum SampleBuffer {
    /// Fully loaded, immutable audio buffer (cached)
    Complete(Arc<DecodedBuffer>),
    /// Audio buffer still being loaded
    Streaming(Arc<RwLock<StreamingBuffer>>),
}

/// State of a streaming buffer's loading progress
#[derive(Clone, Debug, PartialEq)]
pub enum LoadingState {
    /// Still loading data
    Loading,
    /// Loading completed successfully
    Complete,
    /// Loading failed with error
    Error(String),
}

/// Audio buffer that grows as data is loaded.
/// Designed for concurrent read (audio thread) and write (loader thread).
pub struct StreamingBuffer {
    /// Interleaved PCM samples (f32 format)
    data: Vec<f32>,
    /// Number of audio channels
    pub channels: usize,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Number of frames currently available for playback
    frames_available: AtomicUsize,
    /// Estimated total frames (from Content-Length header, if known)
    pub total_frames: Option<usize>,
    /// Current loading state
    pub state: LoadingState,
    /// Notifies waiters when new data is available
    data_available: Arc<Notify>,
}

impl StreamingBuffer {
    /// Create a new streaming buffer with expected size estimate.
    pub fn new(channels: usize, sample_rate: u32, estimated_frames: Option<usize>) -> Self {
        let initial_capacity = estimated_frames.unwrap_or(48000) * channels; // Default 1 second
        Self {
            data: Vec::with_capacity(initial_capacity),
            channels,
            sample_rate,
            frames_available: AtomicUsize::new(0),
            total_frames: estimated_frames,
            state: LoadingState::Loading,
            data_available: Arc::new(Notify::new()),
        }
    }

    /// Append decoded audio data to the buffer.
    /// Called by loader thread with interleaved samples.
    pub fn append(&mut self, samples: &[f32]) {
        debug_assert!(
            samples.len() % self.channels == 0,
            "Sample count must be divisible by channel count"
        );
        let new_frames = samples.len() / self.channels;
        self.data.extend_from_slice(samples);
        // Use Release ordering so readers see the new data
        self.frames_available.fetch_add(new_frames, Ordering::Release);
        self.data_available.notify_waiters();
    }

    /// Mark loading as complete.
    pub fn mark_complete(&mut self) {
        self.state = LoadingState::Complete;
        self.data_available.notify_waiters();
    }

    /// Mark loading as failed with an error message.
    pub fn mark_error(&mut self, error: String) {
        self.state = LoadingState::Error(error);
        self.data_available.notify_waiters();
    }

    /// Get the number of frames currently available for playback.
    /// Uses Acquire ordering to synchronize with append().
    pub fn frames_available(&self) -> usize {
        self.frames_available.load(Ordering::Acquire)
    }

    /// Check if the specified frame range is available.
    pub fn is_range_available(&self, start_frame: usize, end_frame: usize) -> bool {
        start_frame < self.frames_available() && end_frame <= self.frames_available()
    }

    /// Get a sample value if available, or None if not yet loaded.
    /// This is the core method for audio callback access.
    #[inline]
    pub fn get_sample(&self, frame: usize, channel: usize) -> Option<f32> {
        if frame >= self.frames_available() || channel >= self.channels {
            return None;
        }
        let idx = frame * self.channels + channel;
        self.data.get(idx).copied()
    }

    /// Get the Notify handle for waiting on data availability.
    pub fn notifier(&self) -> Arc<Notify> {
        Arc::clone(&self.data_available)
    }

    /// Check if loading is complete.
    pub fn is_complete(&self) -> bool {
        matches!(self.state, LoadingState::Complete)
    }

    /// Check if loading has errored.
    pub fn has_error(&self) -> bool {
        matches!(self.state, LoadingState::Error(_))
    }

    /// Get a reference to the raw sample data.
    /// Used for promoting streaming buffer to memory cache.
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// Convert to a DecodedBuffer once loading is complete.
    /// Returns None if still loading or if there was an error.
    pub fn into_decoded_buffer(self) -> Option<DecodedBuffer> {
        if !matches!(self.state, LoadingState::Complete) {
            return None;
        }
        Some(DecodedBuffer::new(self.data, self.channels, self.sample_rate))
    }
}

impl From<Arc<DecodedBuffer>> for SampleBuffer {
    fn from(buffer: Arc<DecodedBuffer>) -> Self {
        SampleBuffer::Complete(buffer)
    }
}

impl SampleBuffer {
    /// Create a complete buffer from a DecodedBuffer.
    pub fn complete(buffer: Arc<DecodedBuffer>) -> Self {
        SampleBuffer::Complete(buffer)
    }

    /// Get the number of channels in the buffer.
    pub fn channels(&self) -> usize {
        match self {
            SampleBuffer::Complete(buf) => buf.channels,
            SampleBuffer::Streaming(buf) => {
                buf.try_read().map(|b| b.channels).unwrap_or(0)
            }
        }
    }

    /// Get the sample rate of the buffer.
    pub fn sample_rate(&self) -> u32 {
        match self {
            SampleBuffer::Complete(buf) => buf.sample_rate,
            SampleBuffer::Streaming(buf) => {
                buf.try_read().map(|b| b.sample_rate).unwrap_or(0)
            }
        }
    }

    /// Get the total number of frames (for complete) or frames available (for streaming).
    pub fn frames(&self) -> usize {
        match self {
            SampleBuffer::Complete(buf) => buf.frames,
            SampleBuffer::Streaming(buf) => {
                buf.try_read().map(|b| b.frames_available()).unwrap_or(0)
            }
        }
    }

    /// Get a sample value, returning 0.0 (silence) if not available.
    /// This method is designed for audio callback usage - never blocks.
    #[inline]
    pub fn get_sample_or_silence(&self, frame: usize, channel: usize) -> f32 {
        match self {
            SampleBuffer::Complete(buf) => {
                let idx = frame * buf.channels + channel;
                buf.data.get(idx).copied().unwrap_or(0.0)
            }
            SampleBuffer::Streaming(buf) => {
                match buf.try_read() {
                    Ok(guard) => guard.get_sample(frame, channel).unwrap_or(0.0),
                    Err(_) => 0.0, // Lock contention - return silence
                }
            }
        }
    }

    /// Check if this is a complete (fully loaded) buffer.
    pub fn is_complete(&self) -> bool {
        match self {
            SampleBuffer::Complete(_) => true,
            SampleBuffer::Streaming(buf) => {
                buf.try_read().map(|b| b.is_complete()).unwrap_or(false)
            }
        }
    }

    /// Try to convert to Arc<DecodedBuffer> if complete.
    /// Returns None if streaming or if streaming buffer isn't complete yet.
    pub fn as_complete(&self) -> Option<Arc<DecodedBuffer>> {
        match self {
            SampleBuffer::Complete(buf) => Some(Arc::clone(buf)),
            SampleBuffer::Streaming(_) => None,
        }
    }
}

/// Minimum frames of data required before starting playback.
/// At 48kHz, 2560 frames = ~53ms = ~5 callback buffers at 512 frames/callback.
pub const MIN_BUFFER_FRAMES: usize = 2560;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_streaming_buffer_creation() {
        let buf = StreamingBuffer::new(2, 48000, Some(48000));
        assert_eq!(buf.channels, 2);
        assert_eq!(buf.sample_rate, 48000);
        assert_eq!(buf.frames_available(), 0);
        assert_eq!(buf.total_frames, Some(48000));
        assert_eq!(buf.state, LoadingState::Loading);
    }

    #[test]
    fn test_streaming_buffer_append() {
        let mut buf = StreamingBuffer::new(2, 48000, None);

        // Append 10 frames of stereo audio with exact values
        let samples: Vec<f32> = vec![
            0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9,
            1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 1.8, 1.9,
        ];
        buf.append(&samples);

        assert_eq!(buf.frames_available(), 10);

        // Verify sample retrieval
        assert_eq!(buf.get_sample(0, 0), Some(0.0));
        assert_eq!(buf.get_sample(0, 1), Some(0.1));
        assert_eq!(buf.get_sample(9, 0), Some(1.8));
        assert_eq!(buf.get_sample(9, 1), Some(1.9));

        // Out of range
        assert_eq!(buf.get_sample(10, 0), None);
        assert_eq!(buf.get_sample(0, 2), None);
    }

    #[test]
    fn test_streaming_buffer_multiple_appends() {
        let mut buf = StreamingBuffer::new(1, 48000, None);

        buf.append(&[0.1, 0.2, 0.3]);
        assert_eq!(buf.frames_available(), 3);

        buf.append(&[0.4, 0.5]);
        assert_eq!(buf.frames_available(), 5);

        assert_eq!(buf.get_sample(3, 0), Some(0.4));
        assert_eq!(buf.get_sample(4, 0), Some(0.5));
    }

    #[test]
    fn test_streaming_buffer_range_check() {
        let mut buf = StreamingBuffer::new(2, 48000, None);
        buf.append(&[0.0; 20]); // 10 frames

        assert!(buf.is_range_available(0, 5));
        assert!(buf.is_range_available(0, 10));
        assert!(!buf.is_range_available(0, 11));
        assert!(!buf.is_range_available(5, 15));
    }

    #[test]
    fn test_streaming_buffer_state_transitions() {
        let mut buf = StreamingBuffer::new(2, 48000, None);

        assert!(!buf.is_complete());
        assert!(!buf.has_error());

        buf.mark_complete();
        assert!(buf.is_complete());
        assert!(!buf.has_error());
    }

    #[test]
    fn test_streaming_buffer_error_state() {
        let mut buf = StreamingBuffer::new(2, 48000, None);

        buf.mark_error("Connection failed".to_string());
        assert!(!buf.is_complete());
        assert!(buf.has_error());
        assert_eq!(buf.state, LoadingState::Error("Connection failed".to_string()));
    }

    #[test]
    fn test_streaming_buffer_to_decoded() {
        let mut buf = StreamingBuffer::new(2, 48000, None);
        buf.append(&[0.5; 20]); // 10 frames

        // Can't convert while still loading
        assert!(StreamingBuffer::new(2, 48000, None).into_decoded_buffer().is_none());

        buf.mark_complete();
        let decoded = buf.into_decoded_buffer().unwrap();
        assert_eq!(decoded.channels, 2);
        assert_eq!(decoded.sample_rate, 48000);
        assert_eq!(decoded.frames, 10);
        assert_eq!(decoded.data.len(), 20);
    }

    #[test]
    fn test_sample_buffer_complete() {
        let decoded = Arc::new(DecodedBuffer::new(vec![0.5; 20], 2, 48000));
        let buf = SampleBuffer::Complete(decoded);

        assert_eq!(buf.channels(), 2);
        assert_eq!(buf.sample_rate(), 48000);
        assert_eq!(buf.frames(), 10);
        assert!(buf.is_complete());
        assert!(buf.as_complete().is_some());
    }

    #[test]
    fn test_sample_buffer_streaming() {
        let mut streaming = StreamingBuffer::new(2, 48000, None);
        streaming.append(&[0.5; 20]);
        let buf = SampleBuffer::Streaming(Arc::new(RwLock::new(streaming)));

        assert_eq!(buf.channels(), 2);
        assert_eq!(buf.sample_rate(), 48000);
        assert_eq!(buf.frames(), 10);
        assert!(!buf.is_complete());
        assert!(buf.as_complete().is_none());
    }

    #[test]
    fn test_sample_buffer_get_sample_or_silence() {
        // Complete buffer
        let decoded = Arc::new(DecodedBuffer::new(vec![0.5, 0.6], 2, 48000));
        let buf = SampleBuffer::Complete(decoded);

        assert_eq!(buf.get_sample_or_silence(0, 0), 0.5);
        assert_eq!(buf.get_sample_or_silence(0, 1), 0.6);
        assert_eq!(buf.get_sample_or_silence(1, 0), 0.0); // Out of range = silence

        // Streaming buffer
        let mut streaming = StreamingBuffer::new(2, 48000, None);
        streaming.append(&[0.3, 0.4]);
        let buf = SampleBuffer::Streaming(Arc::new(RwLock::new(streaming)));

        assert_eq!(buf.get_sample_or_silence(0, 0), 0.3);
        assert_eq!(buf.get_sample_or_silence(0, 1), 0.4);
        assert_eq!(buf.get_sample_or_silence(1, 0), 0.0); // Not loaded yet
    }

    #[test]
    fn test_min_buffer_frames_constant() {
        // Verify the constant is reasonable for 48kHz playback
        // 2560 frames at 48000Hz = 53.3ms
        let duration_ms = MIN_BUFFER_FRAMES as f64 / 48000.0 * 1000.0;
        assert!(duration_ms > 50.0);
        assert!(duration_ms < 60.0);
    }

    #[test]
    fn test_concurrent_read_write() {
        use std::thread;
        use std::time::Duration;

        let streaming = StreamingBuffer::new(2, 48000, None);
        let buffer = Arc::new(RwLock::new(streaming));
        let sample_buffer = SampleBuffer::Streaming(Arc::clone(&buffer));

        // Writer thread simulates loader
        let writer_buffer = Arc::clone(&buffer);
        let writer = thread::spawn(move || {
            for chunk in 0..100 {
                let samples: Vec<f32> = (0..200).map(|i| (chunk * 200 + i) as f32).collect();
                {
                    let mut guard = writer_buffer.write().unwrap();
                    guard.append(&samples);
                }
                // Small delay to simulate IO
                thread::sleep(Duration::from_micros(100));
            }
            writer_buffer.write().unwrap().mark_complete();
        });

        // Reader thread simulates audio callback using try_read
        let reader = thread::spawn(move || {
            let mut reads = 0;
            let mut successful_samples = 0;
            for _ in 0..1000 {
                // Simulate audio callback reading samples
                let sample = sample_buffer.get_sample_or_silence(reads % 10000, 0);
                reads += 1;
                if sample != 0.0 {
                    successful_samples += 1;
                }
                thread::sleep(Duration::from_micros(10));
            }
            (reads, successful_samples)
        });

        writer.join().unwrap();
        let (reads, successful) = reader.join().unwrap();

        // Verify that:
        // 1. Reader completed all iterations (never blocked indefinitely)
        assert_eq!(reads, 1000);
        // 2. At least some reads succeeded (got actual samples)
        assert!(successful > 0, "Expected some successful reads, got {}", successful);
    }

    #[test]
    fn test_try_read_contention() {
        // Test that try_read returns None during write lock contention
        let streaming = StreamingBuffer::new(2, 48000, None);
        let buffer = Arc::new(RwLock::new(streaming));

        // Hold write lock
        let _guard = buffer.write().unwrap();

        // try_read should return TryLockError
        assert!(buffer.try_read().is_err());
    }
}
