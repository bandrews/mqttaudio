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

/// Disk-cache metadata captured when a streaming download starts, applied to
/// the disk cache once the download completes and its file is finalized
pub struct PendingDiskWrite {
    pub cache_filename: String,
    pub final_path: PathBuf,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_type: Option<String>,
}

/// Tracks an active streaming load operation
pub struct ActiveLoad {
    /// The streaming buffer being filled
    pub buffer: Arc<RwLock<StreamingBuffer>>,
    /// Disk-cache registration to perform once the download completes
    pub pending_disk_write: Option<PendingDiskWrite>,
}

/// Cache behavior settings, from the `cache` config section
#[derive(Debug, Clone)]
pub struct CacheOptions {
    pub resampler_quality: ResamplerQuality,
    /// Memory cache limit in MB; 0 = unlimited
    pub max_memory_mb: u32,
    /// When false, nothing is read from or written to the disk cache
    pub disk_enabled: bool,
    /// How long a disk-cached URL is served without asking the server whether
    /// it changed; 0 = check on every access
    pub revalidate_after_seconds: u64,
}

impl Default for CacheOptions {
    fn default() -> Self {
        Self {
            resampler_quality: ResamplerQuality::default(),
            max_memory_mb: 0,
            disk_enabled: true,
            revalidate_after_seconds: 300,
        }
    }
}

/// Unified cache manager coordinating memory and disk caches
pub struct CacheManager {
    memory_cache: MemoryCache,
    /// None when the disk cache is disabled in config
    disk_cache: Option<DiskCache>,
    resampler_quality: ResamplerQuality,
    revalidate_after_seconds: u64,
    /// When each URL was last checked against its server (in-memory, so a
    /// dead server is asked at most once per interval rather than per play)
    last_revalidation_attempt: HashMap<String, std::time::Instant>,
    /// Currently active streaming loads (path -> ActiveLoad)
    active_loads: HashMap<String, ActiveLoad>,
}

impl CacheManager {
    /// Create a new cache manager with the given options.
    pub fn with_options(cache_dir: PathBuf, options: CacheOptions) -> Result<Self, CacheError> {
        let disk_cache = if options.disk_enabled {
            Some(DiskCache::new(cache_dir)?)
        } else {
            None
        };
        let max_bytes = if options.max_memory_mb == 0 {
            0 // 0 means unlimited in MemoryCache::with_max_size
        } else {
            (options.max_memory_mb as usize) * 1024 * 1024
        };
        let memory_cache = MemoryCache::with_max_size(max_bytes);

        tracing::info!(
            "Cache manager initialized (memory limit: {}, disk cache: {})",
            if options.max_memory_mb == 0 {
                "unlimited".to_string()
            } else {
                format!("{} MB", options.max_memory_mb)
            },
            if options.disk_enabled { "on" } else { "off" }
        );

        Ok(Self {
            memory_cache,
            disk_cache,
            resampler_quality: options.resampler_quality,
            revalidate_after_seconds: options.revalidate_after_seconds,
            last_revalidation_attempt: HashMap::new(),
            active_loads: HashMap::new(),
        })
    }

    /// Create a new cache manager with specified resampler quality and no memory limit.
    pub fn with_quality(cache_dir: PathBuf, resampler_quality: ResamplerQuality) -> Result<Self, CacheError> {
        Self::with_options(cache_dir, CacheOptions {
            resampler_quality,
            ..CacheOptions::default()
        })
    }

    /// Create a new cache manager with default (Fast) resampler quality and no memory limit.
    /// Primarily used by tests and benchmarks.
    #[allow(dead_code)]
    pub fn new(cache_dir: PathBuf) -> Result<Self, CacheError> {
        Self::with_quality(cache_dir, ResamplerQuality::default())
    }

    /// Get or load an audio file with streaming support.
    /// Returns a SampleBuffer that may be either complete (from cache) or
    /// streaming (still loading). For streaming buffers, playback can begin
    /// as soon as MIN_BUFFER_FRAMES are available.
    pub async fn get_or_load_streaming(
        &mut self,
        file_path: &str,
        target_sample_rate: u32,
    ) -> Result<SampleBuffer, Box<dyn std::error::Error + Send + Sync>> {
        // Fold finished loads into the caches before answering
        self.cleanup_completed_loads();

        // Ask the server whether a cached copy is still current, when due
        self.maybe_revalidate(file_path).await;

        // Check memory cache first - return as Complete
        if let Some(buffer) = self.memory_cache.get(file_path) {
            tracing::debug!("Memory cache hit for: {}", file_path);
            return Ok(SampleBuffer::Complete(buffer));
        }

        // Check if already loading - return existing streaming buffer.
        // A load that already failed is evicted instead of joined, so one
        // transient network or decode error does not poison the URL until restart.
        if let Some(active) = self.active_loads.get(file_path) {
            let failed = active.buffer.read().map(|b| b.has_error()).unwrap_or(false);
            if failed {
                tracing::warn!("Previous streaming load of {} failed; retrying", file_path);
                self.active_loads.remove(file_path);
            } else {
                tracing::debug!("Joining existing streaming load for: {}", file_path);
                return Ok(SampleBuffer::Streaming(Arc::clone(&active.buffer)));
            }
        }

        // For HTTP URLs, use streaming approach
        if file_path.starts_with("http://") || file_path.starts_with("https://") {
            // Check disk cache first - if cached, load from disk (fast)
            if self.disk_cache.as_ref().is_some_and(|d| d.is_cached(file_path)) {
                tracing::debug!("Disk cache hit for: {}", file_path);
                let disk = self.disk_cache.as_ref().unwrap();
                let entry = disk.get_entry(file_path).unwrap();
                let local_path = disk.get_cached_file_path(entry);

                let buffer = decoder::decode_file(
                    local_path.to_str().unwrap(),
                    Some(target_sample_rate),
                    self.resampler_quality,
                )?;

                let arc_buffer = Arc::new(buffer);
                self.memory_cache.put(file_path.to_string(), arc_buffer.clone());
                return Ok(SampleBuffer::Complete(arc_buffer));
            }

            // Start streaming download
            tracing::info!("Starting streaming load for: {}", file_path);
            return self.start_streaming_load(file_path, target_sample_rate).await;
        }

        // Local file - load from disk (could stream later for very large files)
        tracing::debug!("Loading local file: {}", file_path);
        let buffer = decoder::decode_file(
            file_path,
            Some(target_sample_rate),
            self.resampler_quality,
        )?;

        let arc_buffer = Arc::new(buffer);
        self.memory_cache.put(file_path.to_string(), arc_buffer.clone());
        Ok(SampleBuffer::Complete(arc_buffer))
    }

    /// Whether it is time to ask the server if a cached copy of this URL is
    /// still current. Gated by an in-memory attempt timestamp so a dead
    /// server is retried at most once per interval, not once per play.
    fn revalidation_due(&self, url: &str) -> bool {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return false;
        }
        let Some(disk) = self.disk_cache.as_ref() else {
            return false;
        };
        let Some(entry) = disk.get_entry(url) else {
            return false;
        };
        if !disk.is_cached(url) {
            return false;
        }

        let interval = std::time::Duration::from_secs(self.revalidate_after_seconds);
        if let Some(attempted) = self.last_revalidation_attempt.get(url) {
            if attempted.elapsed() < interval {
                return false;
            }
        } else {
            // No attempt this run - go by the persisted validation time
            if let Ok(validated) = chrono::DateTime::parse_from_rfc3339(&entry.last_validated) {
                let age = chrono::Utc::now().signed_duration_since(validated);
                if age.to_std().map(|a| a < interval).unwrap_or(false) {
                    return false;
                }
            }
        }
        true
    }

    /// Revalidate a cached URL against its server if due. A changed file
    /// replaces the disk copy and drops the stale memory entry; an
    /// unreachable server leaves the cached copy in service.
    async fn maybe_revalidate(&mut self, url: &str) {
        if !self.revalidation_due(url) {
            return;
        }
        self.last_revalidation_attempt.insert(url.to_string(), std::time::Instant::now());

        let disk = self.disk_cache.as_mut().expect("revalidation_due checked the disk cache");
        match disk.revalidate(url).await {
            disk::Freshness::Fresh => {
                tracing::debug!("Cached copy of {} is still current", url);
            }
            disk::Freshness::Replaced => {
                tracing::info!("Server copy of {} changed; cache refreshed", url);
                self.memory_cache.remove(url);
            }
            disk::Freshness::Unknown => {
                tracing::warn!("Could not revalidate {}; serving the cached copy", url);
            }
        }
    }

    /// Start a streaming load for an HTTP URL
    async fn start_streaming_load(
        &mut self,
        url: &str,
        target_sample_rate: u32,
    ) -> Result<SampleBuffer, Box<dyn std::error::Error + Send + Sync>> {
        // When the disk cache is on, tee the download into it so the file
        // survives a restart
        let persist = self.disk_cache.as_ref().map(|disk| {
            let cache_filename = DiskCache::cache_filename_for_url(url);
            let final_path = disk.files_dir().join(&cache_filename);
            let temp_path = final_path.with_extension("part");
            (cache_filename, http_stream::PersistTarget { temp_path, final_path })
        });

        // Start HTTP stream
        let (reader, response_info) = http_stream::start_http_stream_with_persist(
            url,
            persist.as_ref().map(|(_, target)| target.clone()),
        )
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        // Get estimated frames from content length. Encoded bytes only map
        // to frames for uncompressed WAV (~4 bytes per 16-bit stereo frame);
        // for compressed formats no estimate is better than a wildly wrong one.
        let (_bytes_so_far, content_length) = reader.progress();
        let url_path = url.split(['?', '#']).next().unwrap_or(url);
        let estimated_frames = if url_path.to_lowercase().ends_with(".wav") {
            content_length.map(|len| len / 4)
        } else {
            None
        };

        // Create streaming buffer (channels will be updated by decoder)
        let streaming_buffer = Arc::new(RwLock::new(StreamingBuffer::new(
            2, // Default, will be corrected
            target_sample_rate,
            estimated_frames,
        )));

        // Track the active load, remembering the disk registration to make
        // once the download lands
        let pending_disk_write = persist.map(|(cache_filename, target)| PendingDiskWrite {
            cache_filename,
            final_path: target.final_path,
            etag: response_info.etag,
            last_modified: response_info.last_modified,
            content_type: response_info.content_type,
        });
        let active_load = ActiveLoad {
            buffer: Arc::clone(&streaming_buffer),
            pending_disk_write,
        };
        self.active_loads.insert(url.to_string(), active_load);

        // Create hint for format detection from the URL path (query strings
        // and non-audio suffixes would mislead the probe)
        let mut hint = Hint::new();
        if let Some(ext) = url_path.rsplit('/').next().and_then(|f| f.rsplit('.').next()) {
            let ext = ext.to_lowercase();
            if matches!(ext.as_str(), "wav" | "mp3" | "ogg" | "flac") {
                hint.with_extension(&ext);
            }
        }

        // Spawn decode task
        let buffer_clone = Arc::clone(&streaming_buffer);
        let quality = self.resampler_quality;
        let url_clone = url.to_string();

        tokio::task::spawn_blocking(move || {
            Self::decode_streaming(reader, hint, target_sample_rate, quality, buffer_clone, url_clone);
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
        let decoder = match StreamingDecoder::new(
            reader,
            Some(&hint),
            Some(target_sample_rate),
            quality,
        ) {
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

    /// Fold finished streaming loads into the caches: completed loads are
    /// promoted into the size-limited memory cache and registered in the disk
    /// cache (their bytes were written through during download); failed loads
    /// are dropped so the URL can be retried.
    pub fn cleanup_completed_loads(&mut self) {
        let finished: Vec<String> = self.active_loads
            .iter()
            .filter_map(|(path, active)| {
                if let Ok(guard) = active.buffer.read() {
                    if guard.is_complete() || guard.has_error() {
                        return Some(path.clone());
                    }
                }
                None
            })
            .collect();

        for path in finished {
            if let Some(active) = self.active_loads.remove(&path) {
                let Ok(guard) = active.buffer.read() else { continue };
                if !guard.is_complete() {
                    tracing::debug!("Dropping failed streaming load: {}", path);
                    continue;
                }

                // Promote to the memory cache as a DecodedBuffer so the LRU
                // budget governs it
                let decoded = DecodedBuffer::new(
                    guard.data().to_vec(),
                    guard.channels,
                    guard.sample_rate,
                );
                drop(guard);
                self.memory_cache.put(path.clone(), Arc::new(decoded));
                tracing::debug!("Promoted streaming buffer to cache: {}", path);

                // Register the written-through file in the disk cache
                if let (Some(pending), Some(disk)) = (active.pending_disk_write, self.disk_cache.as_mut()) {
                    match std::fs::metadata(&pending.final_path) {
                        Ok(meta) => {
                            disk.put_entry(path.clone(), disk::CacheEntry {
                                local_file: pending.cache_filename,
                                etag: pending.etag,
                                last_modified: pending.last_modified,
                                last_validated: chrono::Utc::now().to_rfc3339(),
                                file_size: meta.len(),
                                content_type: pending.content_type,
                            });
                            if let Err(e) = disk.save_metadata() {
                                tracing::warn!("Failed to save cache metadata for {}: {}", path, e);
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Streamed download of {} completed but its cache file is missing: {}",
                                path, e
                            );
                        }
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
        // Fold finished loads into the caches and revalidate if due
        self.cleanup_completed_loads();
        self.maybe_revalidate(file_path).await;

        // Check memory cache first
        if let Some(buffer) = self.memory_cache.get(file_path) {
            tracing::debug!("Memory cache hit for: {}", file_path);
            return Ok(buffer);
        }

        // Determine if this is an HTTP URL or local file
        let is_url = file_path.starts_with("http://") || file_path.starts_with("https://");
        let local_path = if is_url {
            if self.disk_cache.is_none() {
                // Disk cache disabled: stream into memory and wait for it
                let buffer = self.get_or_load_streaming(file_path, target_sample_rate).await
                    .map_err(|e| -> Box<dyn std::error::Error> { e })?;
                if let Some(decoded) = buffer.as_complete() {
                    return Ok(decoded);
                }
                return self.wait_for_streaming(file_path, buffer).await;
            } else if self.disk_cache.as_ref().unwrap().is_cached(file_path) {
                tracing::debug!("Disk cache hit for: {}", file_path);
                let disk = self.disk_cache.as_ref().unwrap();
                let entry = disk.get_entry(file_path).unwrap();
                disk.get_cached_file_path(entry)
            } else {
                // Download and cache
                tracing::info!("Cache miss, downloading: {}", file_path);
                let path = self.disk_cache.as_mut().unwrap().download_and_cache(file_path).await?;
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
        let buffer = self.get_or_load_streaming(file_path, target_sample_rate).await?;
        if buffer.is_complete() {
            tracing::info!("Precache started (already cached): {}", file_path);
        } else {
            tracing::info!("Precache started (loading in background): {}", file_path);
        }
        Ok(())
    }

    /// Wait for a streaming load to finish, then hand out its decoded buffer.
    /// Used for blocking loads when the disk cache is disabled.
    async fn wait_for_streaming(
        &mut self,
        url: &str,
        buffer: SampleBuffer,
    ) -> Result<Arc<DecodedBuffer>, Box<dyn std::error::Error>> {
        // The download's own stall timeouts bound this loop in practice; the
        // deadline is a backstop
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
        loop {
            if buffer.has_failed() {
                return Err(format!("Streaming load of {} failed", url).into());
            }
            if buffer.is_complete() {
                break;
            }
            if std::time::Instant::now() > deadline {
                return Err(format!("Timed out loading {}", url).into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        self.cleanup_completed_loads();
        self.memory_cache
            .get(url)
            .ok_or_else(|| format!("Load of {} completed but was not cached", url).into())
    }

    /// Clear all caches (memory and disk)
    pub fn clear_all(&mut self) -> Result<(), CacheError> {
        self.memory_cache.clear();
        if let Some(disk) = self.disk_cache.as_mut() {
            disk.clear_all()?;
        }
        tracing::info!("Cleared all caches");
        Ok(())
    }

    /// Invalidate a specific file from both caches
    pub fn invalidate(&mut self, file_path: &str) -> Result<(), CacheError> {
        self.memory_cache.remove(file_path);
        if let Some(disk) = self.disk_cache.as_mut() {
            disk.remove_entry(file_path)?;
        }
        self.last_revalidation_attempt.remove(file_path);
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
            entry_count: self.disk_cache.as_ref().map_or(0, |d| d.entry_count()),
            size_bytes: self.disk_cache.as_ref().map_or(0, |d| d.total_size_bytes()),
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
