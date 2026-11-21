// ABOUTME: Memory cache for decoded audio buffers.
// ABOUTME: Provides fast access to frequently used samples with LRU eviction.

use crate::audio::types::DecodedBuffer;
use crate::config::MemoryCap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

/// Returned by [`MemoryCache::try_put`] when a buffer cannot be cached under the hard
/// cap (only playing-protected entries remain, or it is larger than the whole cap).
#[derive(Debug, PartialEq, Eq)]
pub struct CacheFull;

/// Entry in the memory cache with access tracking for LRU eviction
struct MemoryCacheEntry {
    buffer: Arc<DecodedBuffer>,
    last_access: Instant,
    size_bytes: usize,
}

/// Memory cache for decoded audio buffers
/// Stores fully decoded PCM data ready for immediate playback
/// Supports optional LRU eviction when a size limit is configured
pub struct MemoryCache {
    /// Map of URL/path -> cache entry
    entries: HashMap<String, MemoryCacheEntry>,
    /// Maximum cache size in bytes (None = unlimited)
    max_size_bytes: Option<usize>,
    /// Current total size of cached data in bytes
    current_size_bytes: usize,
}

impl MemoryCache {
    /// Create a new empty memory cache with no size limit
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            max_size_bytes: None,
            current_size_bytes: 0,
        }
    }

    /// Create a new memory cache with a maximum size limit in bytes.
    /// If max_size is 0, the cache has no limit (unlimited).
    #[allow(dead_code)]
    pub fn with_max_size(max_size: usize) -> Self {
        Self {
            entries: HashMap::new(),
            max_size_bytes: if max_size == 0 { None } else { Some(max_size) },
            current_size_bytes: 0,
        }
    }

    /// Create a memory cache with a resolved [`MemoryCap`]. `Bytes(n)` is a hard cap
    /// of `n` bytes; `Unlimited` is no cap. (Distinct from `with_max_size`, where `0`
    /// means unlimited — here `Bytes(0)` would cache nothing.)
    pub fn with_cap(cap: MemoryCap) -> Self {
        let max_size_bytes = match cap {
            MemoryCap::Unlimited => None,
            MemoryCap::Bytes(n) => Some(n),
        };
        Self {
            entries: HashMap::new(),
            max_size_bytes,
            current_size_bytes: 0,
        }
    }

    /// Get the maximum cache size in bytes, or None if unlimited
    #[allow(dead_code)]
    pub fn max_size_bytes(&self) -> Option<usize> {
        self.max_size_bytes
    }

    /// Get the current total size of cached data in bytes
    #[allow(dead_code)]
    pub fn current_size_bytes(&self) -> usize {
        self.current_size_bytes
    }

    /// Calculate the size of a buffer in bytes
    fn buffer_size(buffer: &DecodedBuffer) -> usize {
        buffer.data.len() * std::mem::size_of::<f32>()
    }

    /// Get a cached buffer for the given URL/path.
    /// Updates the last access time for LRU tracking.
    pub fn get(&mut self, key: &str) -> Option<Arc<DecodedBuffer>> {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_access = Instant::now();
            Some(Arc::clone(&entry.buffer))
        } else {
            None
        }
    }

    /// Sum of the bytes held by entries that cannot be evicted (still referenced /
    /// playing, `strong_count > 1`), excluding the entry at `except` (which is about
    /// to be replaced). These bytes must remain resident, so they reduce the room
    /// available for a new buffer.
    fn protected_bytes_excluding(&self, except: &str) -> usize {
        self.entries
            .iter()
            .filter(|(k, e)| k.as_str() != except && Arc::strong_count(&e.buffer) > 1)
            .map(|(_, e)| e.size_bytes)
            .sum()
    }

    /// Bytes of new data the cache can accept right now without exceeding the cap,
    /// after evicting everything evictable. `usize::MAX` when unlimited. The
    /// load-strategy decision uses this to force an over-budget asset to windowed
    /// streaming instead of a full load.
    pub fn fit_headroom(&self) -> usize {
        match self.max_size_bytes {
            None => usize::MAX,
            Some(max) => max.saturating_sub(self.protected_bytes_excluding("")),
        }
    }

    /// Admit a buffer under the hard cap. Evicts evictable (not currently playing) LRU
    /// entries to make room; if it still would not fit — only playing-protected
    /// entries remain, or it is larger than the whole cap — it caches nothing and
    /// returns [`CacheFull`] rather than over-allocating. Playing buffers are never
    /// evicted (their accounting must stay live).
    pub fn try_put(&mut self, key: String, buffer: Arc<DecodedBuffer>) -> Result<(), CacheFull> {
        let size = Self::buffer_size(&buffer);

        if let Some(max) = self.max_size_bytes {
            // Cannot fit if larger than the whole cap, or larger than the room left
            // once every evictable entry (all but the protected ones, excluding any
            // entry we are replacing) is gone.
            let protected = self.protected_bytes_excluding(&key);
            if size > max || size > max.saturating_sub(protected) {
                return Err(CacheFull);
            }
        }

        // Replacing an existing entry frees its bytes first.
        if let Some(old_entry) = self.entries.remove(&key) {
            self.current_size_bytes -= old_entry.size_bytes;
        }

        // Evict evictable LRU entries until it fits. The admission check above
        // guarantees there is room once the evictable entries are gone.
        if let Some(max_size) = self.max_size_bytes {
            while self.current_size_bytes + size > max_size {
                if !self.evict_one() {
                    break;
                }
            }
        }

        tracing::debug!("Caching decoded buffer for: {} ({} bytes)", key, size);
        let entry = MemoryCacheEntry {
            buffer,
            last_access: Instant::now(),
            size_bytes: size,
        };
        self.entries.insert(key, entry);
        self.current_size_bytes += size;
        Ok(())
    }

    /// Store a decoded buffer, evicting LRU entries to stay within the cap. If it
    /// cannot fit (only playing entries remain), it is dropped rather than cached —
    /// the asset still plays from the caller's own reference, it just is not retained.
    pub fn put(&mut self, key: String, buffer: Arc<DecodedBuffer>) {
        let size = Self::buffer_size(&buffer);
        if self.try_put(key.clone(), buffer).is_err() {
            tracing::debug!(
                "Memory cache full; not caching {} ({} bytes) — it still plays from the \
                 caller's reference, it just is not retained",
                key,
                size
            );
        }
    }

    /// Evict the least recently used entry that no one is still playing.
    /// A buffer is "in use" when something outside the cache holds an `Arc` to it
    /// (`strong_count > 1`); those are skipped so a playing sample is never evicted
    /// out from under its accounting. Returns true if an entry was evicted.
    fn evict_one(&mut self) -> bool {
        let lru_key = self
            .entries
            .iter()
            .filter(|(_, entry)| Arc::strong_count(&entry.buffer) == 1)
            .min_by_key(|(_, entry)| entry.last_access)
            .map(|(key, _)| key.clone());

        if let Some(key) = lru_key {
            if let Some(entry) = self.entries.remove(&key) {
                self.current_size_bytes -= entry.size_bytes;
                tracing::debug!("Evicted LRU entry: {} ({} bytes)", key, entry.size_bytes);
                return true;
            }
        }
        false
    }

    /// Remove a specific entry from the cache
    pub fn remove(&mut self, key: &str) -> bool {
        if let Some(entry) = self.entries.remove(key) {
            self.current_size_bytes -= entry.size_bytes;
            true
        } else {
            false
        }
    }

    /// Clear all cached buffers
    pub fn clear(&mut self) {
        let count = self.entries.len();
        self.entries.clear();
        self.current_size_bytes = 0;
        tracing::info!("Cleared {} decoded buffers from memory cache", count);
    }

    /// Check if a key is resident in the cache, without touching LRU order.
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Get the number of cached items
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the cache is empty
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get memory usage estimate in bytes
    /// (same as current_size_bytes, kept for API compatibility)
    pub fn memory_usage_bytes(&self) -> usize {
        self.current_size_bytes
    }
}

impl Default for MemoryCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::types::DecodedBuffer;

    #[test]
    fn test_put_replacing_existing_key_keeps_size_consistent() {
        // Replacing a key must not let the eviction loop double-subtract the
        // old entry's size (which would wrap current_size_bytes in release)
        let mut cache = MemoryCache::with_max_size(16_000);

        // Replace a resident entry while another evictable entry consumes headroom.
        let big = Arc::new(create_test_buffer(2, 1500));
        cache.put("a".to_string(), big.clone());
        let size_after_first = cache.current_size_bytes();

        cache.put("b".to_string(), Arc::new(create_test_buffer(1, 500)));
        cache.put("a".to_string(), big.clone());
        cache.remove("b");
        assert_eq!(
            cache.current_size_bytes(),
            size_after_first,
            "replacing a key with an identical buffer must not change the accounted size"
        );
        assert_eq!(cache.len(), 1);

        // And the cache must still behave sanely afterwards
        cache.put("b".to_string(), Arc::new(create_test_buffer(1, 100)));
        assert!(cache.current_size_bytes() < 20_000);
    }

    fn create_test_buffer(channels: usize, frames: usize) -> DecodedBuffer {
        let data = vec![0.0f32; channels * frames];
        DecodedBuffer {
            data,
            channels,
            sample_rate: 48000,
            frames,
        }
    }

    #[test]
    fn test_memory_cache_new() {
        let cache = MemoryCache::new();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_put_and_get() {
        let mut cache = MemoryCache::new();
        let buffer = Arc::new(create_test_buffer(2, 1000));

        let url = "http://example.com/test.wav";
        cache.put(url.to_string(), buffer.clone());

        assert_eq!(cache.len(), 1);
        assert!(cache.contains(url));

        let retrieved = cache.get(url).unwrap();
        assert_eq!(retrieved.channels, 2);
        assert_eq!(retrieved.frames, 1000);
        assert_eq!(retrieved.sample_rate, 48000);

        // Arc should point to the same data
        assert!(Arc::ptr_eq(&buffer, &retrieved));
    }

    #[test]
    fn test_get_nonexistent() {
        let mut cache = MemoryCache::new();
        assert!(cache.get("nonexistent").is_none());
    }

    #[test]
    fn test_remove() {
        let mut cache = MemoryCache::new();
        let buffer = Arc::new(create_test_buffer(2, 500));

        let url = "http://example.com/remove.wav";
        cache.put(url.to_string(), buffer);

        assert!(cache.contains(url));
        assert!(cache.remove(url));
        assert!(!cache.contains(url));
        assert_eq!(cache.len(), 0);

        // Removing again should return false
        assert!(!cache.remove(url));
    }

    #[test]
    fn test_clear() {
        let mut cache = MemoryCache::new();

        // Add multiple entries
        for i in 0..5 {
            let url = format!("http://example.com/test{}.wav", i);
            let buffer = Arc::new(create_test_buffer(2, 1000));
            cache.put(url, buffer);
        }

        assert_eq!(cache.len(), 5);

        cache.clear();

        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_overwrite_entry() {
        let mut cache = MemoryCache::new();
        let url = "http://example.com/test.wav";

        // Add first buffer
        let buffer1 = Arc::new(create_test_buffer(2, 1000));
        cache.put(url.to_string(), buffer1.clone());

        // Overwrite with second buffer
        let buffer2 = Arc::new(create_test_buffer(2, 2000));
        cache.put(url.to_string(), buffer2.clone());

        // Should have the second buffer
        assert_eq!(cache.len(), 1);
        let retrieved = cache.get(url).unwrap();
        assert_eq!(retrieved.frames, 2000);
        assert!(Arc::ptr_eq(&buffer2, &retrieved));
        assert!(!Arc::ptr_eq(&buffer1, &retrieved));
    }

    #[test]
    fn test_memory_usage() {
        let mut cache = MemoryCache::new();

        // Add a stereo buffer with 48000 frames (1 second at 48kHz)
        let buffer = Arc::new(create_test_buffer(2, 48000));
        cache.put("test.wav".to_string(), buffer);

        // Calculate expected size
        // 2 channels * 48000 frames * 4 bytes per f32 = 384000 bytes
        let expected_bytes = 2 * 48000 * 4;
        assert_eq!(cache.memory_usage_bytes(), expected_bytes);
    }

    #[test]
    fn test_multiple_entries_memory_usage() {
        let mut cache = MemoryCache::new();

        // Add 3 buffers
        for i in 0..3 {
            let buffer = Arc::new(create_test_buffer(2, 10000));
            cache.put(format!("test{}.wav", i), buffer);
        }

        // 3 buffers * 2 channels * 10000 frames * 4 bytes = 240000 bytes
        let expected_bytes = 3 * 2 * 10000 * 4;
        assert_eq!(cache.memory_usage_bytes(), expected_bytes);
    }

    #[test]
    fn test_arc_ref_counting() {
        let mut cache = MemoryCache::new();
        let url = "http://example.com/ref.wav";

        let buffer = Arc::new(create_test_buffer(2, 100));
        let weak_before = Arc::downgrade(&buffer);

        // Strong count should be 1
        assert_eq!(Arc::strong_count(&buffer), 1);

        cache.put(url.to_string(), buffer.clone());

        // Strong count should be 2 (our reference + cache)
        assert_eq!(Arc::strong_count(&buffer), 2);

        let retrieved = cache.get(url).unwrap();

        // Strong count should be 3 (our ref + cache + retrieved)
        assert_eq!(Arc::strong_count(&buffer), 3);

        drop(retrieved);

        // Strong count back to 2
        assert_eq!(Arc::strong_count(&buffer), 2);

        cache.remove(url);

        // Strong count back to 1
        assert_eq!(Arc::strong_count(&buffer), 1);

        // Weak reference should still be valid
        assert!(weak_before.upgrade().is_some());

        drop(buffer);

        // Now weak reference should be invalid
        assert!(weak_before.upgrade().is_none());
    }

    #[test]
    fn test_memory_cache_with_limit() {
        // Create cache with 1MB limit
        let cache = MemoryCache::with_max_size(1024 * 1024);
        assert_eq!(cache.max_size_bytes(), Some(1024 * 1024));
        assert_eq!(cache.current_size_bytes(), 0);
    }

    #[test]
    fn test_memory_cache_unlimited() {
        // Create cache with no limit (0 means unlimited)
        let cache = MemoryCache::with_max_size(0);
        assert_eq!(cache.max_size_bytes(), None);
    }

    #[test]
    fn test_lru_eviction_basic() {
        // Create cache with limit that fits exactly 2 buffers
        // Each buffer: 2 channels * 10000 frames * 4 bytes = 80000 bytes
        let buffer_size = 2 * 10000 * 4;
        let mut cache = MemoryCache::with_max_size(buffer_size * 2);

        // Add first buffer
        let buffer1 = Arc::new(create_test_buffer(2, 10000));
        cache.put("first.wav".to_string(), buffer1);
        assert!(cache.contains("first.wav"));

        // Add second buffer - should fit
        let buffer2 = Arc::new(create_test_buffer(2, 10000));
        cache.put("second.wav".to_string(), buffer2);
        assert!(cache.contains("first.wav"));
        assert!(cache.contains("second.wav"));

        // Add third buffer - should evict first (LRU)
        let buffer3 = Arc::new(create_test_buffer(2, 10000));
        cache.put("third.wav".to_string(), buffer3);
        assert!(!cache.contains("first.wav")); // evicted
        assert!(cache.contains("second.wav"));
        assert!(cache.contains("third.wav"));
    }

    #[test]
    fn test_lru_access_updates_order() {
        let buffer_size = 2 * 10000 * 4;
        let mut cache = MemoryCache::with_max_size(buffer_size * 2);

        // Add two buffers
        let buffer1 = Arc::new(create_test_buffer(2, 10000));
        cache.put("first.wav".to_string(), buffer1);

        let buffer2 = Arc::new(create_test_buffer(2, 10000));
        cache.put("second.wav".to_string(), buffer2);

        // Access first buffer - makes it more recently used
        let _ = cache.get("first.wav");

        // Add third buffer - should evict second (now LRU)
        let buffer3 = Arc::new(create_test_buffer(2, 10000));
        cache.put("third.wav".to_string(), buffer3);
        assert!(cache.contains("first.wav")); // kept (was accessed)
        assert!(!cache.contains("second.wav")); // evicted (LRU)
        assert!(cache.contains("third.wav"));
    }

    #[test]
    fn externally_referenced_buffer_is_not_evicted() {
        let buffer_size = 2 * 10000 * 4;
        let mut cache = MemoryCache::with_max_size(buffer_size * 2);

        // A "playing" buffer: we keep an Arc to it, exactly as an ActiveSample does.
        let playing = Arc::new(create_test_buffer(2, 10000));
        cache.put("playing.wav".to_string(), playing.clone());

        // An unreferenced buffer (the temporary Arc is dropped after put).
        cache.put(
            "other.wav".to_string(),
            Arc::new(create_test_buffer(2, 10000)),
        );
        // Make `other` the most-recently-used (and drop the returned Arc immediately,
        // so it stays unreferenced and therefore evictable).
        let _ = cache.get("other.wav");

        // Adding a third buffer forces an eviction. The still-referenced buffer must
        // survive even though it is the least-recently-used.
        cache.put(
            "third.wav".to_string(),
            Arc::new(create_test_buffer(2, 10000)),
        );

        assert!(
            cache.contains("playing.wav"),
            "a buffer still referenced by a player must not be evicted"
        );
        assert!(
            !cache.contains("other.wav"),
            "the unreferenced LRU entry should be evicted instead"
        );
        // Accounting reflects exactly the two resident buffers.
        assert_eq!(cache.memory_usage_bytes(), buffer_size * 2);
    }

    #[test]
    fn test_playing_entries_never_evicted() {
        let buffer_size = 2 * 10000 * 4;
        let mut cache = MemoryCache::with_max_size(buffer_size * 2);

        // Keep an external Arc to "first", so it counts as playing (strong_count > 1).
        let first = Arc::new(create_test_buffer(2, 10000));
        cache.put("first.wav".to_string(), first.clone());

        cache.put(
            "second.wav".to_string(),
            Arc::new(create_test_buffer(2, 10000)),
        );

        // Add third buffer - should evict second (first is protected by its live Arc)
        cache.put(
            "third.wav".to_string(),
            Arc::new(create_test_buffer(2, 10000)),
        );
        assert!(cache.contains("first.wav")); // protected (still referenced)
        assert!(!cache.contains("second.wav")); // evicted
        assert!(cache.contains("third.wav"));
    }

    #[test]
    fn test_buffer_evictable_after_external_ref_dropped() {
        let buffer_size = 2 * 10000 * 4;
        let mut cache = MemoryCache::with_max_size(buffer_size * 2);

        // While "first" is referenced it is protected from eviction.
        let first = Arc::new(create_test_buffer(2, 10000));
        cache.put("first.wav".to_string(), first.clone());
        cache.put(
            "second.wav".to_string(),
            Arc::new(create_test_buffer(2, 10000)),
        );
        cache.put(
            "third.wav".to_string(),
            Arc::new(create_test_buffer(2, 10000)),
        );
        assert!(cache.contains("first.wav"), "protected while referenced");

        // Drop the external reference; "first" is now just another cache entry.
        drop(first);
        // Touch the survivor so "first" is the LRU, then force another eviction.
        let _ = cache.get("third.wav");
        cache.put(
            "fourth.wav".to_string(),
            Arc::new(create_test_buffer(2, 10000)),
        );
        assert!(
            !cache.contains("first.wav"),
            "evictable once no longer referenced"
        );
    }

    #[test]
    fn test_eviction_multiple_candidates() {
        let buffer_size = 2 * 10000 * 4;
        // Limit that fits 3 buffers
        let mut cache = MemoryCache::with_max_size(buffer_size * 3);

        cache.put(
            "first.wav".to_string(),
            Arc::new(create_test_buffer(2, 10000)),
        );
        cache.put(
            "second.wav".to_string(),
            Arc::new(create_test_buffer(2, 10000)),
        );
        // Keep an external Arc to "third", so it is protected (playing).
        let third = Arc::new(create_test_buffer(2, 10000));
        cache.put("third.wav".to_string(), third.clone());

        // Access second
        let _ = cache.get("second.wav");

        // Add fourth buffer - should evict first (LRU and not protected)
        cache.put(
            "fourth.wav".to_string(),
            Arc::new(create_test_buffer(2, 10000)),
        );
        assert!(!cache.contains("first.wav")); // evicted (oldest, not protected)
        assert!(cache.contains("second.wav")); // kept (accessed more recently)
        assert!(cache.contains("third.wav")); // kept (referenced/playing)
        assert!(cache.contains("fourth.wav")); // just added
    }

    #[test]
    fn test_current_size_tracking() {
        let mut cache = MemoryCache::with_max_size(1024 * 1024);

        // Add a buffer: 2 channels * 1000 frames * 4 bytes = 8000 bytes
        let buffer = Arc::new(create_test_buffer(2, 1000));
        cache.put("test.wav".to_string(), buffer);
        assert_eq!(cache.current_size_bytes(), 8000);

        // Add another
        let buffer2 = Arc::new(create_test_buffer(2, 2000));
        cache.put("test2.wav".to_string(), buffer2);
        assert_eq!(cache.current_size_bytes(), 8000 + 16000);

        // Remove first
        cache.remove("test.wav");
        assert_eq!(cache.current_size_bytes(), 16000);
    }

    #[test]
    fn try_put_rejects_when_only_protected_entries_remain() {
        // Cap fits exactly one buffer. A playing (protected) buffer occupies it; a new
        // buffer cannot evict it, so try_put refuses and caches nothing (no over-alloc).
        let buffer_size = 2 * 10000 * 4; // 80000 bytes
        let mut cache = MemoryCache::with_cap(MemoryCap::Bytes(buffer_size));
        let playing = Arc::new(create_test_buffer(2, 10000));
        cache.put("playing.wav".to_string(), playing.clone());

        let result = cache.try_put(
            "new.wav".to_string(),
            Arc::new(create_test_buffer(2, 10000)),
        );
        assert_eq!(result, Err(CacheFull));
        assert!(!cache.contains("new.wav"));
        assert!(cache.contains("playing.wav"));
        assert_eq!(
            cache.current_size_bytes(),
            buffer_size,
            "cap must not be exceeded"
        );
    }

    #[test]
    fn try_put_evicts_an_unreferenced_entry_to_admit() {
        let buffer_size = 2 * 10000 * 4;
        let mut cache = MemoryCache::with_cap(MemoryCap::Bytes(buffer_size));
        // An unreferenced entry (the temporary Arc is dropped after put).
        cache.put(
            "old.wav".to_string(),
            Arc::new(create_test_buffer(2, 10000)),
        );
        // try_put admits by evicting the unreferenced LRU entry.
        assert!(cache
            .try_put(
                "new.wav".to_string(),
                Arc::new(create_test_buffer(2, 10000))
            )
            .is_ok());
        assert!(cache.contains("new.wav"));
        assert!(!cache.contains("old.wav"));
        assert_eq!(cache.current_size_bytes(), buffer_size);
    }

    #[test]
    fn try_put_rejects_a_buffer_larger_than_the_whole_cap() {
        let mut cache = MemoryCache::with_cap(MemoryCap::Bytes(1000));
        let big = Arc::new(create_test_buffer(2, 10000)); // 80000 bytes > cap
        assert_eq!(cache.try_put("big.wav".to_string(), big), Err(CacheFull));
        assert_eq!(cache.current_size_bytes(), 0);
    }

    #[test]
    fn fit_headroom_subtracts_protected_bytes() {
        let buffer_size = 2 * 10000 * 4;
        let mut cache = MemoryCache::with_cap(MemoryCap::Bytes(buffer_size * 4));
        // Two playing (protected) buffers occupy 2x; headroom is the remaining 2x.
        let a = Arc::new(create_test_buffer(2, 10000));
        let b = Arc::new(create_test_buffer(2, 10000));
        cache.put("a.wav".to_string(), a.clone());
        cache.put("b.wav".to_string(), b.clone());
        assert_eq!(cache.fit_headroom(), buffer_size * 2);
    }

    #[test]
    fn unlimited_cache_has_unbounded_headroom() {
        let cache = MemoryCache::with_cap(MemoryCap::Unlimited);
        assert_eq!(cache.fit_headroom(), usize::MAX);
    }

    #[test]
    fn put_drops_silently_when_cache_full_instead_of_over_allocating() {
        let buffer_size = 2 * 10000 * 4;
        let mut cache = MemoryCache::with_cap(MemoryCap::Bytes(buffer_size));
        let playing = Arc::new(create_test_buffer(2, 10000));
        cache.put("playing.wav".to_string(), playing.clone());
        cache.put(
            "overflow.wav".to_string(),
            Arc::new(create_test_buffer(2, 10000)),
        );
        assert!(
            !cache.contains("overflow.wav"),
            "put must not over-allocate"
        );
        assert_eq!(cache.current_size_bytes(), buffer_size, "cap not exceeded");
    }
}
