// ABOUTME: HTTP stream adapter that implements Symphonia's MediaSource trait.
// ABOUTME: Buffers downloaded bytes to support seeking within the buffered region.

use bytes::{Buf, Bytes};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use symphonia::core::io::MediaSource;
use tokio::sync::mpsc;

/// How many downloaded chunks may be in flight in a [`BoundedHttpReader`]'s channel
/// before the download task parks. This bounds the reader's resident memory to roughly
/// this many reqwest chunks (tens of KB each), independent of the file's total length.
const WINDOWED_CHUNK_CHANNEL_CAPACITY: usize = 16;

/// Error type for HTTP streaming operations
#[derive(Debug)]
pub enum HttpStreamError {
    /// HTTP request failed
    Request(String),
    /// I/O error during streaming
    Io(io::Error),
}

impl std::fmt::Display for HttpStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpStreamError::Request(msg) => write!(f, "HTTP request error: {}", msg),
            HttpStreamError::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl std::error::Error for HttpStreamError {}

impl From<io::Error> for HttpStreamError {
    fn from(err: io::Error) -> Self {
        HttpStreamError::Io(err)
    }
}

/// Shared state between the reader and background download task
struct SharedBuffer {
    /// Accumulated downloaded bytes
    data: Vec<u8>,
    /// Whether download is complete (successfully or with error)
    complete: bool,
    /// Error message if download failed
    error: Option<String>,
}

/// HTTP stream reader that implements MediaSource for Symphonia.
/// Downloads data in the background and buffers it for reading.
pub struct HttpStreamReader {
    /// Shared buffer with downloaded data
    buffer: Arc<Mutex<SharedBuffer>>,
    /// Condition variable to notify when new data arrives
    data_available: Arc<Condvar>,
    /// Current read position
    position: usize,
    /// Total content length (from Content-Length header, if known)
    content_length: Option<u64>,
    /// Atomic flag to track total bytes downloaded (for progress)
    bytes_downloaded: Arc<AtomicUsize>,
    /// Flag to signal cancellation to the download task
    cancelled: Arc<AtomicBool>,
}

impl HttpStreamReader {
    /// Create a new HTTP stream reader.
    /// This creates the reader synchronously; call `start_download` to begin downloading.
    pub fn new(content_length: Option<u64>) -> Self {
        Self {
            buffer: Arc::new(Mutex::new(SharedBuffer {
                data: Vec::new(),
                complete: false,
                error: None,
            })),
            data_available: Arc::new(Condvar::new()),
            position: 0,
            content_length,
            bytes_downloaded: Arc::new(AtomicUsize::new(0)),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get handles needed by the background download task
    pub fn download_handles(&self) -> DownloadHandles {
        DownloadHandles {
            buffer: Arc::clone(&self.buffer),
            data_available: Arc::clone(&self.data_available),
            bytes_downloaded: Arc::clone(&self.bytes_downloaded),
            cancelled: Arc::clone(&self.cancelled),
        }
    }

    /// Cancel the download. Exercised by the http_stream tests.
    #[allow(dead_code)]
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Check if the download is complete. Exercised by the http_stream tests.
    #[allow(dead_code)]
    pub fn is_complete(&self) -> bool {
        let guard = self.buffer.lock().unwrap();
        guard.complete
    }

    /// Get the number of bytes downloaded so far
    pub fn bytes_downloaded(&self) -> usize {
        self.bytes_downloaded.load(Ordering::Acquire)
    }

    /// Get download progress as (downloaded, total)
    pub fn progress(&self) -> (usize, Option<usize>) {
        (
            self.bytes_downloaded(),
            self.content_length.map(|l| l as usize),
        )
    }

    /// Wait for data to become available at current position
    fn wait_for_data(&self, min_bytes: usize) -> io::Result<()> {
        let mut guard = self.buffer.lock().unwrap();

        loop {
            let available = guard.data.len().saturating_sub(self.position);
            if available >= min_bytes {
                return Ok(());
            }

            // Check if download is complete
            if guard.complete {
                if let Some(ref err) = guard.error {
                    return Err(io::Error::other(err.clone()));
                }
                // No more data coming
                return Ok(());
            }

            // Wait for notification
            guard = self.data_available.wait(guard).unwrap();
        }
    }
}

impl Read for HttpStreamReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        // Wait for at least 1 byte (or EOF)
        self.wait_for_data(1)?;

        let guard = self.buffer.lock().unwrap();
        let available = guard.data.len().saturating_sub(self.position);

        if available == 0 {
            // EOF
            return Ok(0);
        }

        let to_read = buf.len().min(available);
        buf[..to_read].copy_from_slice(&guard.data[self.position..self.position + to_read]);
        drop(guard);

        self.position += to_read;
        Ok(to_read)
    }
}

impl Seek for HttpStreamReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(offset) => offset as i64,
            SeekFrom::End(offset) => {
                // Need to know the total length for SeekFrom::End
                let len = self.content_length.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::Unsupported,
                        "Cannot seek from end without content length",
                    )
                })? as i64;
                len + offset
            }
            SeekFrom::Current(offset) => self.position as i64 + offset,
        };

        if new_pos < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Seek to negative position",
            ));
        }

        let new_pos = new_pos as usize;

        // If seeking forward past current buffer, wait for data
        let buffered = self.buffer.lock().unwrap().data.len();
        if new_pos > buffered {
            drop(self.buffer.lock().unwrap());
            // Wait until enough data is downloaded or stream ends
            self.wait_for_data(new_pos.saturating_sub(self.position))?;

            // Check if we have enough data now
            let buffered_after = self.buffer.lock().unwrap().data.len();
            if new_pos > buffered_after {
                // Stream ended before we could reach target position
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Seek past end of stream",
                ));
            }
        }

        self.position = new_pos;
        Ok(new_pos as u64)
    }
}

impl MediaSource for HttpStreamReader {
    fn is_seekable(&self) -> bool {
        // We support seeking within the buffered region and waiting for forward seeks
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.content_length
    }
}

/// Handles passed to the background download task
pub struct DownloadHandles {
    buffer: Arc<Mutex<SharedBuffer>>,
    data_available: Arc<Condvar>,
    bytes_downloaded: Arc<AtomicUsize>,
    cancelled: Arc<AtomicBool>,
}

impl DownloadHandles {
    /// Append data to the buffer
    pub fn append(&self, data: Bytes) {
        let mut guard = self.buffer.lock().unwrap();
        let len = data.len();
        guard.data.extend_from_slice(&data);
        drop(guard);

        self.bytes_downloaded.fetch_add(len, Ordering::AcqRel);
        self.data_available.notify_all();
    }

    /// Mark download as complete (success)
    pub fn complete(&self) {
        let mut guard = self.buffer.lock().unwrap();
        guard.complete = true;
        drop(guard);
        self.data_available.notify_all();
    }

    /// Mark download as failed
    pub fn fail(&self, error: String) {
        let mut guard = self.buffer.lock().unwrap();
        guard.complete = true;
        guard.error = Some(error);
        drop(guard);
        self.data_available.notify_all();
    }

    /// Check if download was cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Start an HTTP download in the background and return a reader.
/// The reader implements MediaSource and can be used with StreamingDecoder.
pub async fn start_http_stream(url: &str) -> Result<HttpStreamReader, HttpStreamError> {
    use futures_util::StreamExt;

    let response = reqwest::get(url)
        .await
        .map_err(|e| HttpStreamError::Request(format!("Failed to connect: {}", e)))?;

    if !response.status().is_success() {
        return Err(HttpStreamError::Request(format!(
            "HTTP {} from {}",
            response.status(),
            url
        )));
    }

    let content_length = response.content_length();

    let reader = HttpStreamReader::new(content_length);
    let handles = reader.download_handles();

    // Get the byte stream
    let mut stream = response.bytes_stream();

    // Spawn background download task
    tokio::spawn(async move {
        while let Some(chunk_result) = stream.next().await {
            if handles.is_cancelled() {
                handles.fail("Download cancelled".to_string());
                return;
            }

            match chunk_result {
                Ok(chunk) => {
                    handles.append(chunk);
                }
                Err(e) => {
                    handles.fail(format!("Download error: {}", e));
                    return;
                }
            }
        }

        handles.complete();
    });

    Ok(reader)
}

/// An opened HTTP response whose headers (status, Content-Length) have been read but
/// whose body has not been downloaded yet. The caller decides from `content_length`
/// whether to window the play ([`into_bounded_reader`](OpenHttpStream::into_bounded_reader),
/// which starts the bounded download) or to fall back to a full load (drop this, which
/// closes the connection) — so a windowed play needs only a single request.
pub struct OpenHttpStream {
    response: reqwest::Response,
    content_length: Option<u64>,
}

/// Open an HTTP URL and read its response headers, without downloading the body. Fails
/// on a connection error or a non-success status.
pub async fn open_http_stream(url: &str) -> Result<OpenHttpStream, HttpStreamError> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| HttpStreamError::Request(format!("Failed to connect: {}", e)))?;
    if !response.status().is_success() {
        return Err(HttpStreamError::Request(format!(
            "HTTP {} from {}",
            response.status(),
            url
        )));
    }
    let content_length = response.content_length();
    Ok(OpenHttpStream {
        response,
        content_length,
    })
}

impl OpenHttpStream {
    /// The body length from the `Content-Length` header, if the server sent one.
    pub fn content_length(&self) -> Option<u64> {
        self.content_length
    }

    /// Begin downloading into a bounded, forward-only reader, spawning the background
    /// download task. Resident memory stays O(channel) — the download parks when the
    /// channel is full and the decoder drains it as it consumes.
    pub fn into_bounded_reader(self) -> BoundedHttpReader {
        use futures_util::StreamExt;
        let (tx, rx) = mpsc::channel::<Result<Bytes, String>>(WINDOWED_CHUNK_CHANNEL_CAPACITY);
        let content_length = self.content_length;
        let mut stream = self.response.bytes_stream();
        tokio::spawn(async move {
            while let Some(chunk_result) = stream.next().await {
                let is_err = chunk_result.is_err();
                let msg = chunk_result.map_err(|e| format!("Download error: {}", e));
                // `send` awaits when the channel is full: this back-pressure is what
                // bounds memory (the decoder drains the channel as it consumes).
                if tx.send(msg).await.is_err() {
                    return; // receiver gone (playback stopped): stop downloading
                }
                if is_err {
                    return;
                }
            }
            // Stream ended; dropping `tx` here signals EOF to the reader.
        });
        BoundedHttpReader {
            rx,
            current: Bytes::new(),
            position: 0,
            content_length,
            finished: false,
        }
    }
}

/// A forward-only, bounded-memory adapter over an HTTP download, for windowed streaming.
/// Unlike [`HttpStreamReader`] (which buffers the whole file in memory to allow seeking),
/// this holds only a few in-flight chunks: the background download `send`s chunks over a
/// bounded channel and parks when it is full, so resident memory is O(channel) regardless
/// of the file's length. It is **not** seekable — a windowed play is forward-only, and the
/// decoder's own probe buffer covers format sniffing (Symphonia does not seek a source
/// that reports `is_seekable() == false`). Forward skips are served by discarding bytes.
pub struct BoundedHttpReader {
    rx: mpsc::Receiver<Result<Bytes, String>>,
    current: Bytes,
    position: u64,
    content_length: Option<u64>,
    finished: bool,
}

impl BoundedHttpReader {
    /// Block until `current` holds bytes, or the stream ends (EOF) or errors.
    fn fill(&mut self) -> io::Result<()> {
        while self.current.is_empty() && !self.finished {
            match self.rx.blocking_recv() {
                Some(Ok(chunk)) => self.current = chunk,
                Some(Err(e)) => {
                    self.finished = true;
                    return Err(io::Error::other(e));
                }
                None => self.finished = true, // channel closed: EOF
            }
        }
        Ok(())
    }
}

impl Read for BoundedHttpReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.fill()?;
        if self.current.is_empty() {
            return Ok(0); // EOF
        }
        let n = buf.len().min(self.current.len());
        buf[..n].copy_from_slice(&self.current[..n]);
        self.current.advance(n);
        self.position += n as u64;
        Ok(n)
    }
}

impl Seek for BoundedHttpReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(p) => p,
            SeekFrom::Current(n) => {
                let t = self.position as i64 + n;
                if t < 0 {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, "negative seek"));
                }
                t as u64
            }
            SeekFrom::End(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "seek from end is not supported on a forward-only stream",
                ))
            }
        };
        if target < self.position {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "backward seek is not supported on a forward-only stream",
            ));
        }
        // Forward skip: discard bytes until the target (or EOF).
        let mut remaining = target - self.position;
        while remaining > 0 {
            self.fill()?;
            if self.current.is_empty() {
                break; // EOF before target
            }
            let n = remaining.min(self.current.len() as u64);
            self.current.advance(n as usize);
            self.position += n;
            remaining -= n;
        }
        Ok(self.position)
    }
}

impl MediaSource for BoundedHttpReader {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        self.content_length
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom};

    fn bounded_reader_from(chunks: &[&'static [u8]], len: Option<u64>) -> BoundedHttpReader {
        let (tx, rx) = mpsc::channel::<Result<Bytes, String>>(16);
        for c in chunks {
            tx.blocking_send(Ok(Bytes::from_static(c))).unwrap();
        }
        drop(tx); // closing the sender signals EOF
        BoundedHttpReader {
            rx,
            current: Bytes::new(),
            position: 0,
            content_length: len,
            finished: false,
        }
    }

    #[test]
    fn bounded_reader_serves_chunks_in_order_then_eof() {
        let mut reader = bounded_reader_from(&[b"hello ", b"world"], Some(11));
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"hello world");
        assert_eq!(reader.byte_len(), Some(11));
        assert!(
            !reader.is_seekable(),
            "windowed reader must be forward-only"
        );
        // A second read past EOF stays at EOF.
        let mut buf = [0u8; 4];
        assert_eq!(reader.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn bounded_reader_forward_seek_discards_and_rejects_backward() {
        let mut reader = bounded_reader_from(&[b"0123456789"], Some(10));
        assert_eq!(reader.seek(SeekFrom::Start(4)).unwrap(), 4);
        let mut buf = [0u8; 3];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"456");
        // Backward seeks are unsupported on a forward-only stream.
        assert!(reader.seek(SeekFrom::Start(0)).is_err());
        assert!(reader.seek(SeekFrom::End(-1)).is_err());
    }

    #[test]
    fn bounded_reader_surfaces_a_download_error() {
        let (tx, rx) = mpsc::channel::<Result<Bytes, String>>(16);
        tx.blocking_send(Ok(Bytes::from_static(b"partial")))
            .unwrap();
        tx.blocking_send(Err("connection reset".to_string()))
            .unwrap();
        drop(tx);
        let mut reader = BoundedHttpReader {
            rx,
            current: Bytes::new(),
            position: 0,
            content_length: None,
            finished: false,
        };
        let mut buf = [0u8; 7];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"partial");
        // The next read surfaces the download error rather than a silent short EOF.
        let mut more = [0u8; 4];
        assert!(reader.read(&mut more).is_err());
    }

    #[test]
    fn test_reader_basic_read() {
        let reader = HttpStreamReader::new(Some(10));
        let handles = reader.download_handles();

        // Add some data
        handles.append(Bytes::from_static(b"hello"));
        handles.complete();

        let mut reader = reader;
        let mut buf = [0u8; 3];
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 3);
        assert_eq!(&buf, b"hel");

        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 2);
        assert_eq!(&buf[..2], b"lo");
    }

    #[test]
    fn test_reader_read_to_eof() {
        let reader = HttpStreamReader::new(Some(5));
        let handles = reader.download_handles();

        handles.append(Bytes::from_static(b"hello"));
        handles.complete();

        let mut reader = reader;
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).unwrap();
        assert_eq!(&buf, b"hello");
    }

    #[test]
    fn test_reader_seek_within_buffer() {
        let reader = HttpStreamReader::new(Some(10));
        let handles = reader.download_handles();

        handles.append(Bytes::from_static(b"0123456789"));
        handles.complete();

        let mut reader = reader;

        // Seek to position 5
        let pos = reader.seek(SeekFrom::Start(5)).unwrap();
        assert_eq!(pos, 5);

        let mut buf = [0u8; 3];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"567");

        // Seek backward
        let pos = reader.seek(SeekFrom::Start(2)).unwrap();
        assert_eq!(pos, 2);

        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"234");
    }

    #[test]
    fn test_reader_seek_from_current() {
        let reader = HttpStreamReader::new(Some(10));
        let handles = reader.download_handles();

        handles.append(Bytes::from_static(b"0123456789"));
        handles.complete();

        let mut reader = reader;

        // Read 3 bytes
        let mut buf = [0u8; 3];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(reader.position, 3);

        // Seek forward 2 from current
        let pos = reader.seek(SeekFrom::Current(2)).unwrap();
        assert_eq!(pos, 5);

        // Seek backward 1 from current
        let pos = reader.seek(SeekFrom::Current(-1)).unwrap();
        assert_eq!(pos, 4);

        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"456");
    }

    #[test]
    fn test_reader_seek_from_end() {
        let reader = HttpStreamReader::new(Some(10));
        let handles = reader.download_handles();

        handles.append(Bytes::from_static(b"0123456789"));
        handles.complete();

        let mut reader = reader;

        // Seek to 3 bytes before end
        let pos = reader.seek(SeekFrom::End(-3)).unwrap();
        assert_eq!(pos, 7);

        let mut buf = [0u8; 3];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"789");
    }

    #[test]
    fn test_reader_progress() {
        let reader = HttpStreamReader::new(Some(100));
        let handles = reader.download_handles();

        assert_eq!(reader.progress(), (0, Some(100)));
        assert_eq!(reader.bytes_downloaded(), 0);

        handles.append(Bytes::from_static(b"hello"));
        assert_eq!(reader.bytes_downloaded(), 5);
        assert_eq!(reader.progress(), (5, Some(100)));

        handles.append(Bytes::from_static(b"world"));
        assert_eq!(reader.bytes_downloaded(), 10);
        assert_eq!(reader.progress(), (10, Some(100)));
    }

    #[test]
    fn test_reader_is_complete() {
        let reader = HttpStreamReader::new(Some(10));
        let handles = reader.download_handles();

        assert!(!reader.is_complete());

        handles.append(Bytes::from_static(b"hello"));
        assert!(!reader.is_complete());

        handles.complete();
        assert!(reader.is_complete());
    }

    #[test]
    fn test_reader_download_error() {
        let reader = HttpStreamReader::new(Some(100));
        let handles = reader.download_handles();

        handles.append(Bytes::from_static(b"partial"));
        handles.fail("Connection reset".to_string());

        let mut reader = reader;
        let mut buf = [0u8; 7];

        // Should be able to read buffered data
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 7);
        assert_eq!(&buf, b"partial");

        // Next read should fail
        let result = reader.read(&mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn test_reader_cancel() {
        let reader = HttpStreamReader::new(Some(100));
        let handles = reader.download_handles();

        assert!(!handles.is_cancelled());

        reader.cancel();

        assert!(handles.is_cancelled());
    }

    #[test]
    fn test_media_source_traits() {
        let reader = HttpStreamReader::new(Some(100));
        let handles = reader.download_handles();

        handles.append(Bytes::from_static(b"test data"));
        handles.complete();

        // Test MediaSource trait methods
        assert!(reader.is_seekable());
        assert_eq!(reader.byte_len(), Some(100));
    }

    #[test]
    fn test_reader_multiple_chunks() {
        let reader = HttpStreamReader::new(Some(15));
        let handles = reader.download_handles();

        // Simulate chunked transfer
        handles.append(Bytes::from_static(b"hello"));
        handles.append(Bytes::from_static(b" "));
        handles.append(Bytes::from_static(b"world"));
        handles.complete();

        let mut reader = reader;
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).unwrap();
        assert_eq!(&buf, b"hello world");
    }

    #[test]
    fn test_reader_seek_past_current_buffer_fails_when_complete() {
        let reader = HttpStreamReader::new(Some(100));
        let handles = reader.download_handles();

        handles.append(Bytes::from_static(b"short"));
        handles.complete();

        let mut reader = reader;

        // Try to seek past buffered data (5 bytes buffered, try to seek to 50)
        let result = reader.seek(SeekFrom::Start(50));
        assert!(result.is_err());
    }

    #[test]
    fn test_reader_no_content_length() {
        let reader = HttpStreamReader::new(None);
        let handles = reader.download_handles();

        handles.append(Bytes::from_static(b"unknown length"));
        handles.complete();

        assert_eq!(reader.byte_len(), None);
        assert_eq!(reader.progress(), (14, None));

        let mut reader = reader;

        // SeekFrom::End should fail without content length
        let result = reader.seek(SeekFrom::End(-3));
        assert!(result.is_err());

        // But SeekFrom::Start should work
        reader.seek(SeekFrom::Start(0)).unwrap();
    }

    #[test]
    fn test_reader_threaded_producer_consumer() {
        use std::thread;
        use std::time::Duration;

        let reader = HttpStreamReader::new(Some(15));
        let handles = reader.download_handles();

        // Producer thread simulates slow chunked download
        let producer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            handles.append(Bytes::from_static(b"hello"));
            thread::sleep(Duration::from_millis(10));
            handles.append(Bytes::from_static(b" "));
            thread::sleep(Duration::from_millis(10));
            handles.append(Bytes::from_static(b"world"));
            handles.complete();
        });

        // Consumer reads all data
        let mut reader = reader;
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).unwrap();

        producer.join().unwrap();
        assert_eq!(&buf, b"hello world");
    }

    #[test]
    fn test_reader_wait_for_forward_seek() {
        use std::thread;
        use std::time::Duration;

        let reader = HttpStreamReader::new(Some(20));
        let handles = reader.download_handles();

        // Start producer that adds data slowly
        let producer = thread::spawn(move || {
            handles.append(Bytes::from_static(b"01234"));
            thread::sleep(Duration::from_millis(50));
            handles.append(Bytes::from_static(b"56789"));
            thread::sleep(Duration::from_millis(50));
            handles.append(Bytes::from_static(b"abcde"));
            handles.complete();
        });

        let mut reader = reader;

        // Try to seek to position 10, which won't be available immediately
        // This should block until data arrives
        let pos = reader.seek(SeekFrom::Start(10)).unwrap();
        assert_eq!(pos, 10);

        let mut buf = [0u8; 5];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"abcde");

        producer.join().unwrap();
    }
}
