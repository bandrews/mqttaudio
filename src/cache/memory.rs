// ABOUTME: Memory cache for decoded audio buffers.
// ABOUTME: Provides fast access to frequently used samples with LRU eviction.

use crate::audio::types::DecodedBuffer;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

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
    /// Keys of buffers currently being played (protected from eviction)
    playing: HashSet<String>,
}

impl MemoryCache {
    /// Create a new empty memory cache with no size limit
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            max_size_bytes: None,
            current_size_bytes: 0,
            playing: HashSet::new(),
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
            playing: HashSet::new(),
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

    /// Store a decoded buffer in the cache.
    /// Evicts LRU entries if necessary to stay within the size limit.
    pub fn put(&mut self, key: String, buffer: Arc<DecodedBuffer>) {
        let size = Self::buffer_size(&buffer);

        // If we're replacing an existing entry, take it out entirely first so
        // the eviction loop below can neither pick it nor double-subtract it
        if let Some(old_entry) = self.entries.remove(&key) {
            self.current_size_bytes -= old_entry.size_bytes;
        }

        // Evict entries if we would exceed the limit
        if let Some(max_size) = self.max_size_bytes {
            while self.current_size_bytes + size > max_size {
                if !self.evict_one() {
                    // Could not evict anything (all entries are protected)
                    // Log a warning but proceed anyway
                    tracing::warn!(
                        "Cache size limit exceeded but all entries are protected from eviction"
                    );
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
    }

    /// Evict the least recently used entry that is not currently playing.
    /// Returns true if an entry was evicted, false if no evictable entries.
    fn evict_one(&mut self) -> bool {
        // Find the LRU entry that is not playing
        let lru_key = self.entries
            .iter()
            .filter(|(key, _)| !self.playing.contains(*key))
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
            self.playing.remove(key);
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
        self.playing.clear();
        tracing::info!("Cleared {} decoded buffers from memory cache", count);
    }

    /// Check if a key is in the cache
    #[cfg(test)]
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

    /// Mark a buffer as currently playing (protected from eviction)
    #[allow(dead_code)]
    pub fn mark_playing(&mut self, key: &str) {
        if self.entries.contains_key(key) {
            self.playing.insert(key.to_string());
        }
    }

    /// Mark a buffer as no longer playing (can be evicted)
    #[allow(dead_code)]
    pub fn mark_not_playing(&mut self, key: &str) {
        self.playing.remove(key);
    }

    /// Check if a buffer is currently marked as playing
    #[allow(dead_code)]
    pub fn is_playing(&self, key: &str) -> bool {
        self.playing.contains(key)
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
        let mut cache = MemoryCache::with_max_size(10_000);

        // 12000 bytes: larger than the limit, so every put runs the eviction
        // loop, and on replacement the old same-key entry is the only
        // eviction candidate
        let big = Arc::new(create_test_buffer(2, 1500));
        cache.put("a".to_string(), big.clone());
        let size_after_first = cache.current_size_bytes();

        cache.put("a".to_string(), big.clone());
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
    fn test_playing_entries_never_evicted() {
        let buffer_size = 2 * 10000 * 4;
        let mut cache = MemoryCache::with_max_size(buffer_size * 2);

        // Add two buffers
        let buffer1 = Arc::new(create_test_buffer(2, 10000));
        cache.put("first.wav".to_string(), buffer1);

        let buffer2 = Arc::new(create_test_buffer(2, 10000));
        cache.put("second.wav".to_string(), buffer2);

        // Mark first as playing
        cache.mark_playing("first.wav");

        // Add third buffer - should evict second (first is protected)
        let buffer3 = Arc::new(create_test_buffer(2, 10000));
        cache.put("third.wav".to_string(), buffer3);
        assert!(cache.contains("first.wav")); // protected
        assert!(!cache.contains("second.wav")); // evicted
        assert!(cache.contains("third.wav"));
    }

    #[test]
    fn test_mark_not_playing() {
        let buffer_size = 2 * 10000 * 4;
        let mut cache = MemoryCache::with_max_size(buffer_size * 2);

        let buffer1 = Arc::new(create_test_buffer(2, 10000));
        cache.put("first.wav".to_string(), buffer1);

        cache.mark_playing("first.wav");
        assert!(cache.is_playing("first.wav"));

        cache.mark_not_playing("first.wav");
        assert!(!cache.is_playing("first.wav"));
    }

    #[test]
    fn test_eviction_multiple_candidates() {
        let buffer_size = 2 * 10000 * 4;
        // Limit that fits 3 buffers
        let mut cache = MemoryCache::with_max_size(buffer_size * 3);

        // Add 3 buffers
        let buffer1 = Arc::new(create_test_buffer(2, 10000));
        cache.put("first.wav".to_string(), buffer1);
        let buffer2 = Arc::new(create_test_buffer(2, 10000));
        cache.put("second.wav".to_string(), buffer2);
        let buffer3 = Arc::new(create_test_buffer(2, 10000));
        cache.put("third.wav".to_string(), buffer3);

        // Access second
        let _ = cache.get("second.wav");

        // Mark third as playing
        cache.mark_playing("third.wav");

        // Add fourth buffer - should evict first (LRU and not protected)
        let buffer4 = Arc::new(create_test_buffer(2, 10000));
        cache.put("fourth.wav".to_string(), buffer4);
        assert!(!cache.contains("first.wav")); // evicted (oldest, not protected)
        assert!(cache.contains("second.wav")); // kept (accessed more recently)
        assert!(cache.contains("third.wav")); // kept (playing)
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
}
