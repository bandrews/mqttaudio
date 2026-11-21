// ABOUTME: First-start latency instrumentation tests (Sprint 11, D50): first-mix
// ABOUTME: publication semantics on complete and streaming buffers, and probe folding.

use mqttaudio::audio::mixer::{mix_audio, ActiveSample};
use mqttaudio::audio::streaming::{SampleBuffer, StreamingBuffer};
use mqttaudio::audio::test_support::{decoded, sine, SceneBuilder};
use mqttaudio::http::{LatencyTracker, PlayLatencyStats};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

const SR: u32 = 48000;
const BLOCK: usize = 512;

fn probe() -> Arc<AtomicU64> {
    Arc::new(AtomicU64::new(0))
}

#[test]
fn complete_buffer_publishes_first_mix_once() {
    // A fully loaded sample publishes its enqueue-to-first-mix latency on the
    // first mixed block, and exactly once: later blocks leave the value alone.
    let mut sample = ActiveSample::new(
        1,
        "v".to_string(),
        decoded(sine(440.0, SR, SR as usize, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "a.wav".to_string(),
    );
    let p = probe();
    sample.set_latency_probe(std::time::Instant::now(), p.clone());
    let mut state = SceneBuilder::new(2).sample(sample).build();

    let mut block = vec![0.0f32; BLOCK * 2];
    mix_audio(&mut block, &mut state);
    let first = p.load(Ordering::Relaxed);
    assert!(first > 0, "first mix must publish a real latency");

    mix_audio(&mut block, &mut state);
    assert_eq!(
        p.load(Ordering::Relaxed),
        first,
        "the published latency is stored exactly once"
    );
}

#[test]
fn streaming_buffer_defers_publication_until_audio_is_loaded() {
    // A still-empty streaming buffer mixes silence; that is not a start. The
    // probe fires only once the read cursor sits inside decoded audio.
    let streaming = StreamingBuffer::new(2, SR, None);
    let shared = Arc::new(RwLock::new(streaming));
    let mut sample = ActiveSample::new(
        1,
        "v".to_string(),
        SampleBuffer::Streaming(Arc::clone(&shared)),
        1.0,
        1.0,
        "cold.wav".to_string(),
    );
    let p = probe();
    sample.set_latency_probe(std::time::Instant::now(), p.clone());
    let mut state = SceneBuilder::new(2).sample(sample).build();

    let mut block = vec![0.0f32; BLOCK * 2];
    mix_audio(&mut block, &mut state);
    assert_eq!(
        p.load(Ordering::Relaxed),
        0,
        "an all-silence block from an empty streaming buffer must not publish"
    );

    // The decoder catches up well past the (silence-advanced) cursor; the next
    // mixed block is audible and publishes.
    shared
        .write()
        .unwrap()
        .append(&vec![0.25f32; BLOCK * 2 * 8]);
    mix_audio(&mut block, &mut state);
    assert!(
        p.load(Ordering::Relaxed) > 0,
        "the first audible block must publish the latency"
    );
}

#[test]
fn start_position_beyond_loaded_edge_defers_publication() {
    // A play starting deep in a still-loading buffer emits silence until decode
    // reaches the start position; the probe must not count that silence as a start.
    let mut streaming = StreamingBuffer::new(2, SR, Some(SR as usize * 10));
    streaming.append(&vec![0.25f32; 1000 * 2]); // 1000 frames loaded
    let shared = Arc::new(RwLock::new(streaming));
    let mut sample = ActiveSample::new(
        1,
        "v".to_string(),
        SampleBuffer::Streaming(Arc::clone(&shared)),
        1.0,
        1.0,
        "cold.wav".to_string(),
    );
    sample.position = 5000; // beyond the loaded edge
    let p = probe();
    sample.set_latency_probe(std::time::Instant::now(), p.clone());
    let mut state = SceneBuilder::new(2).sample(sample).build();

    let mut block = vec![0.0f32; BLOCK * 2];
    mix_audio(&mut block, &mut state);
    assert_eq!(
        p.load(Ordering::Relaxed),
        0,
        "silence ahead of the loaded edge must not publish"
    );

    // Decode catches up past the cursor; the next block publishes.
    shared.write().unwrap().append(&vec![0.25f32; 9000 * 2]);
    mix_audio(&mut block, &mut state);
    assert!(
        p.load(Ordering::Relaxed) > 0,
        "publication happens once the cursor is inside loaded audio"
    );
}

#[test]
fn tracker_folds_fired_probes_and_drops_dead_ones() {
    // The control-side tracker folds fired probes into last/max/count, keeps
    // unfired live probes pending, and GCs a probe whose sample is gone.
    let stats = Arc::new(PlayLatencyStats::default());
    let tracker = LatencyTracker::new(stats.clone());

    let fired = tracker.new_probe();
    fired.store(42_000, Ordering::Relaxed);
    let dead = tracker.new_probe();
    drop(dead); // the sample never reached its first mix and was dropped
    let pending = tracker.new_probe(); // still in flight

    tracker.fold_fired();
    assert_eq!(stats.plays_measured.load(Ordering::Relaxed), 1);
    assert_eq!(stats.last_ns.load(Ordering::Relaxed), 42_000);
    assert_eq!(stats.max_ns.load(Ordering::Relaxed), 42_000);

    // The pending probe fires later and folds on the next tick; max tracks the
    // larger value, last the most recent.
    pending.store(7_000, Ordering::Relaxed);
    tracker.fold_fired();
    assert_eq!(stats.plays_measured.load(Ordering::Relaxed), 2);
    assert_eq!(stats.last_ns.load(Ordering::Relaxed), 7_000);
    assert_eq!(stats.max_ns.load(Ordering::Relaxed), 42_000);

    // Nothing left to fold.
    tracker.fold_fired();
    assert_eq!(stats.plays_measured.load(Ordering::Relaxed), 2);
}
