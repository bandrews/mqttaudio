// ABOUTME: Background producer that decodes a local file into a bounded ring for
// ABOUTME: windowed streamed playback, off the real-time audio thread.

//! A streamed source plays a long, large, or live asset without holding it fully
//! resident: a dedicated decode thread fills a bounded SPSC ring (a fixed-duration
//! window) that the audio callback consumes wait-free. This module owns the producer
//! side — opening the decoder, pushing decoded f32 frames with back-pressure, looping
//! by re-opening, and signalling EOF/stop through shared atomics. The consumer side
//! is [`crate::audio::mixer::StreamedSource`], handed to the audio thread via
//! [`crate::rt_engine::AudioCommand::AddStreamedSource`].
//!
//! S1 streams local files. HTTP windowed streaming joins in S2, where the load
//! decision already probes the source and can reuse the same connection.

use crate::audio::streaming_decoder::{StreamingDecodeError, StreamingDecoder};
use crate::config::ResamplerQuality;
use ringbuf::{HeapConsumer, HeapProducer};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use symphonia::core::probe::Hint;

/// How long the producer parks when the ring is full before retrying, so a fast
/// decoder does not spin while the slower consumer drains the window. Short enough
/// that a `stop` is honoured within one park.
const BACKPRESSURE_PARK: Duration = Duration::from_millis(3);

/// The read end plus the shared handles the control thread needs to build a
/// [`crate::audio::mixer::StreamedSource`] and gate playback on a prebuffer.
pub struct StreamHandles {
    /// Ring consumer the audio thread reads (moved into the `StreamedSource`).
    pub consumer: HeapConsumer<f32>,
    /// Decoded channel count (interleaved), known once the header is parsed.
    pub channels: usize,
    /// Set by the producer when it reaches EOF (and will not loop) or errors. The
    /// source is finished once this is set and the ring has drained.
    pub producer_done: Arc<AtomicBool>,
    /// Set by the control side to ask the producer to stop early; the producer exits
    /// within one park and sets `producer_done`.
    pub stop_flag: Arc<AtomicBool>,
    /// Total frames pushed so far (monotonic). The control side polls this to start
    /// playback once a prebuffer has accumulated, without blocking a thread.
    pub frames_buffered: Arc<AtomicUsize>,
}

/// Open a streaming decoder over a local file, resampling to `target_sample_rate`.
/// Blocking (opens the file and parses its header).
fn open_local_decoder(
    path: &str,
    target_sample_rate: u32,
    quality: ResamplerQuality,
) -> Result<StreamingDecoder, StreamingDecodeError> {
    let file = std::fs::File::open(path)?;
    let mut hint = Hint::new();
    if let Some(ext) = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
    {
        hint.with_extension(ext);
    }
    StreamingDecoder::new(file, Some(&hint), Some(target_sample_rate), quality)
}

/// Spawn the background producer for a local file and return the handles the control
/// thread uses to build the `StreamedSource`. The ring holds `window_frames` frames
/// (its capacity is `window_frames * channels` samples), which bounds resident memory
/// independently of the file's length.
///
/// Blocking: opens the decoder (file open + header parse) to learn the channel count
/// before sizing the ring, then spawns a dedicated decode thread. Call it from a
/// blocking context (e.g. `tokio::task::spawn_blocking`) so the async runtime is not
/// stalled. A dedicated thread (not the blocking pool) is used because the producer
/// is long-lived — a multi-hour cue must not camp a pooled worker.
pub fn spawn_local_file_stream(
    path: String,
    target_sample_rate: u32,
    quality: ResamplerQuality,
    window_frames: usize,
    loop_mode: bool,
) -> Result<StreamHandles, StreamingDecodeError> {
    let decoder = open_local_decoder(&path, target_sample_rate, quality)?;
    let channels = decoder.channels().max(1);
    let capacity = window_frames.max(1) * channels;
    let (producer, consumer) = crate::audio::input::create_ring_buffer(capacity);

    let stop_flag = Arc::new(AtomicBool::new(false));
    let producer_done = Arc::new(AtomicBool::new(false));
    let frames_buffered = Arc::new(AtomicUsize::new(0));

    let thread_stop = Arc::clone(&stop_flag);
    let thread_done = Arc::clone(&producer_done);
    let thread_buffered = Arc::clone(&frames_buffered);
    std::thread::Builder::new()
        .name("streamed-decode".to_string())
        .spawn(move || {
            run_producer(
                decoder,
                path,
                target_sample_rate,
                quality,
                channels,
                loop_mode,
                producer,
                thread_stop,
                thread_done,
                thread_buffered,
            );
        })
        .map_err(StreamingDecodeError::Io)?;

    Ok(StreamHandles {
        consumer,
        channels,
        producer_done,
        stop_flag,
        frames_buffered,
    })
}

/// The producer loop. Pushes decoded f32 frames into `producer`, parking briefly when
/// the ring is full, honouring `stop_flag` between pushes, tracking total frames in
/// `frames_buffered`, and on EOF either re-opening for `loop_mode` or setting
/// `producer_done`. `initial` is the already-open decoder used for the first pass; a
/// loop re-opens `path` for each subsequent pass.
#[allow(clippy::too_many_arguments)]
fn run_producer(
    initial: StreamingDecoder,
    path: String,
    target_sample_rate: u32,
    quality: ResamplerQuality,
    channels: usize,
    loop_mode: bool,
    mut producer: HeapProducer<f32>,
    stop_flag: Arc<AtomicBool>,
    producer_done: Arc<AtomicBool>,
    frames_buffered: Arc<AtomicUsize>,
) {
    let mut next = Some(initial);
    'outer: loop {
        let decoder = match next.take() {
            Some(d) => d,
            None => match open_local_decoder(&path, target_sample_rate, quality) {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!("Streamed source re-open failed for {}: {}", path, e);
                    break;
                }
            },
        };

        for chunk in decoder {
            if stop_flag.load(Ordering::Acquire) {
                break 'outer;
            }
            let samples = match chunk {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Streamed source decode error for {}: {}", path, e);
                    break 'outer;
                }
            };

            let mut pushed = 0;
            while pushed < samples.len() {
                if stop_flag.load(Ordering::Acquire) {
                    break 'outer;
                }
                let n = producer.push_slice(&samples[pushed..]);
                if n == 0 {
                    // Ring full: the consumer has a full window. Park briefly rather
                    // than spin, then retry (and re-check stop).
                    std::thread::sleep(BACKPRESSURE_PARK);
                    continue;
                }
                pushed += n;
                // The ring capacity and every pushed chunk are multiples of the
                // channel count, so `n` is too and this is an exact frame count.
                frames_buffered.fetch_add(n / channels, Ordering::Release);
            }
        }

        if loop_mode && !stop_flag.load(Ordering::Acquire) {
            // Seamless loop: re-open from the start and keep feeding the ring.
            continue;
        }
        break;
    }

    producer_done.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_WAV: &str = "tests/audio/test_440hz_2s.wav";

    fn test_wav_available() -> bool {
        std::path::Path::new(TEST_WAV).exists()
    }

    /// Drain everything currently queued into a scratch sink, returning frames popped.
    fn drain_frames(handles: &mut StreamHandles, scratch: &mut [f32]) -> usize {
        let n = handles.consumer.pop_slice(scratch);
        n / handles.channels.max(1)
    }

    #[test]
    fn producer_buffers_frames_and_completes_at_eof() {
        if !test_wav_available() {
            eprintln!("skipping: {TEST_WAV} not found");
            return;
        }
        let mut handles = spawn_local_file_stream(
            TEST_WAV.to_string(),
            48000,
            ResamplerQuality::Fast,
            4800, // 0.1 s window
            false,
        )
        .unwrap();
        assert_eq!(handles.channels, 2);

        let mut scratch = vec![0.0f32; 8192];
        let mut total_frames = 0usize;
        for _ in 0..100_000 {
            total_frames += drain_frames(&mut handles, &mut scratch);
            if handles.producer_done.load(Ordering::Acquire) && handles.consumer.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }

        assert!(
            handles.producer_done.load(Ordering::Acquire),
            "producer must signal completion at EOF"
        );
        // 2 s resampled 44.1k -> 48k is ~96000 frames; allow resampler-latency slack.
        assert!(
            (90_000..=100_000).contains(&total_frames),
            "expected ~96000 frames, got {total_frames}"
        );
    }

    #[test]
    fn producer_stops_promptly_on_stop_flag() {
        if !test_wav_available() {
            eprintln!("skipping: {TEST_WAV} not found");
            return;
        }
        // Tiny window and no draining: the producer fills the ring then parks on
        // back-pressure, so it is mid-stream when we ask it to stop.
        let handles = spawn_local_file_stream(
            TEST_WAV.to_string(),
            48000,
            ResamplerQuality::Fast,
            480,
            false,
        )
        .unwrap();

        let mut buffered_something = false;
        for _ in 0..2000 {
            if handles.frames_buffered.load(Ordering::Acquire) > 0 {
                buffered_something = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            buffered_something,
            "producer should buffer the first frames"
        );

        handles.stop_flag.store(true, Ordering::Release);

        let mut stopped = false;
        for _ in 0..2000 {
            if handles.producer_done.load(Ordering::Acquire) {
                stopped = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(stopped, "producer must stop promptly once stop_flag is set");
        assert!(
            handles.frames_buffered.load(Ordering::Acquire) < 90_000,
            "a stopped producer must not have delivered the whole file"
        );
    }

    #[test]
    fn producer_loops_by_reopening_at_eof() {
        if !test_wav_available() {
            eprintln!("skipping: {TEST_WAV} not found");
            return;
        }
        let mut handles = spawn_local_file_stream(
            TEST_WAV.to_string(),
            48000,
            ResamplerQuality::Fast,
            4800,
            true, // loop
        )
        .unwrap();

        // A looping producer must deliver MORE than one file length (it re-opens at
        // EOF and keeps going), so drain past one ~96000-frame pass.
        let target = 150_000usize;
        let mut scratch = vec![0.0f32; 8192];
        let mut total = 0usize;
        for _ in 0..200_000 {
            total += drain_frames(&mut handles, &mut scratch);
            if total >= target {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        handles.stop_flag.store(true, Ordering::Release);

        assert!(
            total >= target,
            "a looping producer must keep producing past one file length, got {total}"
        );
    }

    /// Performance: windowed playback starts after a small fixed prebuffer, not after
    /// decoding the whole file — so time-to-first-sample does not grow with length
    /// (the reason a multi-hour cue is playable as fast as a short one). With a small
    /// window and no consumer draining, the producer parks on back-pressure once the
    /// ring is full, far from EOF, proving the prebuffer gate fires before full decode.
    #[test]
    fn windowed_time_to_first_sample_is_low_and_precedes_full_decode() {
        if !test_wav_available() {
            eprintln!("skipping: {TEST_WAV} not found");
            return;
        }
        let window_frames = 4800; // 0.1 s ring
        let prebuffer_frames = 2400; // 0.05 s — the gate the control side waits for

        let start = std::time::Instant::now();
        let handles = spawn_local_file_stream(
            TEST_WAV.to_string(),
            48000,
            ResamplerQuality::Fast,
            window_frames,
            false,
        )
        .unwrap();

        // Wait (bounded) for the prebuffer to fill, recording how long it took.
        let mut ttfs = None;
        for _ in 0..5000 {
            if handles.frames_buffered.load(Ordering::Acquire) >= prebuffer_frames {
                ttfs = Some(start.elapsed());
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let ttfs = ttfs.expect("prebuffer must fill");

        // The producer is parked on back-pressure (ring full at window_frames, nothing
        // drained), nowhere near the ~96000-frame EOF: playback starts on the prebuffer,
        // not on full decode. This is what makes TTFS independent of file length.
        assert!(
            !handles.producer_done.load(Ordering::Acquire),
            "prebuffer-ready must precede full decode (windowed start, not full decode)"
        );

        // Low in absolute terms — a generous bound (real value is single-digit ms) that
        // still flags a regression that waits on a long decode before the first sample.
        assert!(
            ttfs < Duration::from_millis(500),
            "windowed time-to-first-sample too high: {ttfs:?}"
        );
        eprintln!("windowed TTFS (spawn -> {prebuffer_frames}-frame prebuffer): {ttfs:?}");

        handles.stop_flag.store(true, Ordering::Release);
    }
}
