// ABOUTME: Prepares audio and cache commands away from the real-time control loop.
// ABOUTME: Preserves bounded streaming, cache freshness, and cancellation while loads wait on I/O.

use crate::mqtt::commands::{AudioCommand, CommandError, CommandErrorKind};
use crate::{audio, cache, config};
use std::sync::Arc;
use tokio::sync::Mutex;

type Cache = Arc<Mutex<cache::CacheManager>>;

pub enum PreparedPlayback {
    Full(audio::streaming::SampleBuffer),
    Stream(PreparedStream),
    Done,
}

/// A windowed play's producer, stopped if the play is abandoned before it starts.
pub struct PreparedStream {
    handles: Option<audio::streamed_source::StreamHandles>,
    loops: bool,
}
impl PreparedStream {
    /// Whether the stream can loop: one reading a file (local, or a URL's disk
    /// copy) can, one reading a download cannot.
    pub fn loops(&self) -> bool {
        self.loops
    }
    pub fn into_handles(mut self) -> audio::streamed_source::StreamHandles {
        self.handles
            .take()
            .expect("prepared stream owns its handles")
    }
}
impl Drop for PreparedStream {
    fn drop(&mut self) {
        if let Some(handles) = &self.handles {
            handles
                .stop_flag
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

fn authorize(config: &config::Config, file: &str) -> Result<(), CommandError> {
    config.is_local_path_allowed(file).map_err(|message| {
        let path = std::path::Path::new(file);
        let missing_allowed_file = !file.starts_with("http")
            && !path.exists()
            && path.parent().is_some_and(|parent| {
                config::Config::is_path_allowed(parent, &config.security.allowed_directories)
            });
        CommandError::new(
            if missing_allowed_file {
                CommandErrorKind::NotFound
            } else {
                CommandErrorKind::Forbidden
            },
            message,
        )
    })
}

pub fn needs_preparation(command: &AudioCommand) -> bool {
    matches!(
        command,
        AudioCommand::Play { .. }
            | AudioCommand::Precache { .. }
            | AudioCommand::CacheClear
            | AudioCommand::CacheInvalidate { .. }
            | AudioCommand::CacheReload { .. }
    )
}

fn error(e: impl std::fmt::Display) -> CommandError {
    CommandError::new(CommandErrorKind::NotFound, e.to_string())
}

async fn persistence(
    open: &cache::http_stream::OpenHttpStream,
    file: &str,
    enabled: Option<bool>,
    cache: &Cache,
) -> Option<cache::http_stream::PersistTarget> {
    if open.content_length().is_none() || enabled == Some(false) {
        return None;
    }
    let (temp_path, final_path) = cache.lock().await.windowed_persist_paths(file)?;
    let etag = open.etag().map(str::to_owned);
    let last_modified = open.last_modified().map(str::to_owned);
    let content_type = open.content_type().map(str::to_owned);
    let (done, receiver) = tokio::sync::oneshot::channel();
    let cache = cache.clone();
    let file = file.to_owned();
    tokio::spawn(async move {
        if let Ok(size) = receiver.await {
            if let Err(e) = cache.lock().await.record_streamed_download(
                &file,
                size,
                etag,
                last_modified,
                content_type,
            ) {
                tracing::warn!("Failed to register streamed download {}: {}", file, e);
            }
        }
    });
    Some(cache::http_stream::PersistTarget {
        temp_path,
        final_path,
        done,
    })
}

pub async fn prepare(
    command: &AudioCommand,
    config: &config::Config,
    cache: &Cache,
    rate: u32,
) -> Result<PreparedPlayback, CommandError> {
    use AudioCommand::*;
    match command {
        CacheClear => {
            cache.lock().await.clear_all().map_err(error)?;
            return Ok(PreparedPlayback::Done);
        }
        CacheInvalidate { file } => {
            cache.lock().await.invalidate(file).map_err(error)?;
            return Ok(PreparedPlayback::Done);
        }
        CacheReload { file } | Precache { file } => {
            authorize(config, file)?;
            if matches!(command, CacheReload { .. }) {
                cache.lock().await.invalidate(file).map_err(error)?;
            }
            precache(file, config, cache, rate).await?;
            return Ok(PreparedPlayback::Done);
        }
        _ => {}
    }
    let Play {
        file,
        channel_map,
        mode,
        window_ms,
        prebuffer_ms,
        loop_mode,
        freshness,
        cacheable,
        ..
    } = command
    else {
        return Err(CommandError::new(
            CommandErrorKind::InvalidRequest,
            "Command has no loading phase",
        ));
    };
    authorize(config, file)?;
    if let Some(routes) = channel_map {
        for route in routes {
            config
                .resolve_channel(&route.src)
                .and_then(|_| config.resolve_channel(&route.dest))
                .map_err(|e| CommandError::new(CommandErrorKind::InvalidRequest, e))?;
        }
    }
    let window_ms = window_ms.unwrap_or(config.cache.stream_window_ms);
    // The window holds all the audio buffered ahead, so a longer prebuffer could
    // never fill.
    let prebuffer_ms = prebuffer_ms
        .unwrap_or(config.cache.stream_prebuffer_ms)
        .min(window_ms);
    let window = Window {
        frames: (window_ms as usize * rate as usize / 1000).max(1),
        prebuffer_frames: prebuffer_ms as usize * rate as usize / 1000,
        deadline: std::time::Duration::from_millis(
            config.cache.stream_prebuffer_deadline_ms.max(prebuffer_ms) as u64,
        ),
        rate,
        quality: config.advanced.resampler_quality,
    };
    let freshness = freshness.unwrap_or(config.cache.freshness);
    let stream_asked = *mode == config::LoadMode::Stream;
    let full = |buffer| full_ready(buffer, command, config);

    // A decode already in memory or in progress is shared rather than repeated or
    // windowed, unless the play itself asks to stream.
    let source = examine(cache, file, freshness).await;
    if !stream_asked && (source.resident || source.loading) {
        let buffer = load_full(cache, file, rate, freshness).await?;
        return full(buffer).await;
    }

    // A local file, or a URL with a copy on disk, is probed and decided on its file.
    if let Some(path) = source.local_path {
        let windowed = stream_asked || {
            let probe = probe_file(cache, &path, &window).await?;
            let mut admission = cache.lock().await;
            admission.cleanup_completed_loads();
            if admission.is_resident(file) || admission.is_loading(file) {
                false
            } else {
                probe.is_some_and(|probe| {
                    decides_windowed(config, *mode, &probe, admission.memory_headroom())
                })
            }
        };
        if !windowed {
            let buffer = load_full(cache, file, rate, freshness).await?;
            return full(buffer).await;
        }
        let stream = stream_file(path, *loop_mode, &window).await?;
        return Ok(PreparedPlayback::Stream(stream));
    }

    // An uncached URL: its response decides, and is then decoded or streamed.
    let open = cache::http_stream::open_http_stream(file)
        .await
        .map_err(error)?;
    let length = open.content_length();
    let persist = persistence(&open, file, *cacheable, cache).await;
    let mut admission = cache.lock().await;
    admission.cleanup_completed_loads();
    if !stream_asked && (admission.is_loading(file) || admission.is_resident(file)) {
        drop(admission);
        let buffer = load_full(cache, file, rate, freshness).await?;
        return full(buffer).await;
    }
    let windowed = stream_asked
        || length.is_none()
        || decides_windowed(
            config,
            *mode,
            &cache::strategy::probe_http(length, file),
            admission.memory_headroom(),
        );
    let reader = open.into_bounded_reader(persist);
    if !windowed {
        let buffer = admission.start_streaming_load_from_reader(
            file,
            reader,
            rate,
            wav_frames(file, length),
        );
        drop(admission);
        return full(buffer).await;
    }
    drop(admission);
    let label = file.clone();
    let handles = tokio::task::spawn_blocking(move || {
        audio::streamed_source::spawn_stream_from_source(
            reader,
            label,
            window.rate,
            window.quality,
            window.frames,
        )
    })
    .await
    .map_err(error)?
    .map_err(error)?;
    let stream = PreparedStream {
        handles: Some(handles),
        loops: false,
    };
    Ok(PreparedPlayback::Stream(prebuffer(stream, &window).await))
}

/// How a windowed play buffers: its window and prebuffer in frames, how long it
/// waits for the prebuffer, and how it decodes.
struct Window {
    frames: usize,
    prebuffer_frames: usize,
    deadline: std::time::Duration,
    rate: u32,
    quality: config::ResamplerQuality,
}

/// What the caches hold for a file, after folding in finished loads and dropping a
/// local file's decode if the file changed on disk.
struct Source {
    resident: bool,
    loading: bool,
    /// The file to probe or stream from: the local file itself, or a URL's disk copy
    local_path: Option<std::path::PathBuf>,
}

async fn examine(cache: &Cache, file: &str, freshness: config::FreshnessMode) -> Source {
    let mut cache = cache.lock().await;
    cache.cleanup_completed_loads();
    let http = file.starts_with("http://") || file.starts_with("https://");
    if http {
        cache.revalidate_disk_if_due(file).await;
    } else {
        cache.drop_changed_local(file, freshness);
    }
    Source {
        resident: cache.is_resident(file),
        loading: cache.is_loading(file),
        local_path: if http {
            cache.disk_file(file)
        } else {
            Some(std::path::PathBuf::from(file))
        },
    }
}

/// Whether the load-strategy decision windows a file with this probe.
fn decides_windowed(
    config: &config::Config,
    mode: config::LoadMode,
    probe: &cache::strategy::Probe,
    headroom: usize,
) -> bool {
    cache::strategy::decide(
        mode,
        config.cache.load_mode,
        probe,
        config.cache.full_load_max_bytes,
        config.cache.full_load_max_seconds,
        headroom,
    ) == cache::strategy::Strategy::Windowed
}

/// The header probe of the file at `path`, from the probe cache or read afresh.
async fn probe_file(
    cache: &Cache,
    path: &std::path::Path,
    window: &Window,
) -> Result<Option<cache::strategy::Probe>, CommandError> {
    let key = path.to_string_lossy().into_owned();
    if let Some(probe) = cache.lock().await.cached_probe(&key) {
        return Ok(Some(probe));
    }
    let (rate, quality) = (window.rate, window.quality);
    let probe_key = key.clone();
    let probe = tokio::task::spawn_blocking(move || {
        cache::strategy::probe_local_file(&probe_key, rate, quality)
    })
    .await
    .map_err(error)?;
    if let Some(probe) = probe {
        cache.lock().await.store_probe(&key, probe);
    }
    Ok(probe)
}

/// Load `file` in full: from memory, by joining a load in progress, or by starting
/// a progressive decode.
async fn load_full(
    cache: &Cache,
    file: &str,
    rate: u32,
    freshness: config::FreshnessMode,
) -> Result<audio::streaming::SampleBuffer, CommandError> {
    cache
        .lock()
        .await
        .get_or_load_streaming_with_freshness(file, rate, freshness)
        .await
        .map_err(error)
}

/// Stream the file at `path` through a window, looping at its end when asked.
async fn stream_file(
    path: std::path::PathBuf,
    looping: bool,
    window: &Window,
) -> Result<PreparedStream, CommandError> {
    let (rate, quality, frames) = (window.rate, window.quality, window.frames);
    let handles = tokio::task::spawn_blocking(move || {
        audio::streamed_source::spawn_local_file_stream(
            path.to_string_lossy().into_owned(),
            rate,
            quality,
            frames,
            looping,
        )
    })
    .await
    .map_err(error)?
    .map_err(error)?;
    let stream = PreparedStream {
        handles: Some(handles),
        loops: true,
    };
    Ok(prebuffer(stream, window).await)
}

/// Wait until a windowed play has buffered its prebuffer, or its deadline passes.
async fn prebuffer(stream: PreparedStream, window: &Window) -> PreparedStream {
    stream
        .handles
        .as_ref()
        .expect("prepared handles")
        .wait_prebuffer(window.prebuffer_frames, window.deadline)
        .await;
    stream
}

/// Frames estimated from a URL's `Content-Length`: only uncompressed WAV maps bytes
/// to frames (about 4 bytes per 16-bit stereo frame).
fn wav_frames(url: &str, length: Option<u64>) -> Option<usize> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    length
        .filter(|_| path.to_lowercase().ends_with(".wav"))
        .map(|n| (n / 4) as usize)
}

/// What a started precache is doing in the background.
pub enum Precaching {
    /// Nothing to wait for: already cached, already loading, or not kept.
    Nothing,
    /// Decoding in full into the memory cache.
    Decode(audio::streaming::SampleBuffer),
    /// Downloading into the disk cache without decoding.
    Download(tokio::task::JoinHandle<()>),
}

impl Precaching {
    /// Wait until the background work has finished (or failed).
    pub async fn finished(self) {
        match self {
            Precaching::Nothing => {}
            Precaching::Decode(buffer) => {
                let Some(notify) = buffer.notifier_blocking() else {
                    return;
                };
                loop {
                    let notified = notify.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    if buffer.is_complete() || buffer.has_failed() {
                        break;
                    }
                    // A progress signal can land while the decoder holds the
                    // buffer, so the state is also rechecked periodically.
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_millis(100), notified).await;
                }
            }
            Precaching::Download(task) => {
                let _ = task.await;
            }
        }
    }
}

/// Start precaching `file` the way a play of it would load: a file within the
/// load-strategy limits is decoded in full into the memory cache; one over them
/// (it would play windowed) is not decoded, and a URL is downloaded into the disk
/// cache instead. Returns once the work has started.
pub async fn precache(
    file: &str,
    config: &config::Config,
    cache: &Cache,
    rate: u32,
) -> Result<Precaching, CommandError> {
    authorize(config, file)?;
    let window = Window {
        frames: 1,
        prebuffer_frames: 0,
        deadline: std::time::Duration::ZERO,
        rate,
        quality: config.advanced.resampler_quality,
    };
    let freshness = config.cache.freshness;
    let source = examine(cache, file, freshness).await;
    if source.resident {
        tracing::info!("Precache: {} is already in memory", file);
        return Ok(Precaching::Nothing);
    }
    if source.loading {
        return Ok(Precaching::Decode(
            load_full(cache, file, rate, freshness).await?,
        ));
    }

    if let Some(path) = source.local_path {
        let probe = probe_file(cache, &path, &window).await?;
        let windowed = {
            let admission = cache.lock().await;
            probe.is_some_and(|probe| {
                decides_windowed(
                    config,
                    config::LoadMode::Auto,
                    &probe,
                    admission.memory_headroom(),
                )
            })
        };
        if windowed {
            tracing::info!(
                "Precache: {} is over the limits for loading in full, so it plays windowed \
                 and is not decoded ahead",
                file
            );
            return Ok(Precaching::Nothing);
        }
        tracing::info!("Precache started: {}", file);
        return Ok(Precaching::Decode(
            load_full(cache, file, rate, freshness).await?,
        ));
    }

    let open = cache::http_stream::open_http_stream(file)
        .await
        .map_err(error)?;
    let length = open.content_length();
    let persist = persistence(&open, file, None, cache).await;
    let mut admission = cache.lock().await;
    admission.cleanup_completed_loads();
    if admission.is_resident(file) || admission.is_loading(file) {
        drop(admission);
        return Ok(Precaching::Decode(
            load_full(cache, file, rate, freshness).await?,
        ));
    }
    let windowed = length.is_none()
        || decides_windowed(
            config,
            config::LoadMode::Auto,
            &cache::strategy::probe_http(length, file),
            admission.memory_headroom(),
        );
    if !windowed {
        let reader = open.into_bounded_reader(persist);
        tracing::info!("Precache started: {}", file);
        return Ok(Precaching::Decode(
            admission.start_streaming_load_from_reader(
                file,
                reader,
                rate,
                wav_frames(file, length),
            ),
        ));
    }
    drop(admission);
    let Some(persist) = persist else {
        tracing::warn!(
            "Precache: {} is over the limits for loading in full and cannot be kept on disk \
             (the disk cache is off or the server sent no Content-Length); nothing precached",
            file
        );
        return Ok(Precaching::Nothing);
    };
    tracing::info!(
        "Precache: {} is over the limits for loading in full; downloading it to the disk cache",
        file
    );
    let mut reader = open.into_bounded_reader(Some(persist));
    let label = file.to_string();
    Ok(Precaching::Download(tokio::task::spawn_blocking(
        move || {
            let mut chunk = vec![0u8; 64 * 1024];
            loop {
                match std::io::Read::read(&mut reader, &mut chunk) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("Precache download of {} failed: {}", label, e);
                        break;
                    }
                }
            }
        },
    )))
}

async fn full_ready(
    buffer: audio::streaming::SampleBuffer,
    command: &AudioCommand,
    config: &config::Config,
) -> Result<PreparedPlayback, CommandError> {
    let start_ms = match command {
        AudioCommand::Play {
            start_position_ms, ..
        } => start_position_ms.unwrap_or(0),
        _ => 0,
    };
    if let Some(notify) = buffer.notifier_blocking() {
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(config.cache.stream_prebuffer_deadline_ms as u64);
        loop {
            let notified = notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if buffer.has_failed() {
                return Err(error("Audio decoding failed"));
            }
            let (_, sample_rate, _) = buffer.metadata_blocking();
            let frame = (start_ms * sample_rate as u64 / 1000) as usize;
            if buffer.is_complete() || buffer.is_frame_loaded(frame) {
                break;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                if buffer.frames() == 0 {
                    return Err(error("No audio decoded before the prebuffer deadline"));
                }
                break;
            }
        }
    }
    Ok(PreparedPlayback::Full(buffer))
}

/// Sends an explicit cancellation result when a loading task is aborted.
pub struct PendingReply(
    pub Option<tokio::sync::oneshot::Sender<crate::mqtt::commands::CommandOutcome>>,
);
impl Drop for PendingReply {
    fn drop(&mut self) {
        if let Some(reply) = self.0.take() {
            let _ = reply.send(Err(CommandError::new(
                CommandErrorKind::Cancelled,
                "Load cancelled before playback",
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TEST_WAV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/audio/test_440hz_2s.wav");

    fn play(file: &str) -> AudioCommand {
        AudioCommand::Play {
            file: file.to_string(),
            id: None,
            volume: 1.0,
            voice: None,
            channel_map: None,
            fade_in: None,
            start_position_ms: None,
            loop_mode: false,
            crossfade_ms: 0,
            mode: config::LoadMode::Auto,
            window_ms: None,
            prebuffer_ms: None,
            freshness: None,
            cacheable: None,
        }
    }

    fn test_cache() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::tempdir().unwrap();
        let cache: Cache = Arc::new(Mutex::new(
            cache::CacheManager::new(dir.path().join("cache")).unwrap(),
        ));
        (dir, cache)
    }

    /// Write `seconds` of a quiet 440 Hz mono 16-bit WAV at 48 kHz to `path`.
    fn write_wav(path: &std::path::Path, seconds: f32) {
        let frames = (seconds * 48000.0) as usize;
        let mut bytes = Vec::with_capacity(44 + frames * 2);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + frames as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&48000u32.to_le_bytes());
        bytes.extend_from_slice(&96000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(frames as u32 * 2).to_le_bytes());
        for i in 0..frames {
            let v = ((i as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin() * 3000.0) as i16;
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    async fn wait_until_loaded(file: &str, cache: &Cache) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while cache.lock().await.is_loading(file) {
            assert!(
                std::time::Instant::now() < deadline,
                "load of {file} did not finish"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    async fn wait_until_on_disk(file: &str, cache: &Cache) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !cache.lock().await.is_cached(file) {
            assert!(
                std::time::Instant::now() < deadline,
                "{file} never reached the disk cache"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Run a runtime `precache` of `file` and wait for its background load to finish.
    async fn precache_and_wait(file: &str, config: &config::Config, cache: &Cache) {
        let precache = AudioCommand::Precache {
            file: file.to_string(),
        };
        prepare(&precache, config, cache, 48000).await.unwrap();
        wait_until_loaded(file, cache).await;
    }

    /// How the scripted server answers one request.
    enum Reply {
        /// The whole file at once.
        Whole,
        /// Nothing until `release` fires, then the whole file.
        HeldUntil(tokio::sync::oneshot::Receiver<()>),
        /// The headers and the first `head` bytes, the rest when `release` fires.
        PartUntil(usize, tokio::sync::oneshot::Receiver<()>),
    }

    /// Serve `body` as a WAV file on a local port, answering the requests in turn
    /// as `replies` says (later ones get the whole file) and closing each connection
    /// after its response. Returns the URL and a count of the requests received.
    async fn serve_wav(body: Vec<u8>, replies: Vec<Reply>) -> (String, Arc<AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/cue.wav", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        let body = Arc::new(body);
        tokio::spawn(async move {
            let mut replies = replies.into_iter();
            while let Ok((mut socket, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let reply = replies.next().unwrap_or(Reply::Whole);
                let body = body.clone();
                tokio::spawn(async move {
                    let mut request = [0u8; 4096];
                    let _ = socket.read(&mut request).await;
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    match reply {
                        Reply::Whole => {}
                        Reply::HeldUntil(release) => {
                            let _ = release.await;
                        }
                        Reply::PartUntil(head, release) => {
                            let _ = socket.write_all(header.as_bytes()).await;
                            let _ = socket.write_all(&body[..head]).await;
                            let _ = release.await;
                            let _ = socket.write_all(&body[head..]).await;
                            return;
                        }
                    }
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        (url, requests)
    }

    fn is_full_from_memory(prepared: &PreparedPlayback) -> bool {
        matches!(
            prepared,
            PreparedPlayback::Full(audio::streaming::SampleBuffer::Complete(_))
        )
    }

    #[tokio::test]
    async fn a_precache_that_finishes_while_a_play_opens_its_url_is_used() {
        // The play finds nothing cached and opens its own request; a precache of the
        // same URL starts and finishes while that request is pending.
        let (release, released) = tokio::sync::oneshot::channel();
        let (url, requests) = serve_wav(
            std::fs::read(TEST_WAV).unwrap(),
            vec![Reply::HeldUntil(released)],
        )
        .await;
        let (_dir, cache) = test_cache();
        let config = config::Config::default();
        let playing = tokio::spawn({
            let (url, config, cache) = (url.clone(), config.clone(), cache.clone());
            async move { prepare(&play(&url), &config, &cache, 48000).await }
        });
        wait_until("the play's request", || {
            requests.load(Ordering::SeqCst) == 1
        })
        .await;
        precache_and_wait(&url, &config, &cache).await;
        release.send(()).unwrap();

        let prepared = playing.await.unwrap().unwrap();
        assert!(
            is_full_from_memory(&prepared),
            "the play did not use the precached decode"
        );
    }

    #[tokio::test]
    async fn a_finished_precache_of_a_url_plays_without_downloading_again() {
        let (url, requests) = serve_wav(std::fs::read(TEST_WAV).unwrap(), vec![]).await;
        let (_dir, cache) = test_cache();
        let config = config::Config::default();
        precache_and_wait(&url, &config, &cache).await;

        let prepared = prepare(&play(&url), &config, &cache, 48000).await.unwrap();
        assert!(
            is_full_from_memory(&prepared),
            "the play did not use the precached decode"
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_play_during_a_url_precache_joins_it_without_a_second_request() {
        let (release, released) = tokio::sync::oneshot::channel();
        let (url, requests) = serve_wav(
            std::fs::read(TEST_WAV).unwrap(),
            vec![Reply::PartUntil(64 * 1024, released)],
        )
        .await;
        let (_dir, cache) = test_cache();
        let config = config::Config::default();
        let precache = AudioCommand::Precache { file: url.clone() };
        prepare(&precache, &config, &cache, 48000).await.unwrap();

        let prepared = prepare(&play(&url), &config, &cache, 48000).await.unwrap();
        assert!(
            matches!(prepared, PreparedPlayback::Full(_)),
            "the play must join the precache's load"
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1, "no second download");
        release.send(()).unwrap();
    }

    #[tokio::test]
    async fn a_play_joins_a_full_load_of_a_long_local_file_in_progress() {
        // A full decode of a file over the auto limits is already running (a play
        // asked for "full"); a later auto play shares it instead of windowing.
        let (dir, cache) = test_cache();
        let file = dir.path().join("long.wav");
        write_wav(&file, 120.0);
        let file = file.to_str().unwrap().to_string();
        let mut config = config::Config::default();
        config.cache.full_load_max_bytes = u64::MAX;
        let mut full = play(&file);
        if let AudioCommand::Play { mode, .. } = &mut full {
            *mode = config::LoadMode::Full;
        }
        assert!(matches!(
            prepare(&full, &config, &cache, 48000).await.unwrap(),
            PreparedPlayback::Full(_)
        ));

        let prepared = prepare(&play(&file), &config, &cache, 48000).await.unwrap();
        assert!(
            matches!(prepared, PreparedPlayback::Full(_)),
            "the play must share the decode in progress"
        );
    }

    #[tokio::test]
    async fn a_disk_cached_url_over_the_limits_streams_from_disk_and_can_loop() {
        let (url, requests) = serve_wav(std::fs::read(TEST_WAV).unwrap(), vec![]).await;
        let (_dir, cache) = test_cache();
        let mut config = config::Config::default();
        // A windowed first play with a window longer than the file saves it to disk.
        let mut first = play(&url);
        if let AudioCommand::Play {
            mode, window_ms, ..
        } = &mut first
        {
            *mode = config::LoadMode::Stream;
            *window_ms = Some(5000);
        }
        let first = prepare(&first, &config, &cache, 48000).await.unwrap();
        assert!(matches!(first, PreparedPlayback::Stream(ref s) if !s.loops()));
        wait_until_on_disk(&url, &cache).await;

        config.cache.full_load_max_seconds = 1;
        let mut again = play(&url);
        if let AudioCommand::Play { loop_mode, .. } = &mut again {
            *loop_mode = true;
        }
        let prepared = prepare(&again, &config, &cache, 48000).await.unwrap();
        assert!(
            matches!(prepared, PreparedPlayback::Stream(ref s) if s.loops()),
            "a cached URL over the limits must stream from its disk copy, which can loop"
        );
        assert_eq!(requests.load(Ordering::SeqCst), 1, "played from disk");
        assert!(!cache.lock().await.is_resident(&url), "not decoded whole");
    }

    #[tokio::test]
    async fn an_edited_file_now_over_the_limits_plays_windowed() {
        let (dir, cache) = test_cache();
        let file = dir.path().join("cue.wav");
        write_wav(&file, 2.0);
        let file = file.to_str().unwrap().to_string();
        let mut config = config::Config::default();
        config.cache.full_load_max_seconds = 3;
        assert!(matches!(
            prepare(&play(&file), &config, &cache, 48000).await.unwrap(),
            PreparedPlayback::Full(_)
        ));
        wait_until_loaded(&file, &cache).await;

        write_wav(std::path::Path::new(&file), 4.0);
        let prepared = prepare(&play(&file), &config, &cache, 48000).await.unwrap();
        assert!(
            matches!(prepared, PreparedPlayback::Stream(_)),
            "the edited file is over the limits now"
        );
    }

    #[tokio::test]
    async fn a_precache_of_a_local_file_over_the_limits_does_not_decode_it() {
        let (_dir, cache) = test_cache();
        let mut config = config::Config::default();
        config.cache.full_load_max_seconds = 1;
        let started = precache(TEST_WAV, &config, &cache, 48000).await.unwrap();
        assert!(matches!(started, Precaching::Nothing));
        let cache = cache.lock().await;
        assert!(!cache.is_loading(TEST_WAV) && !cache.is_resident(TEST_WAV));
    }

    #[tokio::test]
    async fn a_precache_of_a_url_over_the_limits_downloads_it_to_disk_only() {
        let (url, requests) = serve_wav(std::fs::read(TEST_WAV).unwrap(), vec![]).await;
        let (_dir, cache) = test_cache();
        let mut config = config::Config::default();
        config.cache.full_load_max_bytes = 1000;
        precache(&url, &config, &cache, 48000)
            .await
            .unwrap()
            .finished()
            .await;
        wait_until_on_disk(&url, &cache).await;
        assert!(!cache.lock().await.is_resident(&url), "not decoded");

        let prepared = prepare(&play(&url), &config, &cache, 48000).await.unwrap();
        assert!(matches!(prepared, PreparedPlayback::Stream(_)));
        assert_eq!(requests.load(Ordering::SeqCst), 1, "played from disk");
    }

    #[tokio::test]
    async fn a_blocking_precache_waits_for_the_decode() {
        let (_dir, cache) = test_cache();
        let config = config::Config::default();
        precache(TEST_WAV, &config, &cache, 48000)
            .await
            .unwrap()
            .finished()
            .await;
        let mut cache = cache.lock().await;
        cache.cleanup_completed_loads();
        assert!(cache.is_resident(TEST_WAV));
    }

    /// Prepare a windowed play of the test file with `window_ms` and `prebuffer_ms`
    /// overrides and a long prebuffer deadline, returning how long it took to start.
    async fn windowed_start_time(
        window_ms: Option<u32>,
        prebuffer_ms: Option<u32>,
    ) -> std::time::Duration {
        let dir = tempfile::tempdir().unwrap();
        let cache: Cache = Arc::new(Mutex::new(
            cache::CacheManager::new(dir.path().to_path_buf()).unwrap(),
        ));
        let mut config = config::Config::default();
        config.cache.stream_prebuffer_deadline_ms = 5000;
        let mut command = play(TEST_WAV);
        if let AudioCommand::Play {
            mode,
            window_ms: window,
            prebuffer_ms: prebuffer,
            ..
        } = &mut command
        {
            *mode = config::LoadMode::Stream;
            *window = window_ms;
            *prebuffer = prebuffer_ms;
        }
        let started = std::time::Instant::now();
        let prepared = prepare(&command, &config, &cache, 48000).await.unwrap();
        assert!(matches!(prepared, PreparedPlayback::Stream(_)));
        started.elapsed()
    }

    #[tokio::test]
    async fn a_default_prebuffer_longer_than_the_play_window_does_not_hold_the_start() {
        // The window is all a windowed play buffers ahead, so a prebuffer longer
        // than it could never fill and would hold the play until its deadline.
        let elapsed = windowed_start_time(Some(100), None).await;
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "started after {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn a_prebuffer_longer_than_the_configured_window_does_not_hold_the_start() {
        let elapsed = windowed_start_time(None, Some(2000)).await;
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "started after {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn abandoning_a_prepared_loop_stops_its_producer() {
        let handles = audio::streamed_source::spawn_local_file_stream(
            TEST_WAV.into(),
            48000,
            config::ResamplerQuality::Fast,
            512,
            true,
        )
        .unwrap();
        let stop = handles.stop_flag.clone();
        let prepared = PreparedStream {
            handles: Some(handles),
            loops: true,
        };
        drop(prepared);
        assert!(stop.load(std::sync::atomic::Ordering::Acquire));
    }
    #[tokio::test]
    async fn aborted_load_replies_with_cancellation() {
        let (request, response) = crate::mqtt::commands::CommandRequest::with_reply("{}".into());
        let guard = PendingReply(request.reply);
        drop(guard);
        assert_eq!(
            response.await.unwrap().unwrap_err().kind,
            CommandErrorKind::Cancelled
        );
    }
}
