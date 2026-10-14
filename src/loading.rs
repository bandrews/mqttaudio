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

pub struct PreparedStream(Option<audio::streamed_source::StreamHandles>);
impl PreparedStream {
    pub fn into_handles(mut self) -> audio::streamed_source::StreamHandles {
        self.0.take().expect("prepared stream owns its handles")
    }
}
impl Drop for PreparedStream {
    fn drop(&mut self) {
        if let Some(handles) = &self.0 {
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
            let mut cache = cache.lock().await;
            if matches!(command, CacheReload { .. }) {
                cache.invalidate(file).map_err(error)?;
            }
            cache.precache_streaming(file, rate).await.map_err(error)?;
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
    let prebuffer_ms = prebuffer_ms.unwrap_or(config.cache.stream_prebuffer_ms);
    let window_frames = (window_ms as usize * rate as usize / 1000).max(1);
    let quality = config.advanced.resampler_quality;
    let http = file.starts_with("http://") || file.starts_with("https://");
    let (cached, probe) = {
        let cache = cache.lock().await;
        (
            if http {
                cache.is_cached(file)
            } else {
                cache.is_resident(file)
            },
            cache.cached_probe(file),
        )
    };
    let decide = |probe: &cache::strategy::Probe, headroom: usize| {
        cache::strategy::decide(
            *mode,
            config.cache.load_mode,
            probe,
            config.cache.full_load_max_bytes,
            config.cache.full_load_max_seconds,
            headroom,
        ) == cache::strategy::Strategy::Windowed
    };
    let handles = if http && !cached {
        let open = cache::http_stream::open_http_stream(file)
            .await
            .map_err(error)?;
        let length = open.content_length();
        let persist = persistence(&open, file, *cacheable, cache).await;
        let mut admission = cache.lock().await;
        if admission.is_loading(file) || admission.is_resident(file) {
            let buffer = admission
                .get_or_load_streaming_with_freshness(
                    file,
                    rate,
                    freshness.unwrap_or(config.cache.freshness),
                )
                .await
                .map_err(error)?;
            drop(admission);
            return full_ready(buffer, command, config).await;
        }
        let windowed = length.is_none()
            || decide(
                &cache::strategy::probe_http(length, file),
                admission.memory_headroom(),
            );
        let reader = open.into_bounded_reader(persist);
        if !windowed {
            let buffer = admission.start_streaming_load_from_reader(
                file,
                reader,
                rate,
                length
                    .filter(|_| {
                        file.split(['?', '#'])
                            .next()
                            .unwrap_or(file)
                            .to_lowercase()
                            .ends_with(".wav")
                    })
                    .map(|n| (n / 4) as usize),
            );
            drop(admission);
            return full_ready(buffer, command, config).await;
        }
        drop(admission);
        let label = file.clone();
        Some(
            tokio::task::spawn_blocking(move || {
                audio::streamed_source::spawn_stream_from_source(
                    reader,
                    label,
                    rate,
                    quality,
                    window_frames,
                )
                .map(|handles| PreparedStream(Some(handles)))
            })
            .await
            .map_err(error)?
            .map_err(error)?,
        )
    } else if !http {
        let probe = if cached || *mode == config::LoadMode::Stream {
            probe
        } else {
            match probe {
                Some(probe) => Some(probe),
                None => {
                    let path = file.clone();
                    let probe = tokio::task::spawn_blocking(move || {
                        cache::strategy::probe_local_file(&path, rate, quality)
                    })
                    .await
                    .map_err(error)?;
                    if let Some(probe) = probe {
                        cache.lock().await.store_probe(file, probe);
                    }
                    probe
                }
            }
        };
        let mut admission = cache.lock().await;
        let windowed = *mode == config::LoadMode::Stream
            || (!admission.is_resident(file)
                && probe
                    .as_ref()
                    .is_some_and(|probe| decide(probe, admission.memory_headroom())));
        if !windowed {
            let buffer = admission
                .get_or_load_streaming_with_freshness(
                    file,
                    rate,
                    freshness.unwrap_or(config.cache.freshness),
                )
                .await
                .map_err(error)?;
            drop(admission);
            return full_ready(buffer, command, config).await;
        }
        drop(admission);
        if windowed {
            let path = file.clone();
            let looping = *loop_mode;
            Some(
                tokio::task::spawn_blocking(move || {
                    audio::streamed_source::spawn_local_file_stream(
                        path,
                        rate,
                        quality,
                        window_frames,
                        looping,
                    )
                    .map(|handles| PreparedStream(Some(handles)))
                })
                .await
                .map_err(error)?
                .map_err(error)?,
            )
        } else {
            None
        }
    } else {
        None
    };
    if let Some(handles) = handles {
        let deadline = config.cache.stream_prebuffer_deadline_ms.max(prebuffer_ms);
        handles
            .0
            .as_ref()
            .expect("prepared handles")
            .wait_prebuffer(
                prebuffer_ms as usize * rate as usize / 1000,
                std::time::Duration::from_millis(deadline as u64),
            )
            .await;
        return Ok(PreparedPlayback::Stream(handles));
    }
    let buffer = cache
        .lock()
        .await
        .get_or_load_streaming_with_freshness(
            file,
            rate,
            freshness.unwrap_or(config.cache.freshness),
        )
        .await
        .map_err(error)?;
    full_ready(buffer, command, config).await
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
    if let Some(notify) = buffer.notifier() {
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
    #[tokio::test]
    async fn abandoning_a_prepared_loop_stops_its_producer() {
        let handles = audio::streamed_source::spawn_local_file_stream(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/audio/test_440hz_2s.wav").into(),
            48000,
            config::ResamplerQuality::Fast,
            512,
            true,
        )
        .unwrap();
        let stop = handles.stop_flag.clone();
        let prepared = PreparedStream(Some(handles));
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
