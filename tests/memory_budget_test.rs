// ABOUTME: Integration test: the resolved memory budget hard-caps the decoded cache.
// ABOUTME: Loading more than fits evicts to stay under the cap instead of over-allocating.

use mqttaudio::cache::CacheManager;
use mqttaudio::config::{MemoryCap, ResamplerQuality};
use std::path::Path;

const WAVS: [&str; 4] = [
    "tests/audio/test_440hz_2s.wav",
    "tests/audio/music_200hz.wav",
    "tests/audio/narration_800hz.wav",
    "tests/audio/dialog_1200hz.wav",
];

fn all_present() -> bool {
    WAVS.iter().all(|f| Path::new(f).exists())
}

#[tokio::test]
async fn memory_cache_evicts_to_stay_under_the_hard_cap() {
    if !all_present() {
        eprintln!("skipping: test WAVs not found");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // A cap that fits only a couple of the decoded clips, so loading all four must
    // evict rather than over-allocate.
    let cap = 3 * 1024 * 1024; // 3 MiB
    let mut cm = CacheManager::with_resolved_cap(
        dir.path().to_path_buf(),
        ResamplerQuality::Fast,
        MemoryCap::Bytes(cap),
        Vec::new(),
        300,
    )
    .unwrap();

    for f in WAVS {
        let buf = cm.get_or_load(f, 48000).await.unwrap();
        // Release our reference so the entry becomes evictable (as a finished player
        // would), then the next load can evict it to make room.
        drop(buf);
        let resident = cm.memory_stats().size_bytes as usize;
        assert!(
            resident <= cap,
            "memory cache exceeded the hard cap after loading {f}: {resident} > {cap}"
        );
    }

    // Eviction actually happened — not all four clips stayed resident under the cap.
    assert!(
        cm.memory_stats().entry_count < WAVS.len(),
        "expected eviction under a tight cap, but all entries are still resident"
    );
}

#[tokio::test]
async fn unlimited_budget_keeps_everything_resident() {
    if !all_present() {
        eprintln!("skipping: test WAVs not found");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut cm = CacheManager::with_resolved_cap(
        dir.path().to_path_buf(),
        ResamplerQuality::Fast,
        MemoryCap::Unlimited,
        Vec::new(),
        300,
    )
    .unwrap();

    for f in WAVS {
        let _ = cm.get_or_load(f, 48000).await.unwrap();
    }
    // With no cap, every distinct clip is retained.
    assert_eq!(cm.memory_stats().entry_count, WAVS.len());
    // Headroom is unbounded.
    assert_eq!(cm.memory_headroom(), usize::MAX);
}
