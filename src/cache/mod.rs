// ABOUTME: Caching subsystem for audio files.
// ABOUTME: Manages memory and disk caches with HTTP validation.

pub mod disk;
pub mod http_stream;
pub mod memory;
pub mod strategy;

use crate::audio::decoder;
use crate::audio::streaming::{SampleBuffer, StreamingBuffer};
use crate::audio::streaming_decoder::StreamingDecoder;
use crate::audio::types::DecodedBuffer;
use crate::config::{FreshnessMode, MemoryCap, ResamplerQuality};
use disk::{CacheError, DiskCache};
use memory::MemoryCache;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use symphonia::core::probe::Hint;

/// Identity of an on-disk source captured when a streaming load starts and
/// re-checked at promotion (D51): a load whose file changed underneath it (an
/// edit, or a revalidation swapping the cached download) is dropped instead of
/// being published as fresh.
#[derive(Clone, Debug)]
pub struct LoadGeneration {
    path: PathBuf,
    mtime: Option<std::time::SystemTime>,
    size: u64,
}

impl LoadGeneration {
    /// Capture the source file's identity at load start. Best-effort: an
    /// unreadable file records nothing (no generation check at promotion).
    fn capture(path: &std::path::Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            path: path.to_path_buf(),
            mtime: meta.modified().ok(),
            size: meta.len(),
        })
    }

    /// Whether the source file is still the one this load decoded. A vanished
    /// file reads as stale (do not publish content with no source).
    fn still_current(&self) -> bool {
        match std::fs::metadata(&self.path) {
            Ok(meta) => {
                meta.len() == self.size
                    && (self.mtime.is_none() || meta.modified().ok() == self.mtime)
            }
            Err(_) => false,
        }
    }
}

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
    /// URL/path being loaded
    #[allow(dead_code)] // Used for logging in cleanup_completed_loads
    pub path: String,
    /// Source-file identity at load start, re-checked at promotion (D51). `None`
    /// for sources with no stable on-disk identity (a live network download).
    pub generation: Option<LoadGeneration>,
    pub pending_disk_write: Option<PendingDiskWrite>,
}

/// Unified cache manager coordinating memory and disk caches
pub struct CacheManager {
    memory_cache: MemoryCache,
    /// None when the disk cache is disabled in config
    disk_cache: Option<DiskCache>,
    resampler_quality: ResamplerQuality,
    /// When each URL was last checked against its server (in-memory, so a
    /// dead server is asked at most once per interval rather than per play)
    last_revalidation_attempt: HashMap<String, std::time::Instant>,
    /// Currently active streaming loads (path -> ActiveLoad)
    active_loads: HashMap<String, ActiveLoad>,
    /// Local-path allowlist; empty = allow-all (open by default).
    allowed_directories: Vec<String>,
    /// Revalidate a disk-cached HTTP entry once it is older than this many seconds.
    revalidate_after_seconds: u64,
    /// mtime + size recorded when a local file was decoded, so a later load can cheaply
    /// detect an edit and re-decode (path -> (mtime, size)).
    local_stats: HashMap<String, (std::time::SystemTime, u64)>,
    /// Bounded cache of local-file header probes keyed by path, valid for one
    /// (mtime, size) identity (D54). Windowed plays never enter the memory cache,
    /// so every replay of a large file would otherwise re-pay the header
    /// open+parse on the play path.
    probe_cache: HashMap<String, CachedProbe>,
    /// Insertion order for the probe cache's FIFO bound.
    probe_cache_order: std::collections::VecDeque<String>,
}

/// A header probe plus the file identity it was taken from (D54).
#[derive(Clone)]
struct CachedProbe {
    mtime: Option<std::time::SystemTime>,
    size: u64,
    probe: strategy::Probe,
}

/// The probe cache keeps at most this many entries (FIFO eviction). Each entry is
/// a few dozen bytes; the bound only guards against unbounded path churn.
const PROBE_CACHE_CAP: usize = 256;

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
        // Legacy/test constructor: 0 keeps the old "unlimited" meaning here. The
        // production path uses `with_resolved_cap` with a config-resolved cap, where
        // `max_memory_mb: 0` instead means auto-detect a bounded cap.
        let cap = if max_memory_mb == 0 {
            MemoryCap::Unlimited
        } else {
            MemoryCap::Bytes((max_memory_mb as usize) * 1024 * 1024)
        };
        Self::with_resolved_cap(
            cache_dir,
            resampler_quality,
            cap,
            allowed_directories,
            revalidate_after_seconds,
        )
    }

    /// Create a cache manager with an already-resolved [`MemoryCap`] (the production
    /// path; `main` resolves the cap from config + system memory at startup).
    pub fn with_resolved_cap(
        cache_dir: PathBuf,
        resampler_quality: ResamplerQuality,
        memory_cap: MemoryCap,
        allowed_directories: Vec<String>,
        revalidate_after_seconds: u64,
    ) -> Result<Self, CacheError> {
        Self::with_disk_cache(
            cache_dir,
            resampler_quality,
            memory_cap,
            allowed_directories,
            revalidate_after_seconds,
            true,
        )
    }

    pub fn with_disk_cache(
        cache_dir: PathBuf,
        resampler_quality: ResamplerQuality,
        memory_cap: MemoryCap,
        allowed_directories: Vec<String>,
        revalidate_after_seconds: u64,
        disk_enabled: bool,
    ) -> Result<Self, CacheError> {
        let disk_cache = if disk_enabled {
            Some(DiskCache::new(cache_dir)?)
        } else {
            None
        };
        let memory_cache = MemoryCache::with_cap(memory_cap);

        tracing::info!(
            "Cache manager initialized (memory cap: {})",
            match memory_cap {
                MemoryCap::Unlimited => "unlimited".to_string(),
                MemoryCap::Bytes(n) => format!("{:.0} MB", n as f64 / (1024.0 * 1024.0)),
            }
        );

        Ok(Self {
            memory_cache,
            disk_cache,
            resampler_quality,
            last_revalidation_attempt: HashMap::new(),
            active_loads: HashMap::new(),
            allowed_directories,
            revalidate_after_seconds,
            local_stats: HashMap::new(),
            probe_cache: HashMap::new(),
            probe_cache_order: std::collections::VecDeque::new(),
        })
    }

    /// Record the current mtime + size of a local file after decoding it, so a later
    /// load can cheaply detect an edit. Best-effort: a stat failure records nothing.
    fn record_local_stat(&mut self, path: &str) {
        if let Ok(meta) = std::fs::metadata(path) {
            if let Ok(mtime) = meta.modified() {
                self.local_stats
                    .insert(path.to_string(), (mtime, meta.len()));
            }
        }
    }

    /// Whether a local file changed on disk since it was cached (mtime or size differs
    /// from what was recorded at decode). A missing recorded stat or an unreadable file
    /// reads as "unchanged", so a transient stat error never thrashes the cache.
    fn local_file_changed(&self, path: &str) -> bool {
        let Some((cached_mtime, cached_size)) = self.local_stats.get(path) else {
            return false;
        };
        match std::fs::metadata(path) {
            Ok(meta) => {
                let size_changed = meta.len() != *cached_size;
                let mtime_changed = meta.modified().map(|m| m != *cached_mtime).unwrap_or(false);
                size_changed || mtime_changed
            }
            Err(_) => false,
        }
    }

    /// Bytes of new decoded data the memory cache could accept right now (after
    /// evicting evictable entries). The load-strategy decision force-windows an asset
    /// whose estimated decoded size exceeds this.
    pub fn memory_headroom(&self) -> usize {
        self.memory_cache.fit_headroom()
    }

    /// The resolved hard memory cap in bytes, or `None` if the budget is unlimited.
    /// `memory_headroom` is the portion of this cap still free right now.
    pub fn memory_cap(&self) -> Option<usize> {
        self.memory_cache.max_size_bytes()
    }

    /// Whether `file_path` is already resident in the memory cache, so a replay can
    /// skip the probe/strategy decision and serve it directly. Does not touch LRU.
    pub fn is_resident(&self, file_path: &str) -> bool {
        self.memory_cache.contains(file_path)
    }

    /// Whether `file_path` is cached in memory or on disk. A cached URL is served
    /// full-featured from the cache rather than windowed.
    pub fn is_cached(&self, file_path: &str) -> bool {
        self.memory_cache.contains(file_path)
            || self
                .disk_cache
                .as_ref()
                .is_some_and(|disk| disk.is_cached(file_path))
    }

    /// Temp + final disk paths for teeing a cacheable windowed download (see
    /// [`DiskCache::windowed_persist_paths`]).
    pub fn windowed_persist_paths(&self, url: &str) -> Option<(PathBuf, PathBuf)> {
        self.disk_cache
            .as_ref()
            .map(|disk| disk.windowed_persist_paths(url))
    }

    /// Register a windowed download that was teed to disk as a cache entry, so a later
    /// play of `url` hits disk with no extra request.
    pub fn record_streamed_download(
        &mut self,
        url: &str,
        file_size: u64,
        etag: Option<String>,
        last_modified: Option<String>,
        content_type: Option<String>,
    ) -> Result<(), CacheError> {
        if let Some(disk) = self.disk_cache.as_mut() {
            disk.record_streamed_download(url, file_size, etag, last_modified, content_type)
        } else {
            Ok(())
        }
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

    /// Get or load an audio file with streaming support, using the default (trusting)
    /// freshness. The convenience entry point for precache and tests; the Play path
    /// uses [`get_or_load_streaming_with_freshness`] to honour a per-play override.
    pub async fn get_or_load_streaming(
        &mut self,
        file_path: &str,
        target_sample_rate: u32,
    ) -> Result<SampleBuffer, Box<dyn std::error::Error + Send + Sync>> {
        self.get_or_load_streaming_with_freshness(
            file_path,
            target_sample_rate,
            FreshnessMode::Trusting,
        )
        .await
    }

    /// Get or load an audio file with streaming support, applying `freshness`.
    /// Returns a SampleBuffer that may be either complete (from cache) or streaming
    /// (still loading). A streaming buffer is returned immediately and playback begins
    /// right away; the audio callback emits silence for any frame that has not been
    /// decoded yet. In trusting/dev freshness a changed local file is re-decoded; in
    /// pinned the cached copy is served directly.
    pub async fn get_or_load_streaming_with_freshness(
        &mut self,
        file_path: &str,
        target_sample_rate: u32,
        freshness: FreshnessMode,
    ) -> Result<SampleBuffer, Box<dyn std::error::Error + Send + Sync>> {
        // Promote any finished streaming loads to the memory cache and drop their
        // active_loads entries first, so a replay of a now-complete URL is served
        // from cache instead of rejoining a stale streaming buffer.
        self.cleanup_completed_loads();

        // Check memory cache first - return as Complete. For a local file in
        // trusting/dev freshness, a cheap stat picks up an on-disk edit and re-decodes
        // it; pinned (and remote, here) serve the cached copy directly.
        if let Some(buffer) = self.memory_cache.get(file_path) {
            let is_http = file_path.starts_with("http://") || file_path.starts_with("https://");
            if !is_http && freshness != FreshnessMode::Pinned && self.local_file_changed(file_path)
            {
                tracing::info!("Local file changed on disk; reloading: {}", file_path);
                self.memory_cache.remove(file_path);
                self.local_stats.remove(file_path);
                // Fall through to re-decode below rather than return the stale buffer.
            } else {
                tracing::debug!("Memory cache hit for: {}", file_path);
                return Ok(SampleBuffer::Complete(buffer));
            }
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
            // Check disk cache first - if cached, decode the cached file
            // progressively (D51): playback starts at once, the decode fills the
            // buffer in the background, and promotion publishes it when done.
            if self
                .disk_cache
                .as_ref()
                .is_some_and(|disk| disk.is_cached(file_path))
            {
                tracing::debug!("Disk cache hit for: {}", file_path);
                let _ = self
                    .disk_cache
                    .as_mut()
                    .unwrap()
                    .revalidate_if_due(
                        file_path,
                        std::time::Duration::from_secs(self.revalidate_after_seconds),
                    )
                    .await;
                let entry = self
                    .disk_cache
                    .as_ref()
                    .unwrap()
                    .get_entry(file_path)
                    .unwrap();
                let local_path = self
                    .disk_cache
                    .as_ref()
                    .unwrap()
                    .get_cached_file_path(entry);
                return self
                    .start_file_streaming_decode(file_path, local_path, target_sample_rate)
                    .await;
            }

            // Start streaming download
            tracing::info!("Starting streaming load for: {}", file_path);
            return self
                .start_streaming_load(file_path, target_sample_rate)
                .await;
        }

        // Local file: decode progressively (D51) — return a streaming buffer at
        // once, fill it in the background, and let promotion publish it when done.
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
        // Remember the file's mtime+size BEFORE the decode (freshness): an edit
        // that lands mid-decode then mismatches this stat and the next play
        // re-decodes — the safe direction.
        self.record_local_stat(file_path);
        self.start_file_streaming_decode(file_path, PathBuf::from(file_path), target_sample_rate)
            .await
    }

    /// Start a progressive decode of the file at `source_path`, registered in
    /// `active_loads` under `key` (the play's path or URL — for a disk-cached HTTP
    /// entry the two differ). The decoder is constructed here (a header parse), so
    /// the returned buffer carries the correct channel count and total-frames
    /// estimate immediately; the packet decode then runs on a blocking task while
    /// playback proceeds (D51). The load's generation is captured for the
    /// promotion guard.
    async fn start_file_streaming_decode(
        &mut self,
        key: &str,
        source_path: PathBuf,
        target_sample_rate: u32,
    ) -> Result<SampleBuffer, Box<dyn std::error::Error + Send + Sync>> {
        let quality = self.resampler_quality;
        let open_path = source_path.clone();
        let decoder = tokio::task::spawn_blocking(move || {
            let file = std::fs::File::open(&open_path)
                .map_err(crate::audio::streaming_decoder::StreamingDecodeError::Io)?;
            let mut hint = Hint::new();
            if let Some(ext) = open_path.extension().and_then(|e| e.to_str()) {
                hint.with_extension(ext);
            }
            StreamingDecoder::new(file, Some(&hint), Some(target_sample_rate), quality)
        })
        .await?
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        let channels = decoder.channels().max(1);
        let estimated = decoder.estimated_frames().map(|f| f as usize);
        let streaming_buffer = Arc::new(RwLock::new(StreamingBuffer::new(
            channels,
            target_sample_rate,
            estimated,
        )));

        self.active_loads.insert(
            key.to_string(),
            ActiveLoad {
                buffer: Arc::clone(&streaming_buffer),
                path: key.to_string(),
                generation: LoadGeneration::capture(&source_path),
                pending_disk_write: None,
            },
        );

        tracing::info!("Starting progressive load for: {}", key);
        let pump_buffer = Arc::clone(&streaming_buffer);
        let label = key.to_string();
        tokio::task::spawn_blocking(move || {
            Self::pump_streaming_decode(decoder, target_sample_rate, pump_buffer, label);
        });

        Ok(SampleBuffer::Streaming(streaming_buffer))
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
        self.last_revalidation_attempt
            .insert(url.to_string(), std::time::Instant::now());

        let disk = self
            .disk_cache
            .as_mut()
            .expect("revalidation_due checked the disk cache");
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
            (
                cache_filename,
                http_stream::BufferedPersistTarget {
                    temp_path,
                    final_path,
                },
            )
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
            path: url.to_string(),
            generation: None,
            pending_disk_write,
        };
        self.active_loads.insert(url.to_string(), active_load);

        // Create hint for format detection from the URL path (query strings
        // and non-audio suffixes would mislead the probe)
        let mut hint = Hint::new();
        if let Some(ext) = url_path
            .rsplit('/')
            .next()
            .and_then(|f| f.rsplit('.').next())
        {
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

    /// Start a progressive full-load decode over an already-open HTTP body (D55):
    /// the windowing probe's response is reused, so a small uncached HTTP asset
    /// costs ONE request instead of a probe GET plus a load GET. Registered as an
    /// active load keyed by `url`, so completion promotes to the memory cache
    /// exactly like any other streaming load.
    pub fn start_streaming_load_from_reader<R>(
        &mut self,
        url: &str,
        reader: R,
        target_sample_rate: u32,
        estimated_frames: Option<usize>,
    ) -> SampleBuffer
    where
        R: symphonia::core::io::MediaSource + Send + 'static,
    {
        let streaming_buffer = Arc::new(RwLock::new(StreamingBuffer::new(
            2, // Default, corrected by the decoder once the header is parsed
            target_sample_rate,
            estimated_frames,
        )));

        self.active_loads.insert(
            url.to_string(),
            ActiveLoad {
                buffer: Arc::clone(&streaming_buffer),
                path: url.to_string(),
                generation: None,
                pending_disk_write: None,
            },
        );

        let mut hint = Hint::new();
        let path_part = url.split(['?', '#']).next().unwrap_or(url);
        if let Some(ext) = std::path::Path::new(path_part)
            .extension()
            .and_then(|e| e.to_str())
        {
            hint.with_extension(ext);
        }

        tracing::info!(
            "Starting full-load streaming decode (reused request) for: {}",
            url
        );
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

        SampleBuffer::Streaming(streaming_buffer)
    }

    /// Background decode task - runs in spawn_blocking
    fn decode_streaming<R: symphonia::core::io::MediaSource + 'static>(
        reader: R,
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

        // Update buffer with the actual channel count and, when the header knows
        // it, a real total-frames estimate (better than the Content-Length guess).
        let channels = decoder.channels();
        if let Ok(mut guard) = buffer.write() {
            guard.channels = channels;
            if let Some(est) = decoder.estimated_frames() {
                guard.total_frames = Some(est as usize);
            }
        }

        tracing::debug!(
            "Streaming decode started: {} channels, {} Hz",
            channels,
            target_sample_rate
        );

        Self::pump_streaming_decode(decoder, target_sample_rate, buffer, url);
    }

    /// Drain a constructed decoder into `buffer` chunk by chunk, marking it
    /// complete (or errored) at the end. The shared tail of every progressive
    /// load — HTTP downloads and on-disk files alike (D51). Runs on a blocking
    /// task.
    fn pump_streaming_decode(
        decoder: StreamingDecoder,
        target_sample_rate: u32,
        buffer: Arc<RwLock<StreamingBuffer>>,
        label: String,
    ) {
        let channels = decoder.channels();

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
                    tracing::error!("Decode error for {}: {}", label, e);
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
            label,
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

    /// Clean up completed loads and promote to memory cache. Errored loads are
    /// removed too (without promotion), so a later play retries fresh instead of
    /// joining a dead buffer.
    pub fn cleanup_completed_loads(&mut self) {
        let completed: Vec<String> = self
            .active_loads
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

        for path in completed {
            if let Some(active) = self.active_loads.remove(&path) {
                // An errored load is dropped without promotion; removing it lets
                // the next play of this key start fresh.
                if let Ok(guard) = active.buffer.read() {
                    if guard.has_error() {
                        tracing::warn!("Dropping errored streaming load for {}", path);
                        continue;
                    }
                }
                // Generation guard (D51): never publish a load whose source file
                // changed underneath it — the next play decodes fresh instead.
                if let Some(generation) = &active.generation {
                    if !generation.still_current() {
                        tracing::info!(
                            "Skipping stale promotion for {} (source changed during load)",
                            path
                        );
                        continue;
                    }
                }
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
                        if let (Some(pending), Some(disk)) =
                            (&active.pending_disk_write, self.disk_cache.as_mut())
                        {
                            if let Ok(meta) = std::fs::metadata(&pending.final_path) {
                                disk.put_entry(
                                    path.clone(),
                                    disk::CacheEntry {
                                        local_file: pending.cache_filename.clone(),
                                        etag: pending.etag.clone(),
                                        last_modified: pending.last_modified.clone(),
                                        last_validated: chrono::Utc::now().to_rfc3339(),
                                        file_size: meta.len(),
                                        content_type: pending.content_type.clone(),
                                    },
                                );
                                if let Err(e) = disk.save_metadata() {
                                    tracing::warn!("Failed to save cache metadata: {}", e);
                                }
                            }
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
                let buffer = self
                    .get_or_load_streaming(file_path, target_sample_rate)
                    .await
                    .map_err(|e| -> Box<dyn std::error::Error> { e })?;
                if let Some(decoded) = buffer.as_complete() {
                    return Ok(decoded);
                }
                return self.wait_for_streaming(file_path, buffer).await;
            } else if self.disk_cache.as_ref().unwrap().is_cached(file_path) {
                tracing::debug!("Disk cache hit for: {}", file_path);
                let _ = self
                    .disk_cache
                    .as_mut()
                    .unwrap()
                    .revalidate_if_due(
                        file_path,
                        std::time::Duration::from_secs(self.revalidate_after_seconds),
                    )
                    .await;
                let entry = self
                    .disk_cache
                    .as_ref()
                    .unwrap()
                    .get_entry(file_path)
                    .unwrap();
                self.disk_cache
                    .as_ref()
                    .unwrap()
                    .get_cached_file_path(entry)
            } else {
                // Download and cache
                tracing::info!("Cache miss, downloading: {}", file_path);
                let path = self
                    .disk_cache
                    .as_mut()
                    .unwrap()
                    .download_and_cache(file_path)
                    .await?;
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

    /// Invalidate a specific file from both caches. An in-flight streaming load of
    /// the same key is abandoned (D52): it is removed from `active_loads`, so its
    /// completion never promotes the now-stale content and no new play joins it —
    /// any voice already playing the old buffer keeps its own Arc and finishes
    /// normally.
    pub fn invalidate(&mut self, file_path: &str) -> Result<(), CacheError> {
        self.memory_cache.remove(file_path);
        self.local_stats.remove(file_path);
        if self.active_loads.remove(file_path).is_some() {
            tracing::info!("Abandoned in-flight streaming load for: {}", file_path);
        }
        if let Some(disk) = self.disk_cache.as_mut() {
            disk.remove_entry(file_path)?;
        }
        self.last_revalidation_attempt.remove(file_path);
        tracing::info!("Invalidated cache for: {}", file_path);
        Ok(())
    }

    /// Revalidate every disk-cached HTTP entry that is also resident in memory and past
    /// the freshness `window`, dropping the decoded copy of any that changed so the next
    /// play re-decodes from the now-fresh disk file (no network on the play). Run
    /// out-of-band by the freshness tick, so a play never blocks on the network — the
    /// stale-while-revalidate fix for the warm-memory-hit short-circuit. Returns how
    /// many entries were refreshed.
    pub async fn revalidate_stale_http(&mut self, window: std::time::Duration) -> usize {
        let Some(disk) = self.disk_cache.as_mut() else {
            return 0;
        };
        let urls: Vec<String> = disk
            .cached_urls()
            .into_iter()
            .filter(|u| self.memory_cache.contains(u))
            .collect();
        let mut refreshed = 0;
        for url in urls {
            match disk.revalidate_if_due(&url, window).await {
                Ok(true) => {
                    // Content changed: drop the stale decoded buffer (the playing
                    // sample keeps its own Arc; the next play re-decodes fresh).
                    self.memory_cache.remove(&url);
                    refreshed += 1;
                    tracing::info!("Refreshed stale HTTP cache entry: {}", url);
                }
                Ok(false) => {}
                Err(e) => tracing::warn!("Revalidation error for {}: {}", url, e),
            }
        }
        refreshed
    }

    /// Flush the disk-cache metadata to disk (called on graceful shutdown).
    pub fn flush_metadata(&self) -> Result<(), CacheError> {
        if let Some(disk) = self.disk_cache.as_ref() {
            disk.save_metadata()
        } else {
            Ok(())
        }
    }

    /// Whether `key` is resident in the memory cache (status/test introspection).
    /// Exercised by the lib's integration tests; the binary reads the cache only
    /// through `get_cached`, so it is dead in the bin target.
    #[allow(dead_code)]
    pub fn is_in_memory_cache(&self, key: &str) -> bool {
        self.memory_cache.contains(key)
    }

    /// Fetch a memory-cached decoded buffer (refreshing its LRU slot) without
    /// triggering any load. The reaper's buffer-upgrade pass (D51) uses this to
    /// hand a finished cold play its promoted Complete buffer.
    pub fn get_cached(&mut self, key: &str) -> Option<Arc<DecodedBuffer>> {
        self.memory_cache.get(key)
    }

    /// Look up a cached header probe for `path`, valid only while the file's
    /// mtime+size match the identity it was taken from (D54). One stat syscall;
    /// any edit to the file misses naturally.
    pub fn cached_probe(&self, path: &str) -> Option<strategy::Probe> {
        let cached = self.probe_cache.get(path)?;
        let meta = std::fs::metadata(path).ok()?;
        if meta.len() == cached.size && meta.modified().ok() == cached.mtime {
            Some(cached.probe)
        } else {
            None
        }
    }

    /// Store a header probe for `path` under its current mtime+size identity
    /// (D54), bounded by FIFO eviction.
    pub fn store_probe(&mut self, path: &str, probe: strategy::Probe) {
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        if !self.probe_cache.contains_key(path) {
            if self.probe_cache_order.len() >= PROBE_CACHE_CAP {
                if let Some(evicted) = self.probe_cache_order.pop_front() {
                    self.probe_cache.remove(&evicted);
                }
            }
            self.probe_cache_order.push_back(path.to_string());
        }
        self.probe_cache.insert(
            path.to_string(),
            CachedProbe {
                mtime: meta.modified().ok(),
                size: meta.len(),
                probe,
            },
        );
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
    fn memory_cap_reports_the_resolved_budget() {
        let temp_dir = TempDir::new().unwrap();
        let bounded = CacheManager::with_resolved_cap(
            temp_dir.path().to_path_buf(),
            ResamplerQuality::Fast,
            MemoryCap::Bytes(64 * 1024 * 1024),
            Vec::new(),
            300,
        )
        .unwrap();
        assert_eq!(bounded.memory_cap(), Some(64 * 1024 * 1024));

        let unlimited = CacheManager::with_resolved_cap(
            temp_dir.path().to_path_buf(),
            ResamplerQuality::Fast,
            MemoryCap::Unlimited,
            Vec::new(),
            300,
        )
        .unwrap();
        assert_eq!(unlimited.memory_cap(), None);
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
                generation: None,
                pending_disk_write: None,
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

    /// Two test WAVs of clearly different length, used to detect a re-decode.
    const SHORT_WAV: &str = "tests/audio/test_440hz_2s.wav";
    const LONG_WAV: &str = "tests/audio/test_beep_5s.wav";

    fn wavs_present() -> bool {
        std::path::Path::new(SHORT_WAV).exists() && std::path::Path::new(LONG_WAV).exists()
    }

    #[tokio::test]
    async fn errored_streaming_load_is_dropped_and_not_promoted() {
        // An errored load must not linger in active_loads (a later play would
        // join the dead buffer forever) and must never promote.
        let cache_dir = TempDir::new().unwrap();
        let mut cm =
            CacheManager::with_quality(cache_dir.path().to_path_buf(), ResamplerQuality::Fast)
                .unwrap();
        let url = "http://example.com/broken.wav";

        let mut sb = StreamingBuffer::new(2, 48000, None);
        sb.mark_error("decoder blew up".to_string());
        cm.active_loads.insert(
            url.to_string(),
            ActiveLoad {
                buffer: Arc::new(RwLock::new(sb)),
                path: url.to_string(),
                generation: None,
                pending_disk_write: None,
            },
        );

        cm.cleanup_completed_loads();
        assert!(
            cm.active_loads.is_empty(),
            "an errored load must be removed so a replay starts fresh"
        );
        assert!(
            !cm.is_in_memory_cache(url),
            "an errored load must never be promoted"
        );
    }

    #[tokio::test]
    async fn probe_cache_hits_until_the_file_changes() {
        // D54: a stored probe serves repeat plays without re-opening the file's
        // header, and any change to the file (size/mtime) invalidates it.
        if !wavs_present() {
            eprintln!("skipping: test WAVs not found");
            return;
        }
        let work = TempDir::new().unwrap();
        let asset = work.path().join("probe.wav");
        std::fs::copy(SHORT_WAV, &asset).unwrap();
        let asset_str = asset.to_string_lossy().into_owned();

        let cache_dir = TempDir::new().unwrap();
        let mut cm =
            CacheManager::with_quality(cache_dir.path().to_path_buf(), ResamplerQuality::Fast)
                .unwrap();

        assert!(
            cm.cached_probe(&asset_str).is_none(),
            "no probe cached before the first store"
        );
        let probe = strategy::probe_local_file(&asset_str, 48000, ResamplerQuality::Fast).unwrap();
        cm.store_probe(&asset_str, probe);
        assert_eq!(
            cm.cached_probe(&asset_str),
            Some(probe),
            "the stored probe is served while the file is unchanged"
        );

        // The file changes (different length => different size): the entry no
        // longer matches and the lookup misses.
        std::fs::copy(LONG_WAV, &asset).unwrap();
        assert!(
            cm.cached_probe(&asset_str).is_none(),
            "a changed file must invalidate its cached probe"
        );
    }

    /// Wait for a progressive load to finish (D51: local loads return Streaming
    /// and fill in the background).
    async fn wait_complete(buffer: &SampleBuffer) {
        let mut attempts = 0;
        while !buffer.is_complete() && attempts < 500 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            attempts += 1;
        }
        assert!(buffer.is_complete(), "background decode must finish");
    }

    #[tokio::test]
    async fn local_freshness_reloads_a_changed_file_in_trusting_but_not_pinned() {
        if !wavs_present() {
            eprintln!("skipping: test WAVs not found");
            return;
        }
        let work = TempDir::new().unwrap();
        let asset = work.path().join("asset.wav");
        std::fs::copy(SHORT_WAV, &asset).unwrap();
        let asset_str = asset.to_string_lossy().into_owned();

        let cache_dir = TempDir::new().unwrap();
        let mut cm =
            CacheManager::with_quality(cache_dir.path().to_path_buf(), ResamplerQuality::Fast)
                .unwrap();

        // First load: the ~2 s clip; its mtime+size are recorded. The load is
        // progressive (D51): wait for it and promote it to the memory cache.
        let b1 = cm
            .get_or_load_streaming_with_freshness(&asset_str, 48000, FreshnessMode::Trusting)
            .await
            .unwrap();
        wait_complete(&b1).await;
        cm.cleanup_completed_loads();
        let frames1 = b1.frames();
        assert!(
            (80_000..=110_000).contains(&frames1),
            "2 s ~= 96000 frames, got {frames1}"
        );

        // Replace the asset with the ~5 s clip (changes size and mtime).
        std::fs::copy(LONG_WAV, &asset).unwrap();

        // Pinned never re-checks: it serves the stale cached 2 s buffer.
        let bp = cm
            .get_or_load_streaming_with_freshness(&asset_str, 48000, FreshnessMode::Pinned)
            .await
            .unwrap();
        assert_eq!(
            bp.frames(),
            frames1,
            "pinned must serve the stale cached buffer"
        );

        // Trusting re-stats, sees the change, and reloads the longer clip.
        let b2 = cm
            .get_or_load_streaming_with_freshness(&asset_str, 48000, FreshnessMode::Trusting)
            .await
            .unwrap();
        wait_complete(&b2).await;
        assert!(
            b2.frames() > frames1 + 50_000,
            "trusting must reload the changed (longer) file, got {} vs {}",
            b2.frames(),
            frames1
        );
    }

    #[tokio::test]
    async fn unchanged_local_file_is_served_from_cache_not_redecoded() {
        if !wavs_present() {
            eprintln!("skipping: test WAVs not found");
            return;
        }
        let work = TempDir::new().unwrap();
        let asset = work.path().join("asset.wav");
        std::fs::copy(SHORT_WAV, &asset).unwrap();
        let asset_str = asset.to_string_lossy().into_owned();

        let cache_dir = TempDir::new().unwrap();
        let mut cm =
            CacheManager::with_quality(cache_dir.path().to_path_buf(), ResamplerQuality::Fast)
                .unwrap();

        // The first (progressive, D51) load completes and is promoted.
        let b1 = cm
            .get_or_load_streaming_with_freshness(&asset_str, 48000, FreshnessMode::Trusting)
            .await
            .unwrap();
        wait_complete(&b1).await;
        cm.cleanup_completed_loads();

        // No change on disk: subsequent loads serve the SAME cached Arc (not a
        // redecode).
        let b2 = cm
            .get_or_load_streaming_with_freshness(&asset_str, 48000, FreshnessMode::Trusting)
            .await
            .unwrap();
        let b3 = cm
            .get_or_load_streaming_with_freshness(&asset_str, 48000, FreshnessMode::Trusting)
            .await
            .unwrap();
        match (b2, b3) {
            (SampleBuffer::Complete(a), SampleBuffer::Complete(b)) => {
                assert!(
                    Arc::ptr_eq(&a, &b),
                    "an unchanged file must serve the same cached Arc"
                );
            }
            _ => panic!("expected Complete buffers"),
        }
    }

    #[tokio::test]
    async fn reload_via_invalidate_then_precache_picks_up_a_changed_file() {
        if !wavs_present() {
            eprintln!("skipping: test WAVs not found");
            return;
        }
        let work = TempDir::new().unwrap();
        let asset = work.path().join("asset.wav");
        std::fs::copy(SHORT_WAV, &asset).unwrap();
        let asset_str = asset.to_string_lossy().into_owned();

        let cache_dir = TempDir::new().unwrap();
        let mut cm =
            CacheManager::with_quality(cache_dir.path().to_path_buf(), ResamplerQuality::Fast)
                .unwrap();

        let b1 = cm.get_or_load_streaming(&asset_str, 48000).await.unwrap();
        wait_complete(&b1).await;
        let frames1 = b1.frames();

        // Republish a longer asset, then run the cache_reload mechanism (invalidate +
        // precache), as a content pipeline would after publishing.
        std::fs::copy(LONG_WAV, &asset).unwrap();
        cm.invalidate(&asset_str).unwrap();
        cm.precache_streaming(&asset_str, 48000).await.unwrap();

        let b2 = cm.get_or_load_streaming(&asset_str, 48000).await.unwrap();
        wait_complete(&b2).await;
        let frames2 = b2.frames();
        assert!(
            frames2 > frames1 + 50_000,
            "reload must serve the republished (longer) file, got {frames2} vs {frames1}"
        );
    }
}
