// ABOUTME: Load-strategy decision: full in-memory load vs windowed streaming.
// ABOUTME: A cheap header probe plus the memory budget choose, without decoding.

//! Choosing how to play a local asset. A small SFX or voiceover is fully decoded into
//! memory (all features, instant replay); a big or long cue is windowed (bounded
//! memory, low time-to-first-sample, forward playback only). The choice is made from a
//! cheap, decode-free header probe and the live memory-budget headroom, so the never-
//! OOM guarantee holds: an estimate larger than the headroom is force-windowed
//! regardless of the requested mode, and a full load can never blow the cap.

use crate::audio::streaming_decoder::StreamingDecoder;
use crate::config::{LoadMode, ResamplerQuality};
use std::fs::File;
use symphonia::core::probe::Hint;

/// A cheap, decode-free estimate of an asset, used to pick a load strategy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probe {
    /// Estimated decoded size in bytes at the output rate (`frames * channels * 4`),
    /// if the header carried a frame count.
    pub est_decoded_bytes: Option<u64>,
    /// Estimated duration in seconds, if known.
    pub est_seconds: Option<f64>,
}

/// The chosen load strategy for a play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Decode fully into memory: all features, instant replay (within budget).
    FullLoad,
    /// Stream through a bounded window: low latency, `O(window)` memory, forward only.
    Windowed,
}

/// A conservative decoded-size estimate from a file's on-disk byte size, used only as a
/// fallback when the header carries no frame count (so `frames * channels * 4` is
/// unavailable). Biased **up** so a large count-less asset still trips the budget/size
/// checks and windows rather than full-loading into an OOM — the never-OOM guarantee
/// must not depend on a frame count being present. A lossy codec decodes to many times
/// its compressed bytes (f32 PCM at the output rate ≈ 10–25× the stored size), while
/// lossless/PCM expands only modestly (f32 is at most 2× 16-bit PCM); unknown formats
/// take the conservative (high) factor.
fn decoded_size_estimate_from_file_size(file_len: u64, extension: Option<&str>) -> u64 {
    let factor = match extension.map(str::to_ascii_lowercase).as_deref() {
        Some("wav" | "wave" | "flac" | "aiff" | "aif" | "alac" | "wv") => 2,
        _ => 25,
    };
    file_len.saturating_mul(factor)
}

/// Probe a local file's header (no packet decode) for a decoded-size estimate at
/// `target_sample_rate`. Returns `None` if the file or its header could not be read.
/// When the header carries a frame count the estimate is exact (`frames * channels *
/// 4`); otherwise it falls back to a conservative size-based estimate so a large
/// count-less file is still recognized as too big to full-load.
pub fn probe_local_file(
    path: &str,
    target_sample_rate: u32,
    quality: ResamplerQuality,
) -> Option<Probe> {
    let file = File::open(path).ok()?;
    let mut hint = Hint::new();
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str());
    if let Some(ext) = extension {
        hint.with_extension(ext);
    }
    // Constructing the decoder parses the container header (channels, rate, frame
    // count) but decodes no audio.
    let decoder =
        StreamingDecoder::new(file, Some(&hint), Some(target_sample_rate), quality).ok()?;
    let channels = decoder.channels().max(1) as u64;
    // `estimated_frames()` is already scaled to the target (post-resample) rate.
    let est_frames = decoder.estimated_frames();
    let est_decoded_bytes = match est_frames {
        Some(f) => Some(f * channels * 4),
        // No frame count: fall back to a conservative size-based estimate (biased up).
        None => std::fs::metadata(path)
            .ok()
            .map(|m| decoded_size_estimate_from_file_size(m.len(), extension)),
    };
    Some(Probe {
        est_decoded_bytes,
        est_seconds: est_frames.map(|f| f as f64 / target_sample_rate.max(1) as f64),
    })
}

/// Build a load-strategy probe for an HTTP source from its `Content-Length`. A known
/// length estimates decoded bytes via the same biased-up codec-class factor as the local
/// size fallback; an unknown length (no header, e.g. a live stream) is treated as
/// effectively unbounded so the decision always windows it — a source whose size cannot
/// be known can never be safely full-loaded under the never-OOM guarantee.
pub fn probe_http(content_length: Option<u64>, url: &str) -> Probe {
    match content_length {
        Some(len) => {
            let ext = url
                .split(['?', '#'])
                .next()
                .and_then(|p| std::path::Path::new(p).extension())
                .and_then(|e| e.to_str());
            Probe {
                est_decoded_bytes: Some(decoded_size_estimate_from_file_size(len, ext)),
                est_seconds: None,
            }
        }
        None => Probe {
            est_decoded_bytes: Some(u64::MAX),
            est_seconds: None,
        },
    }
}

/// Decide full-load vs windowed for a local asset.
///
/// Precedence: a non-`Auto` `per_play` mode wins; else a non-`Auto` `config_mode`;
/// else the `Auto` heuristic (the size/duration thresholds). The memory budget is a
/// hard override on top: if the estimated decoded size exceeds `headroom_bytes`, the
/// asset is force-windowed regardless of mode — a full load can never blow the cap, so
/// even an explicit `mode=full` on an over-budget asset is downgraded to windowed.
pub fn decide(
    per_play: LoadMode,
    config_mode: LoadMode,
    probe: &Probe,
    full_load_max_bytes: u64,
    full_load_max_seconds: u32,
    headroom_bytes: usize,
) -> Strategy {
    let effective = if per_play != LoadMode::Auto {
        per_play
    } else {
        config_mode
    };

    // A known estimate larger than the budget headroom can never be a full load.
    let over_budget = probe
        .est_decoded_bytes
        .is_some_and(|b| b > headroom_bytes as u64);

    match effective {
        LoadMode::Stream => Strategy::Windowed,
        LoadMode::Full => {
            if over_budget {
                Strategy::Windowed
            } else {
                Strategy::FullLoad
            }
        }
        LoadMode::Auto => {
            let too_big = probe
                .est_decoded_bytes
                .is_some_and(|b| b > full_load_max_bytes);
            let too_long = probe
                .est_seconds
                .is_some_and(|s| s > full_load_max_seconds as f64);
            if over_budget || too_big || too_long {
                Strategy::Windowed
            } else {
                Strategy::FullLoad
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUGE: usize = usize::MAX; // effectively-unlimited headroom for non-budget tests
    const MAX_BYTES: u64 = 32 * 1024 * 1024;
    const MAX_SECONDS: u32 = 60;

    fn probe(bytes: Option<u64>, seconds: Option<f64>) -> Probe {
        Probe {
            est_decoded_bytes: bytes,
            est_seconds: seconds,
        }
    }

    #[test]
    fn explicit_stream_always_windows() {
        // Even a tiny asset windows when stream is requested.
        let p = probe(Some(1024), Some(0.1));
        assert_eq!(
            decide(
                LoadMode::Stream,
                LoadMode::Auto,
                &p,
                MAX_BYTES,
                MAX_SECONDS,
                HUGE
            ),
            Strategy::Windowed
        );
    }

    #[test]
    fn explicit_full_within_budget_full_loads() {
        let p = probe(Some(64 * 1024 * 1024), Some(600.0)); // big + long, but mode=full
        assert_eq!(
            decide(
                LoadMode::Full,
                LoadMode::Auto,
                &p,
                MAX_BYTES,
                MAX_SECONDS,
                HUGE
            ),
            Strategy::FullLoad
        );
    }

    #[test]
    fn explicit_full_over_budget_is_downgraded_to_windowed() {
        // mode=full cannot exceed the cap: the budget override wins.
        let p = probe(Some(2 * 1024 * 1024 * 1024), None); // ~2 GiB
        let headroom = 512 * 1024 * 1024; // 512 MiB
        assert_eq!(
            decide(
                LoadMode::Full,
                LoadMode::Auto,
                &p,
                MAX_BYTES,
                MAX_SECONDS,
                headroom
            ),
            Strategy::Windowed
        );
    }

    #[test]
    fn auto_small_asset_full_loads() {
        let p = probe(Some(4 * 1024 * 1024), Some(10.0)); // 4 MiB, 10 s
        assert_eq!(
            decide(
                LoadMode::Auto,
                LoadMode::Auto,
                &p,
                MAX_BYTES,
                MAX_SECONDS,
                HUGE
            ),
            Strategy::FullLoad
        );
    }

    #[test]
    fn auto_windows_when_too_big() {
        let p = probe(Some(64 * 1024 * 1024), Some(10.0)); // 64 MiB > 32 MiB
        assert_eq!(
            decide(
                LoadMode::Auto,
                LoadMode::Auto,
                &p,
                MAX_BYTES,
                MAX_SECONDS,
                HUGE
            ),
            Strategy::Windowed
        );
    }

    #[test]
    fn auto_windows_when_too_long() {
        let p = probe(Some(4 * 1024 * 1024), Some(120.0)); // 120 s > 60 s
        assert_eq!(
            decide(
                LoadMode::Auto,
                LoadMode::Auto,
                &p,
                MAX_BYTES,
                MAX_SECONDS,
                HUGE
            ),
            Strategy::Windowed
        );
    }

    #[test]
    fn auto_windows_when_over_budget_even_if_under_thresholds() {
        // Under the size/duration thresholds, but no room in the cache right now.
        let p = probe(Some(20 * 1024 * 1024), Some(30.0));
        let headroom = 8 * 1024 * 1024; // only 8 MiB free
        assert_eq!(
            decide(
                LoadMode::Auto,
                LoadMode::Auto,
                &p,
                MAX_BYTES,
                MAX_SECONDS,
                headroom
            ),
            Strategy::Windowed
        );
    }

    #[test]
    fn config_mode_applies_when_per_play_is_auto() {
        let small = probe(Some(1024), Some(0.1));
        // Operator set the global default to stream.
        assert_eq!(
            decide(
                LoadMode::Auto,
                LoadMode::Stream,
                &small,
                MAX_BYTES,
                MAX_SECONDS,
                HUGE
            ),
            Strategy::Windowed
        );
        // Per-play full overrides the config stream default.
        assert_eq!(
            decide(
                LoadMode::Full,
                LoadMode::Stream,
                &small,
                MAX_BYTES,
                MAX_SECONDS,
                HUGE
            ),
            Strategy::FullLoad
        );
    }

    #[test]
    fn unknown_estimate_under_auto_full_loads() {
        // A header with no frame count trips no threshold and no budget check.
        let p = probe(None, None);
        assert_eq!(
            decide(
                LoadMode::Auto,
                LoadMode::Auto,
                &p,
                MAX_BYTES,
                MAX_SECONDS,
                1
            ),
            Strategy::FullLoad
        );
    }

    #[test]
    fn size_fallback_biases_up_for_lossy_and_unknown() {
        // Lossy codecs decode to many times their compressed size: bias up so a large
        // count-less file windows rather than full-loading into an OOM.
        assert_eq!(
            decoded_size_estimate_from_file_size(1_000_000, Some("mp3")),
            25_000_000
        );
        assert_eq!(
            decoded_size_estimate_from_file_size(1_000_000, Some("ogg")),
            25_000_000
        );
        // Case-insensitive, and an unknown/odd extension takes the conservative factor.
        assert_eq!(
            decoded_size_estimate_from_file_size(1_000_000, Some("MP3")),
            25_000_000
        );
        assert_eq!(
            decoded_size_estimate_from_file_size(1_000_000, Some("dat")),
            25_000_000
        );
        assert_eq!(
            decoded_size_estimate_from_file_size(1_000_000, None),
            25_000_000
        );
    }

    #[test]
    fn size_fallback_is_modest_for_lossless() {
        // PCM/lossless expands little (f32 over 16-bit), and these formats almost always
        // carry a frame count anyway, so the fallback rarely governs them.
        assert_eq!(
            decoded_size_estimate_from_file_size(1_000_000, Some("wav")),
            2_000_000
        );
        assert_eq!(
            decoded_size_estimate_from_file_size(1_000_000, Some("flac")),
            2_000_000
        );
    }

    #[test]
    fn size_fallback_estimate_can_force_windowed_under_auto() {
        // A 50 MB count-less lossy file estimates to ~1.25 GB decoded → windowed under
        // auto (and certainly over a real budget), so it never full-loads to an OOM.
        let bytes = decoded_size_estimate_from_file_size(50 * 1024 * 1024, Some("mp3"));
        let p = probe(Some(bytes), None);
        assert_eq!(
            decide(
                LoadMode::Auto,
                LoadMode::Auto,
                &p,
                MAX_BYTES,
                MAX_SECONDS,
                HUGE
            ),
            Strategy::Windowed
        );
    }

    #[test]
    fn probe_http_unknown_length_estimates_unbounded() {
        // No Content-Length (live stream): the estimate is unbounded. Under auto/stream
        // it windows on the size heuristic; under an explicit full with a finite budget
        // the budget override windows it. (A live source is additionally force-windowed
        // by the caller regardless of mode, since it has no finite end to full-load.)
        let p = probe_http(None, "http://example.com/live");
        assert_eq!(p.est_decoded_bytes, Some(u64::MAX));
        assert_eq!(
            decide(
                LoadMode::Auto,
                LoadMode::Auto,
                &p,
                MAX_BYTES,
                MAX_SECONDS,
                HUGE
            ),
            Strategy::Windowed
        );
        assert_eq!(
            decide(
                LoadMode::Stream,
                LoadMode::Auto,
                &p,
                MAX_BYTES,
                MAX_SECONDS,
                HUGE
            ),
            Strategy::Windowed
        );
        let finite_headroom = 512 * 1024 * 1024;
        assert_eq!(
            decide(
                LoadMode::Full,
                LoadMode::Auto,
                &p,
                MAX_BYTES,
                MAX_SECONDS,
                finite_headroom
            ),
            Strategy::Windowed
        );
    }

    #[test]
    fn probe_http_known_length_estimates_and_decides() {
        // A small known file full-loads under auto; a large one windows.
        let small = probe_http(Some(500_000), "http://example.com/sfx.mp3"); // ~12.5 MB est
        assert_eq!(
            decide(
                LoadMode::Auto,
                LoadMode::Auto,
                &small,
                MAX_BYTES,
                MAX_SECONDS,
                HUGE
            ),
            Strategy::FullLoad
        );
        let big = probe_http(Some(50 * 1024 * 1024), "http://example.com/cue.mp3?sig=x"); // huge
        assert_eq!(
            decide(
                LoadMode::Auto,
                LoadMode::Auto,
                &big,
                MAX_BYTES,
                MAX_SECONDS,
                HUGE
            ),
            Strategy::Windowed
        );
    }

    #[test]
    fn probe_local_file_estimates_a_wav_without_decoding() {
        let path = "tests/audio/test_440hz_2s.wav";
        if !std::path::Path::new(path).exists() {
            eprintln!("skipping: {path} not found");
            return;
        }
        let p = probe_local_file(path, 48000, ResamplerQuality::Fast).unwrap();
        // 2 s @ 48k stereo f32 ~= 96000 * 2 * 4 = 768000 bytes.
        let bytes = p
            .est_decoded_bytes
            .expect("a WAV header carries a frame count");
        assert!(
            (700_000..=820_000).contains(&bytes),
            "expected ~768000 decoded bytes, got {bytes}"
        );
        let secs = p.est_seconds.expect("a WAV header carries a frame count");
        assert!((secs - 2.0).abs() < 0.1, "expected ~2 s, got {secs}");
    }
}
