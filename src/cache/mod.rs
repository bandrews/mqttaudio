// ABOUTME: Caching subsystem for audio files.
// ABOUTME: Manages memory and disk caches with HTTP validation.

pub mod disk;
pub mod memory;

use crate::audio::decoder;
use crate::audio::types::DecodedBuffer;
use disk::{CacheError, DiskCache};
use memory::MemoryCache;
use std::path::PathBuf;
use std::sync::Arc;

/// Unified cache manager coordinating memory and disk caches
pub struct CacheManager {
    memory_cache: MemoryCache,
    disk_cache: DiskCache,
}

impl CacheManager {
    /// Create a new cache manager
    pub fn new(cache_dir: PathBuf) -> Result<Self, CacheError> {
        let disk_cache = DiskCache::new(cache_dir)?;
        let memory_cache = MemoryCache::new();

        Ok(Self {
            memory_cache,
            disk_cache,
        })
    }

    /// Get or load an audio file, handling both HTTP URLs and local files
    /// Returns a decoded buffer ready for playback
    pub async fn get_or_load(
        &mut self,
        file_path: &str,
        target_sample_rate: u32,
    ) -> Result<Arc<DecodedBuffer>, Box<dyn std::error::Error>> {
        // Check memory cache first
        if let Some(buffer) = self.memory_cache.get(file_path) {
            tracing::debug!("Memory cache hit for: {}", file_path);
            return Ok(buffer);
        }

        // Determine if this is an HTTP URL or local file
        let local_path = if file_path.starts_with("http://") || file_path.starts_with("https://") {
            // HTTP URL - check disk cache
            if self.disk_cache.is_cached(file_path) {
                tracing::debug!("Disk cache hit for: {}", file_path);
                let entry = self.disk_cache.get_entry(file_path).unwrap();
                self.disk_cache.get_cached_file_path(entry)
            } else {
                // Download and cache
                tracing::info!("Cache miss, downloading: {}", file_path);
                self.disk_cache.download_and_cache(file_path).await?
            }
        } else {
            // Local file path
            PathBuf::from(file_path)
        };

        // Decode the file
        tracing::debug!("Decoding: {}", local_path.display());
        let buffer = decoder::decode_file(
            local_path.to_str().unwrap(),
            Some(target_sample_rate),
        )?;

        // Store in memory cache
        let arc_buffer = Arc::new(buffer);
        self.memory_cache.put(file_path.to_string(), arc_buffer.clone());

        Ok(arc_buffer)
    }

    /// Precache a file (download and decode) without playing it
    pub async fn precache(
        &mut self,
        file_path: &str,
        target_sample_rate: u32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let _ = self.get_or_load(file_path, target_sample_rate).await?;
        tracing::info!("Precached: {}", file_path);
        Ok(())
    }

    /// Clear all caches (memory and disk)
    pub fn clear_all(&mut self) -> Result<(), CacheError> {
        self.memory_cache.clear();
        self.disk_cache.clear_all()?;
        tracing::info!("Cleared all caches");
        Ok(())
    }

    /// Invalidate a specific file from both caches
    pub fn invalidate(&mut self, file_path: &str) -> Result<(), CacheError> {
        self.memory_cache.remove(file_path);
        self.disk_cache.remove_entry(file_path)?;
        tracing::info!("Invalidated cache for: {}", file_path);
        Ok(())
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            memory_entries: self.memory_cache.len(),
            memory_mb: self.memory_cache.memory_usage_mb(),
            disk_entries: self.disk_cache.entry_count(),
        }
    }
}

/// Cache statistics
pub struct CacheStats {
    pub memory_entries: usize,
    pub memory_mb: f64,
    pub disk_entries: usize,
}
