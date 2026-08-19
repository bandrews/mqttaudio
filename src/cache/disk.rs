// ABOUTME: Disk cache implementation for downloaded audio files.
// ABOUTME: Handles file storage, metadata, and staleness checking.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(test)]
use std::path::Path;
use std::fs;
use std::io;

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

impl Default for CacheMetadata {
    fn default() -> Self {
        Self {
            version: 1,
            entries: HashMap::new(),
        }
    }
}

/// Disk cache manager
pub struct DiskCache {
    cache_dir: PathBuf,
    metadata: CacheMetadata,
}

#[derive(Debug)]
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
                Ok(content) => {
                    match serde_json::from_str::<CacheMetadata>(&content) {
                        Ok(meta) => {
                            tracing::info!("Loaded cache metadata with {} entries", meta.entries.len());
                            meta
                        }
                        Err(e) => {
                            tracing::warn!("Failed to parse cache metadata: {}, starting fresh", e);
                            CacheMetadata::default()
                        }
                    }
                }
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
        })
    }

    /// Save metadata to disk
    pub fn save_metadata(&self) -> Result<(), CacheError> {
        let metadata_path = self.cache_dir.join("metadata.json");
        let json = serde_json::to_string_pretty(&self.metadata)?;
        fs::write(metadata_path, json)?;
        Ok(())
    }

    /// Generate a cache filename for a given URL
    /// Uses first 12 chars of SHA256 hash + extension
    pub fn cache_filename_for_url(url: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        url.hash(&mut hasher);
        let hash = hasher.finish();

        // Extract extension from URL
        let extension = if let Some(last_part) = url.split('/').last() {
            if let Some(ext) = last_part.split('.').last() {
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

        format!("{:016x}.{}", hash, extension)
    }

    /// Get the cache entry for a URL, if it exists
    pub fn get_entry(&self, url: &str) -> Option<&CacheEntry> {
        self.metadata.entries.get(url)
    }

    /// Get the full path to a cached file
    pub fn get_cached_file_path(&self, entry: &CacheEntry) -> PathBuf {
        self.cache_dir.join("files").join(&entry.local_file)
    }

    /// Check if a file is cached and the file actually exists
    pub fn is_cached(&self, url: &str) -> bool {
        if let Some(entry) = self.get_entry(url) {
            let path = self.get_cached_file_path(entry);
            path.exists()
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
        self.metadata.entries.values()
            .map(|e| e.file_size)
            .sum()
    }

    /// Download a file from HTTP/HTTPS URL and store in cache
    /// Returns the path to the cached file
    pub async fn download_and_cache(&mut self, url: &str) -> Result<PathBuf, CacheError> {
        tracing::info!("Downloading {}", url);

        // Make HTTP request, bounded so an unresponsive server errors instead
        // of stalling the caller indefinitely
        let response = super::http_stream::get_with_timeout(url).await
            .map_err(CacheError::HttpError)?;

        if !response.status().is_success() {
            return Err(CacheError::HttpError(format!(
                "HTTP {} from {}",
                response.status(),
                url
            )));
        }

        // Extract cache headers
        let etag = response.headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let last_modified = response.headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let content_type = response.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Download body
        let bytes = response.bytes().await
            .map_err(|e| CacheError::HttpError(format!("Failed to read response: {}", e)))?;

        let file_size = bytes.len() as u64;

        // Generate cache filename
        let cache_filename = Self::cache_filename_for_url(url);
        let cache_path = self.cache_dir.join("files").join(&cache_filename);

        // Write to disk
        fs::write(&cache_path, &bytes)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_cache_filename_generation() {
        let url1 = "http://example.com/sound.wav";
        let url2 = "http://example.com/sound.mp3";
        let url3 = "http://other.com/sound.wav";

        let name1 = DiskCache::cache_filename_for_url(url1);
        let name2 = DiskCache::cache_filename_for_url(url2);
        let name3 = DiskCache::cache_filename_for_url(url3);

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

        // Add entry but don't create file
        let entry = CacheEntry {
            local_file: "test123.wav".to_string(),
            etag: None,
            last_modified: None,
            last_validated: "2025-10-19T10:00:00Z".to_string(),
            file_size: 100,
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
