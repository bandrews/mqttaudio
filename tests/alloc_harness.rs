// ABOUTME: Allocation-counting harness proving the mix path does no heap work per block.
// ABOUTME: A dedicated test binary with a counting global allocator; arms around mix_audio.

use mqttaudio::audio::mixer::{mix_audio, ActiveSample};
use mqttaudio::audio::test_support::{decoded, sine, SceneBuilder};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

thread_local! {
    // Const-initialized so accessing it never itself allocates (no lazy heap init).
    static ARMED: Cell<bool> = const { Cell::new(false) };
}
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static DEALLOCS: AtomicUsize = AtomicUsize::new(0);

/// Passthrough allocator that counts (de)allocations only while the current
/// thread is "armed". Disarmed it is a thin wrapper over the system allocator.
struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.with(|a| a.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.with(|a| a.get()) {
            DEALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Count allocations and frees made on this thread while `f` runs.
fn count_allocs<F: FnOnce()>(f: F) -> (usize, usize) {
    ALLOCS.store(0, Ordering::Relaxed);
    DEALLOCS.store(0, Ordering::Relaxed);
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    (
        ALLOCS.load(Ordering::Relaxed),
        DEALLOCS.load(Ordering::Relaxed),
    )
}

const SR: u32 = 48000;
const BLOCK: usize = 512;

#[test]
fn mix_path_is_allocation_free_after_warmup() {
    // A representative scene: a plain sample plus a looping sample, mixed to a
    // stereo bus. (The pitch path is covered separately below.)
    let plain = ActiveSample::new(
        1,
        "a".to_string(),
        decoded(sine(440.0, SR, SR as usize, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "a".to_string(),
    );
    let looping = ActiveSample::new_with_id(
        2,
        "b".to_string(),
        decoded(sine(330.0, SR, 4800, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "b".to_string(),
        None,
        true,
        0,
    );
    let mut state = SceneBuilder::new(2).sample(plain).sample(looping).build();
    let mut block = vec![0.0f32; BLOCK * 2];

    // Warm up off the armed region (first block may touch lazily-sized buffers).
    mix_audio(&mut block, &mut state);

    let (allocs, deallocs) = count_allocs(|| {
        for _ in 0..8 {
            mix_audio(&mut block, &mut state);
        }
    });

    assert_eq!(
        allocs, 0,
        "mix path allocated {allocs} times across 8 blocks"
    );
    assert_eq!(
        deallocs, 0,
        "mix path freed {deallocs} times across 8 blocks"
    );
}

#[test]
fn pitch_mix_path_is_allocation_free_after_warmup() {
    // The pitch-correction path used to heap-allocate a stretcher output buffer
    // every block (F5-4). With the pre-allocated scratch it must not allocate in
    // steady state.
    let mut pitched = ActiveSample::new(
        1,
        "p".to_string(),
        decoded(sine(220.0, SR, SR as usize, 2, 0.3), 2, SR),
        1.0,
        1.0,
        "p".to_string(),
    );
    pitched.enable_pitch_correction();
    pitched.set_speed(0.7);
    let mut state = SceneBuilder::new(2).sample(pitched).build();
    let mut block = vec![0.0f32; BLOCK * 2];

    // Warm up: sizes the pitch scratch and primes the stretcher.
    for _ in 0..4 {
        mix_audio(&mut block, &mut state);
    }

    let (allocs, deallocs) = count_allocs(|| {
        for _ in 0..8 {
            mix_audio(&mut block, &mut state);
        }
    });

    assert_eq!(
        allocs, 0,
        "pitch mix path allocated {allocs} times across 8 blocks"
    );
    assert_eq!(
        deallocs, 0,
        "pitch mix path freed {deallocs} times across 8 blocks"
    );
}
