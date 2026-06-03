// ABOUTME: Caching subsystem for audio files.
// ABOUTME: Manages memory and disk caches with HTTP validation.

pub mod disk;
pub mod http_stream;
pub mod memory;

use crate::audio::decoder;
use crate::audio::streaming::{SampleBuffer, StreamingBuffer};
use crate::audio::streaming_decoder::StreamingDecoder;
use crate::audio::types::DecodedBuffer;
use crate::config::ResamplerQuality;
use disk::{CacheError, DiskCache};
use memory::MemoryCache;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use symphonia::core::probe::Hint;

/// Tracks an active streaming load operation
#[derive(Clone)]
pub struct ActiveLoad {
    /// The streaming buffer being filled
    pub buffer: Arc<RwLock<StreamingBuffer>>,
    /// URL/path being loaded
    #[allow(dead_code)] // Used for logging in cleanup_completed_loads
    pub path: String,
}

/// Unified cache manager coordinating memory and disk caches
pub struct CacheManager {
    memory_cache: MemoryCache,
    disk_cache: DiskCache,
    resampler_quality: ResamplerQuality,
    /// Currently active streaming loads (path -> ActiveLoad)
    active_loads: HashMap<String, ActiveLoad>,
    /// Local-path allowlist; empty = allow-all (open by default).
    allowed_directories: Vec<String>,
    /// Revalidate a disk-cached HTTP entry once it is older than this many seconds.
    revalidate_after_seconds: u64,
}

impl CacheManager {
    /// Create a new cache manager with specified resampler quality and memory limit.
    /// max_memory_mb of 0 means unlimited.
    pub fn with_options(
        cache_dir: PathBuf,
        resampler_quality: ResamplerQuality,
        max_memory_mb: u32,
        allowed_directories: Vec<String>,
        revalidate_after_seconds: u64,
    ) -> Result<Self, CacheError> {
        let disk_cache = DiskCache::new(cache_dir)?;
        let max_bytes = if max_memory_mb == 0 {
            0 // 0 means unlimited in MemoryCache::with_max_size
        } else {
            (max_memory_mb as usize) * 1024 * 1024
        };
        let memory_cache = MemoryCache::with_max_size(max_bytes);

        tracing::info!(
            "Cache manager initialized (memory limit: {})",
            if max_memory_mb == 0 {
                "unlimited".to_string()
            } else {
                format!("{} MB", max_memory_mb)
            }
        );

        Ok(Self {
            memory_cache,
            disk_cache,
            resampler_quality,
            active_loads: HashMap::new(),
            allowed_directories,
            revalidate_after_seconds,
        })
    }

    /// Create a new cache manager with specified resampler quality and no memory limit.
    pub fn with_quality(
        cache_dir: PathBuf,
        resampler_quality: ResamplerQuality,
    ) -> Result<Self, CacheError> {
        Self::with_options(cache_dir, resampler_quality, 0, Vec::new(), 300)
    }

    /// Create a new cache manager with default (Fast) resampler quality and no memory limit.
    /// Primarily used by tests and benchmarks.
    #[allow(dead_code)]
    pub fn new(cache_dir: PathBuf) -> Result<Self, CacheError> {
        Self::with_quality(cache_dir, ResamplerQuality::default())
    }

    /// Get or load an audio file with streaming support.
    /// Returns a SampleBuffer that may be either complete (from cache) or
    /// streaming (still loading). A streaming buffer is returned immediately and
    /// playback begins right away; the audio callback emits silence for any frame
    /// that has not been decoded yet, so early frames may be silent until data
    /// arrives.
    pub async fn get_or_load_streaming(
        &mut self,
        file_path: &str,
        target_sample_rate: u32,
    ) -> Result<SampleBuffer, Box<dyn std::error::Error + Send + Sync>> {
        // Promote any finished streaming loads to the memory cache and drop their
        // active_loads entries first, so a replay of a now-complete URL is served
        // from cache instead of rejoining a stale streaming buffer.
        self.cleanup_completed_loads();

        // Check memory cache first - return as Complete
        if let Some(buffer) = self.memory_cache.get(file_path) {
            tracing::debug!("Memory cache hit for: {}", file_path);
            return Ok(SampleBuffer::Complete(buffer));
        }

        // Check if already loading - return existing streaming buffer
        if let Some(active) = self.active_loads.get(file_path) {
            tracing::debug!("Joining existing streaming load for: {}", file_path);
            return Ok(SampleBuffer::Streaming(Arc::clone(&active.buffer)));
        }

        // For HTTP URLs, use streaming approach
        if file_path.starts_with("http://") || file_path.starts_with("https://") {
            // Check disk cache first - if cached, load from disk (fast)
            if self.disk_cache.is_cached(file_path) {
                tracing::debug!("Disk cache hit for: {}", file_path);
                let _ = self
                    .disk_cache
                    .revalidate_if_due(
                        file_path,
                        std::time::Duration::from_secs(self.revalidate_after_seconds),
                    )
                    .await;
                let entry = self.disk_cache.get_entry(file_path).unwrap();
                let local_path = self.disk_cache.get_cached_file_path(entry);

                let path = local_path.to_string_lossy().into_owned();
                let quality = self.resampler_quality;
                let buffer = tokio::task::spawn_blocking(move || {
                    decoder::decode_file(&path, Some(target_sample_rate), quality)
                })
                .await??;

                let arc_buffer = Arc::new(buffer);
                self.memory_cache
                    .put(file_path.to_string(), arc_buffer.clone());
                return Ok(SampleBuffer::Complete(arc_buffer));
            }

            // Start streaming download
            tracing::info!("Starting streaming load for: {}", file_path);
            return self
                .start_streaming_load(file_path, target_sample_rate)
                .await;
        }

        // Local file - load from disk (could stream later for very large files)
        tracing::debug!("Loading local file: {}", file_path);
        if !crate::config::Config::is_path_allowed(
            std::path::Path::new(file_path),
            &self.allowed_directories,
        ) {
            return Err(format!(
                "file path not permitted by allowed_directories: {}",
                file_path
            )
            .into());
        }
        let path = file_path.to_string();
        let quality = self.resampler_quality;
        let buffer = tokio::task::spawn_blocking(move || {
            decoder::decode_file(&path, Some(target_sample_rate), quality)
        })
        .await??;

        let arc_buffer = Arc::new(buffer);
        self.memory_cache
            .put(file_path.to_string(), arc_buffer.clone());
        Ok(SampleBuffer::Complete(arc_buffer))
    }

    /// Start a streaming load for an HTTP URL
    async fn start_streaming_load(
        &mut self,
        url: &str,
        target_sample_rate: u32,
    ) -> Result<SampleBuffer, Box<dyn std::error::Error + Send + Sync>> {
        // Start HTTP stream
        let reader = http_stream::start_http_stream(url)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        // Get estimated frames from content length (rough estimate)
        let (_bytes_so_far, content_length) = reader.progress();
        let estimated_frames = content_length.map(|len| {
            // Rough estimate: assume 16-bit stereo @ target rate
            // This will be refined once the decoder starts
            len / 4
        });

        // Create streaming buffer (channels will be updated by decoder)
        let streaming_buffer = Arc::new(RwLock::new(StreamingBuffer::new(
            2, // Default, will be corrected
            target_sample_rate,
            estimated_frames,
        )));

        // Track the active load
        let active_load = ActiveLoad {
            buffer: Arc::clone(&streaming_buffer),
            path: url.to_string(),
        };
        self.active_loads.insert(url.to_string(), active_load);

        // Create hint for format detection
        let mut hint = Hint::new();
        if let Some(ext) = url.rsplit('.').next() {
            hint.with_extension(ext);
        }

        // Spawn decode task
        let buffer_clone = Arc::clone(&streaming_buffer);
        let quality = self.resampler_quality;
        let url_clone = url.to_string();

        tokio::task::spawn_blocking(move || {
            Self::decode_streaming(
                reader,
                hint,
                target_sample_rate,
                quality,
                buffer_clone,
                url_clone,
            );
        });

        Ok(SampleBuffer::Streaming(streaming_buffer))
    }

    /// Background decode task - runs in spawn_blocking
    fn decode_streaming(
        reader: http_stream::HttpStreamReader,
        hint: Hint,
        target_sample_rate: u32,
        quality: ResamplerQuality,
        buffer: Arc<RwLock<StreamingBuffer>>,
        url: String,
    ) {
        // Create streaming decoder
        let decoder =
            match StreamingDecoder::new(reader, Some(&hint), Some(target_sample_rate), quality) {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!("Failed to create decoder for {}: {}", url, e);
                    if let Ok(mut guard) = buffer.write() {
                        guard.mark_error(format!("Decoder init failed: {}", e));
                    }
                    return;
                }
            };

        // Update buffer with actual channel count
        let channels = decoder.channels();
        if let Ok(mut guard) = buffer.write() {
            guard.channels = channels;
        }

        tracing::debug!(
            "Streaming decode started: {} channels, {} Hz",
            channels,
            target_sample_rate
        );

        // Decode chunks and append to buffer
        let mut total_samples = 0;
        for chunk_result in decoder {
            match chunk_result {
                Ok(samples) => {
                    total_samples += samples.len();
                    if let Ok(mut guard) = buffer.write() {
                        guard.append(&samples);
                    }
                }
                Err(e) => {
                    tracing::error!("Decode error for {}: {}", url, e);
                    if let Ok(mut guard) = buffer.write() {
                        guard.mark_error(format!("Decode error: {}", e));
                    }
                    return;
                }
            }
        }

        // Mark complete
        if let Ok(mut guard) = buffer.write() {
            guard.mark_complete();
        }

        let frames = total_samples / channels.max(1);
        tracing::info!(
            "Streaming decode complete for {}: {} frames ({:.2}s)",
            url,
            frames,
            frames as f64 / target_sample_rate as f64
        );
    }

    /// Check if a file is currently being loaded
    // Allow dead_code until Phase 10 connects streaming to main.rs
    #[allow(dead_code)]
    pub fn is_loading(&self, file_path: &str) -> bool {
        if let Some(active) = self.active_loads.get(file_path) {
            if let Ok(guard) = active.buffer.read() {
                return !guard.is_complete() && !guard.has_error();
            }
        }
        false
    }

    /// Get loading progress for a file: (frames_loaded, total_frames_estimate)
    // Allow dead_code until Phase 10 connects streaming to main.rs
    #[allow(dead_code)]
    pub fn loading_progress(&self, file_path: &str) -> Option<(usize, Option<usize>)> {
        if let Some(active) = self.active_loads.get(file_path) {
            if let Ok(guard) = active.buffer.read() {
                return Some((guard.frames_available(), guard.total_frames));
            }
        }
        None
    }

    /// Clean up completed loads and promote to memory cache
    pub fn cleanup_completed_loads(&mut self) {
        let completed: Vec<String> = self
            .active_loads
            .iter()
            .filter_map(|(path, active)| {
                if let Ok(guard) = active.buffer.read() {
                    if guard.is_complete() {
                        return Some(path.clone());
                    }
                }
                None
            })
            .collect();

        for path in completed {
            if let Some(active) = self.active_loads.remove(&path) {
                // Try to promote to memory cache as DecodedBuffer
                if let Ok(guard) = active.buffer.read() {
                    if guard.is_complete() {
                        // Create DecodedBuffer from streaming data
                        let decoded = DecodedBuffer::new(
                            guard.data().to_vec(),
                            guard.channels,
                            guard.sample_rate,
                        );
                        self.memory_cache.put(path.clone(), Arc::new(decoded));
                        tracing::debug!("Promoted streaming buffer to cache: {}", path);
                    }
                }
            }
        }
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
                let _ = self
                    .disk_cache
                    .revalidate_if_due(
                        file_path,
                        std::time::Duration::from_secs(self.revalidate_after_seconds),
                    )
                    .await;
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
            if !crate::config::Config::is_path_allowed(
                std::path::Path::new(file_path),
                &self.allowed_directories,
            ) {
                return Err(format!(
                    "file path not permitted by allowed_directories: {}",
                    file_path
                )
                .into());
            }
            PathBuf::from(file_path)
        };

        // Decode the file
        tracing::debug!("Decoding: {}", local_path.display());
        let path = local_path.to_string_lossy().into_owned();
        let quality = self.resampler_quality;
        let buffer = tokio::task::spawn_blocking(move || {
            decoder::decode_file(&path, Some(target_sample_rate), quality)
        })
        .await??;

        // Store in memory cache
        let arc_buffer = Arc::new(buffer);
        self.memory_cache
            .put(file_path.to_string(), arc_buffer.clone());

        // Log overall cache stats
        self.log_stats();

        Ok(arc_buffer)
    }

    /// Precache a file (download and decode) without playing it.
    /// Blocks until the file is fully loaded.
    pub async fn precache(
        &mut self,
        file_path: &str,
        target_sample_rate: u32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let _ = self.get_or_load(file_path, target_sample_rate).await?;
        tracing::info!("Precached: {}", file_path);
        Ok(())
    }

    /// Start precaching a file without blocking.
    /// Returns immediately after initiating the load. The file loads in background.
    /// If the file is already cached or loading, this is a no-op.
    pub async fn precache_streaming(
        &mut self,
        file_path: &str,
        target_sample_rate: u32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let buffer = self
            .get_or_load_streaming(file_path, target_sample_rate)
            .await?;
        if buffer.is_complete() {
            tracing::info!("Precache started (already cached): {}", file_path);
        } else {
            tracing::info!("Precache started (loading in background): {}", file_path);
        }
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

    /// Flush the disk-cache metadata to disk (called on graceful shutdown).
    pub fn flush_metadata(&self) -> Result<(), CacheError> {
        self.disk_cache.save_metadata()
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

    #[tokio::test]
    async fn load_rejects_local_path_outside_allowlist() {
        let allowed_dir = TempDir::new().unwrap();
        let cache_dir = TempDir::new().unwrap();
        let allowed = vec![allowed_dir.path().to_string_lossy().into_owned()];
        let mut cm = CacheManager::with_options(
            cache_dir.path().to_path_buf(),
            ResamplerQuality::default(),
            0,
            allowed,
            300,
        )
        .unwrap();

        // A path outside the configured allowlist must be refused before any open.
        let result = cm.get_or_load_streaming("/etc/passwd", 48000).await;
        assert!(result.is_err());
        assert!(result
            .err()
            .unwrap()
            .to_string()
            .contains("allowed_directories"));
    }

    #[tokio::test]
    async fn completed_streaming_load_is_promoted_and_active_loads_bounded() {
        let cache_dir = TempDir::new().unwrap();
        let mut cm =
            CacheManager::with_quality(cache_dir.path().to_path_buf(), ResamplerQuality::default())
                .unwrap();
        let url = "http://example.com/sound.wav";

        // Simulate a finished streaming decode: a Complete buffer tracked in
        // active_loads (no network needed).
        let mut sb = StreamingBuffer::new(2, 48000, Some(4));
        sb.append(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]); // 4 stereo frames
        sb.mark_complete();
        cm.active_loads.insert(
            url.to_string(),
            ActiveLoad {
                buffer: Arc::new(RwLock::new(sb)),
                path: url.to_string(),
            },
        );

        // Replaying the URL must serve the promoted Complete cache entry rather
        // than rejoining the streaming buffer, and must drop the active_loads entry.
        let buf = cm.get_or_load_streaming(url, 48000).await.unwrap();
        assert!(
            matches!(buf, SampleBuffer::Complete(_)),
            "replay should return a Complete (cached) buffer, not a Streaming one"
        );
        assert!(
            cm.active_loads.is_empty(),
            "a completed load must be removed from active_loads"
        );
    }
}
