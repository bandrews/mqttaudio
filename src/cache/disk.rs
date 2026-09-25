// ABOUTME: Disk cache implementation for downloaded audio files.
// ABOUTME: Handles file storage, metadata, and staleness checking.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Metadata for a single cached file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Name of the cached file (hash-based)
    pub local_file: String,
    /// ETag from HTTP response (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// Last-Modified from HTTP response (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    /// Last time we validated this cache entry
    pub last_validated: String, // ISO 8601 timestamp
    /// File size in bytes
    pub file_size: u64,
    /// Content type from HTTP response (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

/// Cache metadata containing all cached entries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetadata {
    /// Version of the metadata format
    pub version: u32,
    /// Map of URL -> CacheEntry
    pub entries: HashMap<String, CacheEntry>,
}

/// Current on-disk metadata format version. Bumped when the cache-key hash or
/// entry layout changes so older entries are discarded rather than misused.
const CACHE_METADATA_VERSION: u32 = 2;

impl Default for CacheMetadata {
    fn default() -> Self {
        Self {
            version: CACHE_METADATA_VERSION,
            entries: HashMap::new(),
        }
    }
}

/// Disk cache manager
pub struct DiskCache {
    cache_dir: PathBuf,
    metadata: CacheMetadata,
    /// When each URL's most recent failed freshness check happened, so an
    /// unreachable server is retried once per revalidation window rather than on
    /// every check
    failed_revalidations: HashMap<String, std::time::Instant>,
}

/// How long a freshness check may wait for the server. Checks run while the
/// caller holds the cache, so a slow server costs at most this, once per
/// revalidation window.
const REVALIDATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug)]
#[allow(clippy::enum_variant_names)] // descriptive variant names; renaming deferred to Sprint 9
pub enum CacheError {
    IoError(io::Error),
    JsonError(serde_json::Error),
    HttpError(String),
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheError::IoError(e) => write!(f, "I/O error: {}", e),
            CacheError::JsonError(e) => write!(f, "JSON error: {}", e),
            CacheError::HttpError(msg) => write!(f, "HTTP error: {}", msg),
        }
    }
}

impl std::error::Error for CacheError {}

impl From<io::Error> for CacheError {
    fn from(err: io::Error) -> Self {
        CacheError::IoError(err)
    }
}

impl From<serde_json::Error> for CacheError {
    fn from(err: serde_json::Error) -> Self {
        CacheError::JsonError(err)
    }
}

impl DiskCache {
    /// Create or load a disk cache from the specified directory
    pub fn new(cache_dir: PathBuf) -> Result<Self, CacheError> {
        // Create cache directory if it doesn't exist
        if !cache_dir.exists() {
            fs::create_dir_all(&cache_dir)?;
        }

        // Create files subdirectory
        let files_dir = cache_dir.join("files");
        if !files_dir.exists() {
            fs::create_dir_all(&files_dir)?;
        }

        // Load or create metadata
        let metadata_path = cache_dir.join("metadata.json");
        let metadata = if metadata_path.exists() {
            match fs::read_to_string(&metadata_path) {
                Ok(content) => match serde_json::from_str::<CacheMetadata>(&content) {
                    Ok(meta) if meta.version == CACHE_METADATA_VERSION => {
                        tracing::info!("Loaded cache metadata with {} entries", meta.entries.len());
                        meta
                    }
                    Ok(meta) => {
                        tracing::info!(
                            "Cache metadata version {} != {}; discarding stale entries",
                            meta.version,
                            CACHE_METADATA_VERSION
                        );
                        CacheMetadata::default()
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse cache metadata: {}, starting fresh", e);
                        CacheMetadata::default()
                    }
                },
                Err(e) => {
                    tracing::warn!("Failed to read cache metadata: {}, starting fresh", e);
                    CacheMetadata::default()
                }
            }
        } else {
            tracing::info!("No cache metadata found, starting fresh");
            CacheMetadata::default()
        };

        Ok(Self {
            cache_dir,
            metadata,
            failed_revalidations: HashMap::new(),
        })
    }

    /// Save metadata to disk
    pub fn save_metadata(&self) -> Result<(), CacheError> {
        let metadata_path = self.cache_dir.join("metadata.json");
        let json = serde_json::to_string_pretty(&self.metadata)?;
        fs::write(metadata_path, json)?;
        Ok(())
    }

    /// Generate a cache filename for a given URL.
    /// Uses the first 6 bytes of the URL's SHA-256 (stable across Rust
    /// releases, unlike DefaultHasher) plus the URL's audio extension.
    /// The full URL including any query string is hashed - different query
    /// strings are different resources - but the extension is taken from the
    /// path alone so signed URLs keep their format hint.
    pub fn cache_filename_for_url(url: &str) -> String {
        use sha2::{Digest, Sha256};

        let digest = Sha256::digest(url.as_bytes());
        let hash: String = digest
            .iter()
            .take(6)
            .map(|b| format!("{:02x}", b))
            .collect();

        let path = url.split(['?', '#']).next().unwrap_or(url);
        let extension = if let Some(last_part) = path.split('/').next_back() {
            if let Some(ext) = last_part.split('.').next_back() {
                // Only use common audio extensions
                match ext.to_lowercase().as_str() {
                    "wav" | "mp3" | "ogg" | "flac" => ext.to_lowercase(),
                    _ => "dat".to_string(),
                }
            } else {
                "dat".to_string()
            }
        } else {
            "dat".to_string()
        };

        format!("{}.{}", hash, extension)
    }

    /// Get the cache entry for a URL, if it exists
    pub fn get_entry(&self, url: &str) -> Option<&CacheEntry> {
        self.metadata.entries.get(url)
    }

    /// Directory that holds the cached files themselves
    pub fn files_dir(&self) -> PathBuf {
        self.cache_dir.join("files")
    }

    /// Get the full path to a cached file
    pub fn get_cached_file_path(&self, entry: &CacheEntry) -> PathBuf {
        self.cache_dir.join("files").join(&entry.local_file)
    }

    /// Check if a file is cached, the file exists, and its on-disk length matches
    /// the recorded `file_size` (so a crash-truncated file is treated as missing
    /// and re-downloaded).
    pub fn is_cached(&self, url: &str) -> bool {
        if let Some(entry) = self.get_entry(url) {
            let path = self.get_cached_file_path(entry);
            match fs::metadata(&path) {
                Ok(meta) => meta.len() == entry.file_size,
                Err(_) => false,
            }
        } else {
            false
        }
    }

    /// Add or update a cache entry
    pub fn put_entry(&mut self, url: String, entry: CacheEntry) {
        self.metadata.entries.insert(url, entry);
    }

    /// Remove a cache entry and delete the associated file
    pub fn remove_entry(&mut self, url: &str) -> Result<(), CacheError> {
        if let Some(entry) = self.metadata.entries.remove(url) {
            let file_path = self.get_cached_file_path(&entry);
            if file_path.exists() {
                fs::remove_file(file_path)?;
            }
        }
        Ok(())
    }

    /// Clear all cache entries and delete all files
    pub fn clear_all(&mut self) -> Result<(), CacheError> {
        // Delete all cached files
        let files_dir = self.cache_dir.join("files");
        if files_dir.exists() {
            for entry in fs::read_dir(&files_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    fs::remove_file(entry.path())?;
                }
            }
        }

        // Clear metadata
        self.metadata.entries.clear();
        self.save_metadata()?;

        tracing::info!("Cleared all cache entries");
        Ok(())
    }

    /// Get cache directory path
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Get number of cached entries
    pub fn entry_count(&self) -> usize {
        self.metadata.entries.len()
    }

    /// Get total size of cached files in bytes
    pub fn total_size_bytes(&self) -> u64 {
        self.metadata.entries.values().map(|e| e.file_size).sum()
    }

    /// Download a file from HTTP/HTTPS URL and store in cache
    /// Returns the path to the cached file
    pub async fn download_and_cache(&mut self, url: &str) -> Result<PathBuf, CacheError> {
        tracing::info!("Downloading {}", url);

        // Make HTTP request, bounded so an unresponsive server errors instead
        // of stalling the caller indefinitely
        let response = super::http_stream::get_with_timeout(url)
            .await
            .map_err(CacheError::HttpError)?;

        if !response.status().is_success() {
            return Err(CacheError::HttpError(format!(
                "HTTP {} from {}",
                response.status(),
                url
            )));
        }

        // Extract cache headers
        let etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let last_modified = response
            .headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Generate cache filename
        let cache_filename = Self::cache_filename_for_url(url);
        let cache_path = self.cache_dir.join("files").join(&cache_filename);

        // Stream the body to disk (temp file + rename)
        let temp_path = cache_path.with_extension("part");
        let file_size = stream_body_to_file(response, &temp_path, &cache_path).await?;

        tracing::info!("Downloaded {} bytes to {}", file_size, cache_filename);

        // Create cache entry
        let now = chrono::Utc::now().to_rfc3339();
        let entry = CacheEntry {
            local_file: cache_filename,
            etag,
            last_modified,
            last_validated: now,
            file_size,
            content_type,
        };

        // Store metadata
        self.put_entry(url.to_string(), entry);
        self.save_metadata()?;

        Ok(cache_path)
    }

    /// Paths for teeing a cacheable windowed download to disk: a unique temp file (so
    /// concurrent downloads of the same URL never share a temp) and the final cache path
    /// it is atomically renamed to. The final path is what `is_cached`/`get_entry`
    /// resolve for this URL, so a later `record_streamed_download` makes it a cache hit.
    pub fn windowed_persist_paths(&self, url: &str) -> (PathBuf, PathBuf) {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let cache_filename = Self::cache_filename_for_url(url);
        let files = self.cache_dir.join("files");
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = files.join(format!("{}.{}.streaming.tmp", cache_filename, seq));
        let final_path = files.join(&cache_filename);
        (temp, final_path)
    }

    /// Register a windowed download that was teed to disk (the file is already at its
    /// final path) as a cache entry, so a later play hits disk with no extra request.
    pub fn record_streamed_download(
        &mut self,
        url: &str,
        file_size: u64,
        etag: Option<String>,
        last_modified: Option<String>,
        content_type: Option<String>,
    ) -> Result<(), CacheError> {
        let entry = CacheEntry {
            local_file: Self::cache_filename_for_url(url),
            etag,
            last_modified,
            last_validated: chrono::Utc::now().to_rfc3339(),
            file_size,
            content_type,
        };
        self.put_entry(url.to_string(), entry);
        self.save_metadata()
    }

    /// All URLs currently in the disk cache metadata.
    pub fn cached_urls(&self) -> Vec<String> {
        self.metadata.entries.keys().cloned().collect()
    }

    /// The freshness check `url` is due for, if any: the cached entry is older than
    /// `revalidate_after` and no failed check of it is more recent than that. A zero
    /// duration makes every cached entry due. The request carries what the check
    /// needs, so [`check_freshness`] can run without the disk cache.
    pub fn revalidation_request(
        &self,
        url: &str,
        revalidate_after: std::time::Duration,
    ) -> Option<RevalidationRequest> {
        let entry = self.get_entry(url)?;
        if let Ok(last) = chrono::DateTime::parse_from_rfc3339(&entry.last_validated) {
            let age = chrono::Utc::now().signed_duration_since(last.with_timezone(&chrono::Utc));
            if age.to_std().is_ok_and(|age| age < revalidate_after) {
                return None;
            }
        }
        if self
            .failed_revalidations
            .get(url)
            .is_some_and(|failed| failed.elapsed() < revalidate_after)
        {
            return None;
        }
        let (temp_path, cache_path) = self.windowed_persist_paths(url);
        Some(RevalidationRequest {
            url: url.to_string(),
            etag: entry.etag.clone(),
            last_modified: entry.last_modified.clone(),
            local_file: Self::cache_filename_for_url(url),
            cache_path,
            temp_path,
        })
    }

    /// Record what a freshness check found: a current copy restarts its age, a
    /// changed one (already saved in place by the check) takes the new validators,
    /// and a failure is not retried until the revalidation window passes. Returns
    /// whether the content changed.
    pub fn record_revalidation(
        &mut self,
        url: &str,
        result: &Revalidation,
    ) -> Result<bool, CacheError> {
        match result {
            Revalidation::Current => {
                self.failed_revalidations.remove(url);
                if let Some(entry) = self.metadata.entries.get_mut(url) {
                    entry.last_validated = chrono::Utc::now().to_rfc3339();
                }
                self.save_metadata()?;
                Ok(false)
            }
            Revalidation::Changed {
                local_file,
                etag,
                last_modified,
                content_type,
                file_size,
            } => {
                self.failed_revalidations.remove(url);
                self.put_entry(
                    url.to_string(),
                    CacheEntry {
                        local_file: local_file.clone(),
                        etag: etag.clone(),
                        last_modified: last_modified.clone(),
                        last_validated: chrono::Utc::now().to_rfc3339(),
                        file_size: *file_size,
                        content_type: content_type.clone(),
                    },
                );
                self.save_metadata()?;
                Ok(true)
            }
            Revalidation::Failed => {
                self.failed_revalidations
                    .insert(url.to_string(), std::time::Instant::now());
                Ok(false)
            }
        }
    }

    /// If the cached entry is due (see [`DiskCache::revalidation_request`]), ask the
    /// server whether it changed and record the answer; any error keeps the cached
    /// copy. No-op for URLs that are not cached. Returns whether the content
    /// changed, so the caller can drop a now-stale decoded copy from memory.
    pub async fn revalidate_if_due(
        &mut self,
        url: &str,
        revalidate_after: std::time::Duration,
    ) -> Result<bool, CacheError> {
        let Some(request) = self.revalidation_request(url, revalidate_after) else {
            return Ok(false);
        };
        let result = check_freshness(&request).await;
        self.record_revalidation(url, &result)
    }

    /// Start a streaming download from HTTP/HTTPS URL.
    /// Returns an HttpStreamReader that can be used immediately for decoding
    /// while the download continues in the background.
    ///
    /// Note: This does NOT cache to disk. Use download_and_cache() for caching.
    // Allow dead_code until Phase 10 connects streaming to main.rs
    #[allow(dead_code)]
    pub async fn start_streaming_download(
        url: &str,
    ) -> Result<super::http_stream::HttpStreamReader, CacheError> {
        super::http_stream::start_http_stream(url)
            .await
            .map_err(|e| CacheError::HttpError(format!("Streaming download failed: {}", e)))
    }
}

/// Stream a response body straight to a temp file and rename it into place,
/// so downloads never hold whole files in memory and a crash or stall can
/// never leave a partial file at the final path. Returns the byte count.
async fn stream_body_to_file(
    response: reqwest::Response,
    temp_path: &Path,
    final_path: &Path,
) -> Result<u64, CacheError> {
    use futures_util::StreamExt;
    use std::io::Write;

    let mut file = fs::File::create(temp_path)?;
    let mut stream = response.bytes_stream();
    let mut size: u64 = 0;

    loop {
        let chunk =
            match tokio::time::timeout(std::time::Duration::from_secs(60), stream.next()).await {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(e))) => {
                    let _ = fs::remove_file(temp_path);
                    return Err(CacheError::HttpError(format!(
                        "Failed to read response: {}",
                        e
                    )));
                }
                Ok(None) => break,
                Err(_) => {
                    let _ = fs::remove_file(temp_path);
                    return Err(CacheError::HttpError(
                        "Download stalled: no data for 60 seconds".to_string(),
                    ));
                }
            };

        if let Err(e) = file.write_all(&chunk) {
            let _ = fs::remove_file(temp_path);
            return Err(e.into());
        }
        size += chunk.len() as u64;
    }

    file.sync_all()?;
    drop(file);
    fs::rename(temp_path, final_path)?;
    Ok(size)
}

/// A due freshness check of one cached URL, carrying what the request needs so it
/// can run without holding the disk cache.
pub struct RevalidationRequest {
    url: String,
    etag: Option<String>,
    last_modified: Option<String>,
    local_file: String,
    cache_path: PathBuf,
    temp_path: PathBuf,
}

impl RevalidationRequest {
    /// The URL being checked.
    pub fn url(&self) -> &str {
        &self.url
    }
}

/// What a freshness check found.
pub enum Revalidation {
    /// `304 Not Modified`: the cached copy is current.
    Current,
    /// The server sent new content, already saved over the cached file.
    Changed {
        local_file: String,
        etag: Option<String>,
        last_modified: Option<String>,
        content_type: Option<String>,
        file_size: u64,
    },
    /// No usable answer; the cached copy stays in service.
    Failed,
}

/// Ask the server whether a cached URL changed, with a conditional request
/// (`If-None-Match` / `If-Modified-Since`) that waits at most 5 seconds for an
/// answer. A `200` answer's body is saved over the cached file (through a temp file
/// and a rename, so a play reading the old file keeps it). Holds nothing but the
/// request, so the caller can run it without the cache.
pub async fn check_freshness(request: &RevalidationRequest) -> Revalidation {
    let url = &request.url;
    let mut get = super::http_stream::http_client().get(url);
    if let Some(etag) = &request.etag {
        get = get.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    if let Some(last_modified) = &request.last_modified {
        get = get.header(reqwest::header::IF_MODIFIED_SINCE, last_modified);
    }
    let response = match tokio::time::timeout(REVALIDATION_TIMEOUT, get.send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => {
            tracing::warn!(
                "Revalidation failed for {}: {}; keeping cached copy",
                url,
                e
            );
            return Revalidation::Failed;
        }
        Err(_) => {
            tracing::warn!("Revalidation of {} timed out; keeping cached copy", url);
            return Revalidation::Failed;
        }
    };
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        tracing::debug!("Cache revalidated (304 Not Modified): {}", url);
        return Revalidation::Current;
    }
    if !response.status().is_success() {
        tracing::warn!(
            "Revalidation got HTTP {} for {}; keeping cached copy",
            response.status(),
            url
        );
        return Revalidation::Failed;
    }
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let (etag, last_modified, content_type) = (
        header("etag"),
        header("last-modified"),
        header("content-type"),
    );
    match stream_body_to_file(response, &request.temp_path, &request.cache_path).await {
        Ok(file_size) => {
            tracing::info!("Cache refreshed (content changed): {}", url);
            Revalidation::Changed {
                local_file: request.local_file.clone(),
                etag,
                last_modified,
                content_type,
                file_size,
            }
        }
        Err(e) => {
            tracing::warn!(
                "Downloading the new version of {} failed: {}; keeping cached copy",
                url,
                e
            );
            Revalidation::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_cache_filename_ignores_query_for_extension() {
        // A signed URL still names an mp3; the query string must not break
        // extension detection
        let name = DiskCache::cache_filename_for_url("http://example.com/sound.mp3?sig=abc.def");
        assert!(name.ends_with(".mp3"), "got: {}", name);

        // But different query strings are different resources
        let a = DiskCache::cache_filename_for_url("http://example.com/sound.mp3?v=1");
        let b = DiskCache::cache_filename_for_url("http://example.com/sound.mp3?v=2");
        assert_ne!(a, b);
    }

    #[test]
    fn test_cache_filename_is_stable_across_processes() {
        // The filename hash must not depend on the process or toolchain:
        // DefaultHasher is explicitly unstable across Rust releases, which
        // would orphan every cached file on a compiler upgrade. Pin the
        // expected SHA-256-derived name for a known URL.
        let name = DiskCache::cache_filename_for_url("http://example.com/sound.wav");
        let expected_prefix = {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest("http://example.com/sound.wav".as_bytes());
            hex_prefix(&digest)
        };
        assert_eq!(name, format!("{}.wav", expected_prefix));
    }

    fn hex_prefix(digest: &[u8]) -> String {
        digest
            .iter()
            .take(6)
            .map(|b| format!("{:02x}", b))
            .collect()
    }

    #[test]
    fn test_cache_filename_generation() {
        let url1 = "http://example.com/sound.wav";
        let url2 = "http://example.com/sound.mp3";
        let url3 = "http://other.com/sound.wav";

        let name1 = DiskCache::cache_filename_for_url(url1);
        let name2 = DiskCache::cache_filename_for_url(url2);
        let name3 = DiskCache::cache_filename_for_url(url3);

        // Pinned SHA-256 prefixes — stable across Rust versions and platforms.
        assert_eq!(name1, "5acc8be08cc8.wav");
        assert_eq!(name2, "4271e79bbe75.mp3");
        assert_eq!(name3, "4045c088839e.wav");

        // Different URLs should have different names
        assert_ne!(name1, name2);
        assert_ne!(name1, name3);

        // Extensions should be preserved
        assert!(name1.ends_with(".wav"));
        assert!(name2.ends_with(".mp3"));
        assert!(name3.ends_with(".wav"));

        // Same URL should always generate same name
        let name1_again = DiskCache::cache_filename_for_url(url1);
        assert_eq!(name1, name1_again);
    }

    #[test]
    fn test_disk_cache_creation() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("cache");

        let cache = DiskCache::new(cache_dir.clone()).unwrap();

        // Cache directory should be created
        assert!(cache_dir.exists());
        assert!(cache_dir.join("files").exists());

        // Should start with empty metadata
        assert_eq!(cache.entry_count(), 0);
    }

    #[test]
    fn test_add_and_get_entry() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("cache");
        let mut cache = DiskCache::new(cache_dir).unwrap();

        let url = "http://example.com/test.wav";
        let entry = CacheEntry {
            local_file: "abc123.wav".to_string(),
            etag: Some("\"etag123\"".to_string()),
            last_modified: None,
            last_validated: "2025-10-19T10:00:00Z".to_string(),
            file_size: 1024,
            content_type: Some("audio/wav".to_string()),
        };

        cache.put_entry(url.to_string(), entry.clone());

        // Should be able to retrieve entry
        let retrieved = cache.get_entry(url).unwrap();
        assert_eq!(retrieved.local_file, "abc123.wav");
        assert_eq!(retrieved.etag, Some("\"etag123\"".to_string()));
        assert_eq!(retrieved.file_size, 1024);
    }

    #[test]
    fn test_is_cached() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("cache");
        let mut cache = DiskCache::new(cache_dir.clone()).unwrap();

        let url = "http://example.com/test.wav";

        // Not cached initially
        assert!(!cache.is_cached(url));

        // Add entry but don't create file (file_size matches the bytes written below)
        let entry = CacheEntry {
            local_file: "test123.wav".to_string(),
            etag: None,
            last_modified: None,
            last_validated: "2025-10-19T10:00:00Z".to_string(),
            file_size: 9, // "test data"
            content_type: None,
        };
        cache.put_entry(url.to_string(), entry.clone());

        // Still not cached because file doesn't exist
        assert!(!cache.is_cached(url));

        // Create the actual file
        let file_path = cache.get_cached_file_path(&entry);
        fs::write(file_path, b"test data").unwrap();

        // Now it should be cached
        assert!(cache.is_cached(url));
    }

    #[test]
    fn test_is_cached_rejects_truncated_file() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("cache");
        let mut cache = DiskCache::new(cache_dir).unwrap();

        let url = "http://example.com/truncated.wav";
        let entry = CacheEntry {
            local_file: "trunc.wav".to_string(),
            etag: None,
            last_modified: None,
            last_validated: "2025-10-19T10:00:00Z".to_string(),
            file_size: 100,
            content_type: None,
        };
        cache.put_entry(url.to_string(), entry.clone());

        // A crash-truncated cache file: shorter than the recorded size.
        let file_path = cache.get_cached_file_path(&entry);
        fs::write(file_path, b"short").unwrap(); // 5 bytes, not 100

        // Treated as not cached so the caller re-downloads.
        assert!(!cache.is_cached(url));
    }

    #[test]
    fn test_remove_entry() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("cache");
        let mut cache = DiskCache::new(cache_dir.clone()).unwrap();

        let url = "http://example.com/test.wav";
        let entry = CacheEntry {
            local_file: "remove_test.wav".to_string(),
            etag: None,
            last_modified: None,
            last_validated: "2025-10-19T10:00:00Z".to_string(),
            file_size: 50,
            content_type: None,
        };

        // Add entry and create file
        cache.put_entry(url.to_string(), entry.clone());
        let file_path = cache.get_cached_file_path(&entry);
        fs::write(&file_path, b"data").unwrap();
        assert!(file_path.exists());

        // Remove entry
        cache.remove_entry(url).unwrap();

        // Entry and file should be gone
        assert!(cache.get_entry(url).is_none());
        assert!(!file_path.exists());
    }

    #[test]
    fn test_clear_all() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("cache");
        let mut cache = DiskCache::new(cache_dir.clone()).unwrap();

        // Add multiple entries with files
        for i in 0..3 {
            let url = format!("http://example.com/test{}.wav", i);
            let entry = CacheEntry {
                local_file: format!("file{}.wav", i),
                etag: None,
                last_modified: None,
                last_validated: "2025-10-19T10:00:00Z".to_string(),
                file_size: 100,
                content_type: None,
            };
            cache.put_entry(url, entry.clone());
            let file_path = cache.get_cached_file_path(&entry);
            fs::write(file_path, b"data").unwrap();
        }

        assert_eq!(cache.entry_count(), 3);

        // Clear all
        cache.clear_all().unwrap();

        // All entries should be gone
        assert_eq!(cache.entry_count(), 0);

        // All files should be deleted
        let files_dir = cache_dir.join("files");
        let file_count = fs::read_dir(&files_dir).unwrap().count();
        assert_eq!(file_count, 0);
    }

    #[test]
    fn test_metadata_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("cache");

        // Create cache and add entry
        {
            let mut cache = DiskCache::new(cache_dir.clone()).unwrap();
            let url = "http://example.com/persist.wav";
            let entry = CacheEntry {
                local_file: "persist123.wav".to_string(),
                etag: Some("\"persist_etag\"".to_string()),
                last_modified: Some("Wed, 21 Oct 2015 07:28:00 GMT".to_string()),
                last_validated: "2025-10-19T10:00:00Z".to_string(),
                file_size: 2048,
                content_type: Some("audio/wav".to_string()),
            };
            cache.put_entry(url.to_string(), entry);
            cache.save_metadata().unwrap();
        }

        // Load cache again
        {
            let cache = DiskCache::new(cache_dir).unwrap();
            assert_eq!(cache.entry_count(), 1);

            let url = "http://example.com/persist.wav";
            let entry = cache.get_entry(url).unwrap();
            assert_eq!(entry.local_file, "persist123.wav");
            assert_eq!(entry.etag, Some("\"persist_etag\"".to_string()));
            assert_eq!(entry.file_size, 2048);
        }
    }
}
