// ABOUTME: Render-harness tests for looping playback against a streaming buffer.
// ABOUTME: A looping play must not wrap a still-growing prefix (buzz) before complete.

use mqttaudio::audio::mixer::ActiveSample;
use mqttaudio::audio::streaming::{SampleBuffer, StreamingBuffer};
use mqttaudio::audio::test_support::{
    max_inter_sample_delta, peak, render, rms, sine, SceneBuilder,
};
use std::sync::{Arc, RwLock};

const SR: u32 = 48000;
const BLOCK: usize = 512;

/// A streaming buffer that knows its eventual size but has only `loaded` frames
/// of a recognizable ramp appended so far (still `Loading`).
fn incomplete_prefix(total_estimate: usize, loaded: usize) -> Arc<RwLock<StreamingBuffer>> {
    let mut buf = StreamingBuffer::new(1, SR, Some(total_estimate));
    let ramp: Vec<f32> = (0..loaded)
        .map(|i| 0.2 + 0.6 * (i as f32 / loaded as f32))
        .collect();
    buf.append(&ramp);
    Arc::new(RwLock::new(buf))
}

#[test]
fn looping_incomplete_stream_does_not_buzz() {
    // The loaded prefix is smaller than one callback block, so a correct
    // implementation consumes it within the first block and—because the buffer
    // is still streaming—plays silence afterward instead of looping the prefix.
    let loaded = 64;
    let buf = incomplete_prefix(48_000, loaded);
    let sample = ActiveSample::new_with_id(
        1,
        "v".to_string(),
        SampleBuffer::Streaming(Arc::clone(&buf)),
        1.0,
        1.0,
        "stream".to_string(),
        None,
        true, // loop_mode
        0,    // crossfade_samples
    );
    let mut state = SceneBuilder::new(1).sample(sample).build();
    let out = render(&mut state, BLOCK, 4);

    // Everything after the first block must be silent: a buggy wrap would replay
    // the tiny prefix across every block (an audible buzz).
    let after_first_block = &out[BLOCK..];
    assert!(
        peak(after_first_block) < 1e-4,
        "looping a still-streaming buffer replayed its prefix; peak after block 1 = {}",
        peak(after_first_block)
    );
}

#[test]
fn looping_completed_stream_loops_cleanly() {
    // Once complete, looping engages against the true total. A single full sine
    // period meets itself at the wrap point, so there is no inter-sample click.
    let frames = 480; // exactly one 100 Hz period at 48 kHz
    let buf = {
        let mut b = StreamingBuffer::new(1, SR, Some(frames));
        b.append(&sine(100.0, SR, frames, 1, 0.5));
        b.mark_complete();
        Arc::new(RwLock::new(b))
    };
    let sample = ActiveSample::new_with_id(
        1,
        "v".to_string(),
        SampleBuffer::Streaming(buf),
        1.0,
        1.0,
        "stream".to_string(),
        None,
        true,
        0,
    );
    let mut state = SceneBuilder::new(1).sample(sample).build();
    let out = render(&mut state, BLOCK, 4); // 2048 frames > 480 → wraps several times

    assert!(
        rms(&out, 0, 1) > 0.1,
        "a completed looping stream should keep playing, rms = {}",
        rms(&out, 0, 1)
    );
    assert!(
        max_inter_sample_delta(&out, 0, 1) < 0.05,
        "loop wrap introduced a click, max delta = {}",
        max_inter_sample_delta(&out, 0, 1)
    );
}
