// ABOUTME: Audio file decoding using symphonia.
// ABOUTME: Supports WAV, OGG, MP3, FLAC formats.

use crate::audio::resampler;
use crate::audio::types::DecodedBuffer;
use crate::config::ResamplerQuality;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use std::fs::File;
use std::path::Path;

#[derive(Debug)]
pub enum DecodeError {
    IoError(std::io::Error),
    SymphoniaError(SymphoniaError),
    ResampleError(resampler::ResampleError),
    NoDefaultTrack,
    UnsupportedFormat,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::IoError(e) => write!(f, "I/O error: {}", e),
            DecodeError::SymphoniaError(e) => write!(f, "Decode error: {}", e),
            DecodeError::ResampleError(e) => write!(f, "Resample error: {}", e),
            DecodeError::NoDefaultTrack => write!(f, "No default audio track found"),
            DecodeError::UnsupportedFormat => write!(f, "Unsupported audio format"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<std::io::Error> for DecodeError {
    fn from(err: std::io::Error) -> Self {
        DecodeError::IoError(err)
    }
}

impl From<SymphoniaError> for DecodeError {
    fn from(err: SymphoniaError) -> Self {
        DecodeError::SymphoniaError(err)
    }
}

impl From<resampler::ResampleError> for DecodeError {
    fn from(err: resampler::ResampleError) -> Self {
        DecodeError::ResampleError(err)
    }
}

/// Decode an audio file to f32 PCM samples, optionally resampling to target rate
///
/// # Arguments
/// * `path` - Path to the audio file
/// * `target_sample_rate` - If Some, resample to this rate. If None, use file's native rate
/// * `resampler_quality` - Quality preset for resampling (Fast, Medium, High, Maximum)
pub fn decode_file(
    path: &str,
    target_sample_rate: Option<u32>,
    resampler_quality: ResamplerQuality,
) -> Result<DecodedBuffer, DecodeError> {
    tracing::debug!("Decoding file: {}", path);

    // Open the file
    let file = File::open(path)?;

    // Create a media source stream
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    // Create a hint to help the format registry guess the format
    let mut hint = Hint::new();
    if let Some(extension) = Path::new(path).extension() {
        if let Some(ext_str) = extension.to_str() {
            hint.with_extension(ext_str);
        }
    }

    // Probe the media source
    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)?;

    let mut format = probed.format;

    // Get the default track
    let track = format
        .default_track()
        .ok_or(DecodeError::NoDefaultTrack)?;

    let track_id = track.id;

    // Get codec parameters
    let codec_params = &track.codec_params;
    let channels = codec_params.channels
        .ok_or(DecodeError::UnsupportedFormat)?
        .count();
    let sample_rate = codec_params.sample_rate
        .ok_or(DecodeError::UnsupportedFormat)?;

    tracing::debug!(
        "Track info: {} channels, {} Hz",
        channels,
        sample_rate
    );

    // Create a decoder
    let decoder_opts = DecoderOptions::default();
    let mut decoder = symphonia::default::get_codecs()
        .make(codec_params, &decoder_opts)?;

    // Decode all packets
    let mut samples = Vec::new();

    loop {
        // Get the next packet
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                // End of stream
                break;
            }
            Err(e) => return Err(DecodeError::SymphoniaError(e)),
        };

        // Skip packets that don't belong to our track
        if packet.track_id() != track_id {
            continue;
        }

        // Decode the packet
        let decoded = decoder.decode(&packet)?;

        // Convert to f32 and append to samples vector
        convert_samples_to_f32(&decoded, &mut samples);
    }

    tracing::info!(
        "Decoded {} frames ({} samples) from {}",
        samples.len() / channels,
        samples.len(),
        path
    );

    // Resample if needed
    let (final_data, final_sample_rate) = if let Some(target_rate) = target_sample_rate {
        if sample_rate != target_rate {
            let resampled = resampler::resample(
                samples,
                sample_rate,
                target_rate,
                channels,
                resampler_quality,
            )?;
            (resampled, target_rate)
        } else {
            (samples, sample_rate)
        }
    } else {
        (samples, sample_rate)
    };

    Ok(DecodedBuffer::new(final_data, channels, final_sample_rate))
}

/// Convert symphonia audio buffer to interleaved f32 samples
fn convert_samples_to_f32(audio_buf: &AudioBufferRef, output: &mut Vec<f32>) {
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
            convert_typed_to_f32(buf, output, |s| (s as f64 - 2147483648.0) as f32 / 2147483648.0);
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

    // Reserve space
    output.reserve(num_frames * num_channels);

    // Interleave samples
    for frame_idx in 0..num_frames {
        for ch_idx in 0..num_channels {
            let sample = buf.chan(ch_idx)[frame_idx];
            output.push(convert(sample));
        }
    }
}
