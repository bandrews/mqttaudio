// ABOUTME: Memory cache for decoded audio buffers.
// ABOUTME: Provides fast access to frequently used samples.

use crate::audio::types::DecodedBuffer;
use std::collections::HashMap;
use std::sync::Arc;

/// Memory cache for decoded audio buffers
/// Stores fully decoded PCM data ready for immediate playback
pub struct MemoryCache {
    /// Map of URL/path -> decoded buffer
    cache: HashMap<String, Arc<DecodedBuffer>>,
}

impl MemoryCache {
    /// Create a new empty memory cache
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Get a cached buffer for the given URL/path
    pub fn get(&self, key: &str) -> Option<Arc<DecodedBuffer>> {
        self.cache.get(key).map(Arc::clone)
    }

    /// Store a decoded buffer in the cache
    pub fn put(&mut self, key: String, buffer: Arc<DecodedBuffer>) {
        tracing::debug!("Caching decoded buffer for: {}", key);
        self.cache.insert(key, buffer);
    }

    /// Remove a specific entry from the cache
    pub fn remove(&mut self, key: &str) -> bool {
        self.cache.remove(key).is_some()
    }

    /// Clear all cached buffers
    pub fn clear(&mut self) {
        let count = self.cache.len();
        self.cache.clear();
        tracing::info!("Cleared {} decoded buffers from memory cache", count);
    }

    /// Check if a key is in the cache
    #[cfg(test)]
    pub fn contains(&self, key: &str) -> bool {
        self.cache.contains_key(key)
    }

    /// Get the number of cached items
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Check if the cache is empty
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Get memory usage estimate in bytes
    /// Calculates based on decoded PCM data size
    pub fn memory_usage_bytes(&self) -> usize {
        self.cache.values()
            .map(|buf| buf.data.len() * std::mem::size_of::<f32>())
            .sum()
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
        let cache = MemoryCache::new();
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
}
