// ABOUTME: Iterator-based audio decoder for streaming playback.
// ABOUTME: Yields decoded chunks progressively, enabling playback before full load.

// Allow dead_code until Phase 10 connects streaming to main.rs.
// This code is tested via integration tests and will be integrated soon.
#![allow(dead_code)]

use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::audio::chunked_resampler::{ChunkedResampleError, ChunkedResampler};
use crate::config::ResamplerQuality;

/// Error type for streaming decoder operations
#[derive(Debug)]
pub enum StreamingDecodeError {
    /// I/O error during reading
    Io(std::io::Error),
    /// Symphonia decoding error
    Decode(SymphoniaError),
    /// Resampling error
    Resample(ChunkedResampleError),
    /// No audio track found in the media
    NoAudioTrack,
    /// Audio format not supported
    UnsupportedFormat(String),
}

impl std::fmt::Display for StreamingDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamingDecodeError::Io(e) => write!(f, "I/O error: {}", e),
            StreamingDecodeError::Decode(e) => write!(f, "Decode error: {}", e),
            StreamingDecodeError::Resample(e) => write!(f, "Resample error: {}", e),
            StreamingDecodeError::NoAudioTrack => write!(f, "No audio track found"),
            StreamingDecodeError::UnsupportedFormat(msg) => {
                write!(f, "Unsupported format: {}", msg)
            }
        }
    }
}

impl std::error::Error for StreamingDecodeError {}

impl From<std::io::Error> for StreamingDecodeError {
    fn from(err: std::io::Error) -> Self {
        StreamingDecodeError::Io(err)
    }
}

impl From<SymphoniaError> for StreamingDecodeError {
    fn from(err: SymphoniaError) -> Self {
        StreamingDecodeError::Decode(err)
    }
}

impl From<ChunkedResampleError> for StreamingDecodeError {
    fn from(err: ChunkedResampleError) -> Self {
        StreamingDecodeError::Resample(err)
    }
}

/// Iterator-based audio decoder that yields chunks of decoded audio.
/// Optionally resamples to a target sample rate using ChunkedResampler.
pub struct StreamingDecoder {
    /// Symphonia format reader
    format: Box<dyn FormatReader>,
    /// Symphonia codec decoder
    decoder: Box<dyn Decoder>,
    /// Track ID we're decoding
    track_id: u32,
    /// Number of audio channels
    channels: usize,
    /// Source sample rate
    sample_rate: u32,
    /// Target sample rate (after resampling)
    target_sample_rate: u32,
    /// Optional resampler for sample rate conversion
    resampler: Option<ChunkedResampler>,
    /// Whether we've reached end of stream
    finished: bool,
    /// Estimated total frames (if known from metadata)
    estimated_frames: Option<u64>,
}

impl StreamingDecoder {
    /// Create a streaming decoder from a readable source.
    ///
    /// # Arguments
    /// * `reader` - Any readable source (file, network stream, etc.)
    /// * `hint` - Optional hint about the format (extension, mime type)
    /// * `target_sample_rate` - Target sample rate, or None to use source rate
    /// * `resampler_quality` - Quality preset for resampling
    pub fn new<R: MediaSource + 'static>(
        reader: R,
        hint: Option<&Hint>,
        target_sample_rate: Option<u32>,
        resampler_quality: ResamplerQuality,
    ) -> Result<Self, StreamingDecodeError> {
        // Create media source stream
        let mss = MediaSourceStream::new(Box::new(reader), Default::default());

        // Probe the format
        let format_opts = FormatOptions::default();
        let metadata_opts = MetadataOptions::default();

        let default_hint = Hint::new();
        let hint = hint.unwrap_or(&default_hint);

        let probed = symphonia::default::get_probe()
            .format(hint, mss, &format_opts, &metadata_opts)?;

        let format = probed.format;

        // Find the default audio track
        let track = format
            .default_track()
            .ok_or(StreamingDecodeError::NoAudioTrack)?;

        let track_id = track.id;
        let codec_params = &track.codec_params;

        // Extract audio parameters
        let channels = codec_params
            .channels
            .ok_or_else(|| {
                StreamingDecodeError::UnsupportedFormat("Missing channel info".to_string())
            })?
            .count();

        let sample_rate = codec_params.sample_rate.ok_or_else(|| {
            StreamingDecodeError::UnsupportedFormat("Missing sample rate".to_string())
        })?;

        // Get estimated frames from metadata if available
        let estimated_frames = codec_params.n_frames;

        // Create decoder
        let decoder_opts = DecoderOptions::default();
        let decoder = symphonia::default::get_codecs().make(codec_params, &decoder_opts)?;

        // Determine target sample rate
        let target_rate = target_sample_rate.unwrap_or(sample_rate);

        // Create resampler if needed
        let resampler = if sample_rate != target_rate {
            Some(ChunkedResampler::new(
                sample_rate,
                target_rate,
                channels,
                resampler_quality,
            )?)
        } else {
            None
        };

        tracing::debug!(
            "StreamingDecoder: {} channels, {} Hz -> {} Hz, estimated {} frames",
            channels,
            sample_rate,
            target_rate,
            estimated_frames.map(|f| f.to_string()).unwrap_or_else(|| "unknown".to_string())
        );

        Ok(Self {
            format,
            decoder,
            track_id,
            channels,
            sample_rate,
            target_sample_rate: target_rate,
            resampler,
            finished: false,
            estimated_frames,
        })
    }

    /// Get the number of audio channels
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Get the source sample rate (before resampling)
    pub fn source_sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Get the target sample rate (after resampling)
    pub fn sample_rate(&self) -> u32 {
        self.target_sample_rate
    }

    /// Get the estimated total frames (if known from metadata)
    /// This is an estimate based on file metadata and may not be exact.
    pub fn estimated_frames(&self) -> Option<u64> {
        if let Some(frames) = self.estimated_frames {
            if self.resampler.is_some() {
                // Adjust for resampling ratio
                let ratio = self.target_sample_rate as f64 / self.sample_rate as f64;
                Some((frames as f64 * ratio) as u64)
            } else {
                Some(frames)
            }
        } else {
            None
        }
    }

    /// Check if the decoder has finished (no more data to yield)
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Decode the next chunk of audio.
    /// Returns None when the stream is exhausted.
    fn decode_next_chunk(&mut self) -> Result<Option<Vec<f32>>, StreamingDecodeError> {
        if self.finished {
            return Ok(None);
        }

        loop {
            // Get the next packet
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    // End of stream - flush resampler if present
                    self.finished = true;
                    if let Some(ref mut resampler) = self.resampler {
                        let flushed = resampler.flush()?;
                        if !flushed.is_empty() {
                            return Ok(Some(flushed));
                        }
                    }
                    return Ok(None);
                }
                Err(e) => return Err(StreamingDecodeError::Decode(e)),
            };

            // Skip packets that don't belong to our track
            if packet.track_id() != self.track_id {
                continue;
            }

            // Decode the packet
            let decoded = self.decoder.decode(&packet)?;

            // Convert to f32 interleaved samples
            let mut samples = Vec::new();
            convert_audio_buffer_to_f32(&decoded, &mut samples);

            if samples.is_empty() {
                continue;
            }

            // Apply resampling if needed
            if let Some(ref mut resampler) = self.resampler {
                if let Some(resampled) = resampler.push(&samples)? {
                    return Ok(Some(resampled));
                }
                // Not enough samples for a chunk yet, decode more
                continue;
            } else {
                return Ok(Some(samples));
            }
        }
    }
}

impl Iterator for StreamingDecoder {
    type Item = Result<Vec<f32>, StreamingDecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.decode_next_chunk() {
            Ok(Some(chunk)) => Some(Ok(chunk)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

/// Convert Symphonia audio buffer to interleaved f32 samples
fn convert_audio_buffer_to_f32(audio_buf: &AudioBufferRef, output: &mut Vec<f32>) {
    match audio_buf {
        AudioBufferRef::U8(buf) => {
            convert_typed_to_f32(buf, output, |s| (s as f32 - 128.0) / 128.0);
        }
        AudioBufferRef::U16(buf) => {
            convert_typed_to_f32(buf, output, |s| (s as f32 - 32768.0) / 32768.0);
        }
        AudioBufferRef::U24(buf) => {
            convert_typed_to_f32(buf, output, |s| (s.inner() as f32 - 8388608.0) / 8388608.0);
        }
        AudioBufferRef::U32(buf) => {
            convert_typed_to_f32(
                buf,
                output,
                |s| (s as f64 - 2147483648.0) as f32 / 2147483648.0,
            );
        }
        AudioBufferRef::S8(buf) => {
            convert_typed_to_f32(buf, output, |s| s as f32 / 128.0);
        }
        AudioBufferRef::S16(buf) => {
            convert_typed_to_f32(buf, output, |s| s as f32 / 32768.0);
        }
        AudioBufferRef::S24(buf) => {
            convert_typed_to_f32(buf, output, |s| s.inner() as f32 / 8388608.0);
        }
        AudioBufferRef::S32(buf) => {
            convert_typed_to_f32(buf, output, |s| s as f32 / 2147483648.0);
        }
        AudioBufferRef::F32(buf) => {
            convert_typed_to_f32(buf, output, |s| s);
        }
        AudioBufferRef::F64(buf) => {
            convert_typed_to_f32(buf, output, |s| s as f32);
        }
    }
}

/// Generic conversion function that handles interleaving
fn convert_typed_to_f32<S>(
    buf: &symphonia::core::audio::AudioBuffer<S>,
    output: &mut Vec<f32>,
    convert: impl Fn(S) -> f32,
) where
    S: symphonia::core::sample::Sample,
{
    let num_channels = buf.spec().channels.count();
    let num_frames = buf.frames();

    output.reserve(num_frames * num_channels);

    for frame_idx in 0..num_frames {
        for ch_idx in 0..num_channels {
            let sample = buf.chan(ch_idx)[frame_idx];
            output.push(convert(sample));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::path::Path;

    // Helper to get test audio file path
    fn test_audio_path() -> &'static str {
        "tests/audio/test_440hz_2s.wav"
    }

    fn ensure_test_file_exists() -> bool {
        Path::new(test_audio_path()).exists()
    }

    #[test]
    fn test_streaming_decoder_basic() {
        if !ensure_test_file_exists() {
            eprintln!("Skipping test: test audio file not found");
            return;
        }

        let file = File::open(test_audio_path()).unwrap();
        let mut hint = Hint::new();
        hint.with_extension("wav");

        let decoder = StreamingDecoder::new(
            file,
            Some(&hint),
            None, // No resampling
            ResamplerQuality::Fast,
        )
        .unwrap();

        assert_eq!(decoder.channels(), 2);
        assert_eq!(decoder.sample_rate(), 44100);
        assert!(!decoder.is_finished());
    }

    #[test]
    fn test_streaming_decoder_yields_chunks() {
        if !ensure_test_file_exists() {
            eprintln!("Skipping test: test audio file not found");
            return;
        }

        let file = File::open(test_audio_path()).unwrap();
        let mut hint = Hint::new();
        hint.with_extension("wav");

        let decoder = StreamingDecoder::new(file, Some(&hint), None, ResamplerQuality::Fast).unwrap();

        let mut total_samples = 0;
        let mut chunk_count = 0;

        for chunk_result in decoder {
            let chunk = chunk_result.unwrap();
            total_samples += chunk.len();
            chunk_count += 1;
        }

        // 2 seconds at 44100 Hz stereo = 176400 samples
        assert!(total_samples > 170000, "Expected ~176400 samples, got {}", total_samples);
        assert!(total_samples < 180000, "Expected ~176400 samples, got {}", total_samples);
        assert!(chunk_count > 1, "Expected multiple chunks, got {}", chunk_count);
    }

    #[test]
    fn test_streaming_decoder_with_resampling() {
        if !ensure_test_file_exists() {
            eprintln!("Skipping test: test audio file not found");
            return;
        }

        let file = File::open(test_audio_path()).unwrap();
        let mut hint = Hint::new();
        hint.with_extension("wav");

        let decoder = StreamingDecoder::new(
            file,
            Some(&hint),
            Some(48000), // Resample to 48kHz
            ResamplerQuality::Fast,
        )
        .unwrap();

        assert_eq!(decoder.channels(), 2);
        assert_eq!(decoder.sample_rate(), 48000);
        assert_eq!(decoder.source_sample_rate(), 44100);

        let mut total_samples = 0;
        for chunk_result in decoder {
            let chunk = chunk_result.unwrap();
            total_samples += chunk.len();
        }

        // 2 seconds at 48000 Hz stereo = 192000 samples
        // Allow some tolerance for resampler latency
        assert!(
            total_samples > 180000,
            "Expected ~192000 samples, got {}",
            total_samples
        );
        assert!(
            total_samples < 200000,
            "Expected ~192000 samples, got {}",
            total_samples
        );
    }

    #[test]
    fn test_streaming_decoder_same_sample_rate() {
        if !ensure_test_file_exists() {
            eprintln!("Skipping test: test audio file not found");
            return;
        }

        let file = File::open(test_audio_path()).unwrap();
        let mut hint = Hint::new();
        hint.with_extension("wav");

        // Request same sample rate as source - should not create resampler
        let decoder = StreamingDecoder::new(
            file,
            Some(&hint),
            Some(44100),
            ResamplerQuality::Fast,
        )
        .unwrap();

        assert_eq!(decoder.sample_rate(), 44100);
        assert_eq!(decoder.source_sample_rate(), 44100);

        let mut total_samples = 0;
        for chunk_result in decoder {
            let chunk = chunk_result.unwrap();
            total_samples += chunk.len();
        }

        // Should be same as source: 2 seconds at 44100 Hz stereo
        assert!(total_samples > 170000, "Got {} samples", total_samples);
    }

    #[test]
    fn test_streaming_decoder_finished_state() {
        if !ensure_test_file_exists() {
            eprintln!("Skipping test: test audio file not found");
            return;
        }

        let file = File::open(test_audio_path()).unwrap();
        let mut hint = Hint::new();
        hint.with_extension("wav");

        let mut decoder =
            StreamingDecoder::new(file, Some(&hint), None, ResamplerQuality::Fast).unwrap();

        assert!(!decoder.is_finished());

        // Consume all chunks
        while decoder.next().is_some() {}

        assert!(decoder.is_finished());
    }

    #[test]
    fn test_streaming_decoder_estimated_frames() {
        if !ensure_test_file_exists() {
            eprintln!("Skipping test: test audio file not found");
            return;
        }

        let file = File::open(test_audio_path()).unwrap();
        let mut hint = Hint::new();
        hint.with_extension("wav");

        let decoder =
            StreamingDecoder::new(file, Some(&hint), None, ResamplerQuality::Fast).unwrap();

        // WAV files typically have frame count in header
        if let Some(frames) = decoder.estimated_frames() {
            // 2 seconds at 44100 Hz = 88200 frames
            assert!(frames > 80000, "Expected ~88200 frames, got {}", frames);
            assert!(frames < 100000, "Expected ~88200 frames, got {}", frames);
        }
    }

    #[test]
    fn test_streaming_decoder_matches_full_decode() {
        if !ensure_test_file_exists() {
            eprintln!("Skipping test: test audio file not found");
            return;
        }

        use crate::audio::decoder::decode_file;

        // Full decode
        let full_result = decode_file(test_audio_path(), Some(48000), ResamplerQuality::Fast).unwrap();

        // Streaming decode
        let file = File::open(test_audio_path()).unwrap();
        let mut hint = Hint::new();
        hint.with_extension("wav");

        let decoder = StreamingDecoder::new(
            file,
            Some(&hint),
            Some(48000),
            ResamplerQuality::Fast,
        )
        .unwrap();

        let mut streaming_samples: Vec<f32> = Vec::new();
        for chunk_result in decoder {
            streaming_samples.extend(chunk_result.unwrap());
        }

        // Compare sample counts (should be close)
        let full_samples = full_result.data.len();
        let stream_samples = streaming_samples.len();

        assert!(
            (full_samples as i64 - stream_samples as i64).abs() < 1000,
            "Sample count mismatch: full={}, streaming={}",
            full_samples,
            stream_samples
        );

        // Compare actual samples (first 10000)
        let compare_count = full_samples.min(stream_samples).min(10000);
        let mut max_diff: f32 = 0.0;

        for i in 0..compare_count {
            let diff = (full_result.data[i] - streaming_samples[i]).abs();
            max_diff = max_diff.max(diff);
        }

        // Allow small differences due to chunked processing
        assert!(
            max_diff < 0.1,
            "Max sample difference too large: {}",
            max_diff
        );
    }
}
