// ABOUTME: Caching subsystem for audio files.
// ABOUTME: Manages memory and disk caches with HTTP validation.

pub mod disk;
pub mod memory;

use crate::audio::decoder;
use crate::audio::types::DecodedBuffer;
use crate::config::ResamplerQuality;
use disk::{CacheError, DiskCache};
use memory::MemoryCache;
use std::path::PathBuf;
use std::sync::Arc;

/// Unified cache manager coordinating memory and disk caches
pub struct CacheManager {
    memory_cache: MemoryCache,
    disk_cache: DiskCache,
    resampler_quality: ResamplerQuality,
}

impl CacheManager {
    /// Create a new cache manager with specified resampler quality
    pub fn with_quality(cache_dir: PathBuf, resampler_quality: ResamplerQuality) -> Result<Self, CacheError> {
        let disk_cache = DiskCache::new(cache_dir)?;
        let memory_cache = MemoryCache::new();

        Ok(Self {
            memory_cache,
            disk_cache,
            resampler_quality,
        })
    }

    /// Create a new cache manager with default (Fast) resampler quality.
    /// Primarily used by tests and benchmarks.
    #[allow(dead_code)]
    pub fn new(cache_dir: PathBuf) -> Result<Self, CacheError> {
        Self::with_quality(cache_dir, ResamplerQuality::default())
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
                let path = self.disk_cache.download_and_cache(file_path).await?;
                // Log stats after download
                self.log_stats();
                path
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
            self.resampler_quality,
        )?;

        // Store in memory cache
        let arc_buffer = Arc::new(buffer);
        self.memory_cache.put(file_path.to_string(), arc_buffer.clone());

        // Log overall cache stats
        self.log_stats();

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

    /// Get memory cache statistics
    pub fn memory_stats(&self) -> CacheStats {
        CacheStats {
            entry_count: self.memory_cache.len(),
            size_bytes: self.memory_cache.memory_usage_bytes() as u64,
        }
    }

    /// Get disk cache statistics
    pub fn disk_stats(&self) -> CacheStats {
        CacheStats {
            entry_count: self.disk_cache.entry_count(),
            size_bytes: self.disk_cache.total_size_bytes(),
        }
    }

    /// Log current cache statistics (for verbose mode)
    pub fn log_stats(&self) {
        let mem = self.memory_stats();
        let disk = self.disk_stats();
        tracing::debug!(
            "Cache stats - Memory: {} entries ({:.2} MB), Disk: {} entries ({:.2} MB)",
            mem.entry_count,
            mem.size_bytes as f64 / (1024.0 * 1024.0),
            disk.entry_count,
            disk.size_bytes as f64 / (1024.0 * 1024.0)
        );
    }
}

/// Cache size statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub entry_count: usize,
    pub size_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_cache_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        let cache_manager = CacheManager::new(temp_dir.path().to_path_buf()).unwrap();

        let stats = cache_manager.memory_stats();
        assert_eq!(stats.entry_count, 0);
        assert_eq!(stats.size_bytes, 0);
    }

    #[test]
    fn test_cache_stats_initial_empty() {
        let temp_dir = TempDir::new().unwrap();
        let cache_manager = CacheManager::new(temp_dir.path().to_path_buf()).unwrap();

        let mem_stats = cache_manager.memory_stats();
        let disk_stats = cache_manager.disk_stats();

        assert_eq!(mem_stats.entry_count, 0);
        assert_eq!(mem_stats.size_bytes, 0);
        assert_eq!(disk_stats.entry_count, 0);
        assert_eq!(disk_stats.size_bytes, 0);
    }

    #[test]
    fn test_cache_stats_struct() {
        let stats = CacheStats {
            entry_count: 5,
            size_bytes: 1024 * 1024 * 10, // 10 MB
        };

        assert_eq!(stats.entry_count, 5);
        assert_eq!(stats.size_bytes, 10 * 1024 * 1024);
    }

    #[test]
    fn test_cache_stats_clone() {
        let stats = CacheStats {
            entry_count: 3,
            size_bytes: 12345,
        };

        let cloned = stats.clone();
        assert_eq!(cloned.entry_count, 3);
        assert_eq!(cloned.size_bytes, 12345);
    }
}
