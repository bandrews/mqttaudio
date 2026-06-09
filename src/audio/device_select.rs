// ABOUTME: Pure device output-config selection (channels / format / rate / buffer).
// ABOUTME: Operates on plain data so the choices unit-test without opening a device.

use cpal::{BufferSize, SampleFormat};

/// Largest channel count we auto-select for; ALSA plugins can report absurd
/// values (10000+), so cap the automatic ("max available") choice.
pub const MAX_SANE_CHANNELS: u16 = 32;

/// One output configuration the device supports, mirroring
/// `cpal::SupportedStreamConfigRange` as plain data for testable selection.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigOption {
    pub channels: u16,
    pub sample_format: SampleFormat,
    pub min_rate: u32,
    pub max_rate: u32,
    pub buffer: BufferLimits,
}

/// Device buffer-size limits for a config option.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BufferLimits {
    Range { min: u32, max: u32 },
    Unknown,
}

/// Why an output configuration could not be chosen.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::enum_variant_names)] // the "No" prefix is semantic (each is an absence)
pub enum SelectError {
    /// The device exposed no output configurations at all.
    NoConfigs,
    /// No configuration offers a channel count that satisfies the request.
    NoChannelMatch {
        requested: usize,
        available: Vec<u16>,
    },
    /// No usable sample format for the chosen channel count.
    NoFormat { channels: u16 },
}

impl std::fmt::Display for SelectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SelectError::NoConfigs => write!(f, "device exposed no output configurations"),
            SelectError::NoChannelMatch {
                requested,
                available,
            } => write!(
                f,
                "no output configuration for {} channels (available: {:?})",
                requested, available
            ),
            SelectError::NoFormat { channels } => {
                write!(f, "no usable sample format for {} channels", channels)
            }
        }
    }
}

impl std::error::Error for SelectError {}

/// Choose the output channel count.
///
/// - `Some(n)`: an exact match if available, otherwise the smallest available
///   count greater than `n` (open more channels and zero-fill the extras);
///   `NoChannelMatch` if nothing satisfies it.
/// - `None`: the largest available count, capped at [`MAX_SANE_CHANNELS`].
pub fn select_channels(
    options: &[ConfigOption],
    requested: Option<usize>,
) -> Result<u16, SelectError> {
    if options.is_empty() {
        return Err(SelectError::NoConfigs);
    }

    let mut available: Vec<u16> = options.iter().map(|o| o.channels).collect();
    available.sort_unstable();
    available.dedup();

    match requested {
        None => {
            // Prefer the largest count within the sane cap; if every option is
            // above the cap (pathological plugin device), open the fewest.
            let capped_max = available
                .iter()
                .copied()
                .filter(|&c| c <= MAX_SANE_CHANNELS)
                .max();
            Ok(capped_max.unwrap_or_else(|| available[0]))
        }
        Some(req) => {
            let req16 = req as u16;
            if available.contains(&req16) {
                Ok(req16)
            } else if let Some(&next) = available.iter().find(|&&c| c > req16) {
                Ok(next)
            } else {
                Err(SelectError::NoChannelMatch {
                    requested: req,
                    available,
                })
            }
        }
    }
}

/// Choose a sample format among options for the chosen channel count.
/// Prefer `F32` (transparent to the f32 mix bus), otherwise the first of the
/// other cpal-buildable formats that the device offers for those channels.
pub fn select_sample_format(options: &[ConfigOption], channels: u16) -> Option<SampleFormat> {
    let formats: Vec<SampleFormat> = options
        .iter()
        .filter(|o| o.channels == channels)
        .map(|o| o.sample_format)
        .collect();

    const PREFERENCE: [SampleFormat; 4] = [
        SampleFormat::F32,
        SampleFormat::I16,
        SampleFormat::I32,
        SampleFormat::U16,
    ];
    PREFERENCE
        .iter()
        .copied()
        .find(|f| formats.contains(f))
        .or_else(|| formats.into_iter().next())
}

fn nearest(rates: impl Iterator<Item = u32>, target: u32) -> Option<u32> {
    rates.min_by_key(|&r| (r as i64 - target as i64).abs())
}

/// Choose the sample rate nearest to `requested` that the device can actually
/// produce.
///
/// `ranges` are the `(min, max)` rate spans of the matching `(channels, format)`
/// configurations. cpal reports discrete rates (CoreAudio/WASAPI) as separate
/// `min == max` ranges and a true span (ALSA) as `min < max`. `discrete` is an
/// optional explicit set (e.g. an ALSA `hw:` device's probed discrete rates),
/// which takes precedence when present.
pub fn select_sample_rate(ranges: &[(u32, u32)], discrete: Option<&[u32]>, requested: u32) -> u32 {
    if let Some(rates) = discrete.filter(|r| !r.is_empty()) {
        return nearest(rates.iter().copied(), requested).unwrap_or(requested);
    }

    // Honor the request if it falls inside any continuous span.
    if ranges
        .iter()
        .any(|&(lo, hi)| requested >= lo && requested <= hi)
    {
        return requested;
    }

    // Otherwise snap to the nearest reachable endpoint across all spans/points.
    nearest(ranges.iter().flat_map(|&(lo, hi)| [lo, hi]), requested).unwrap_or(requested)
}

/// Choose a buffer size: honor `requested` via `Fixed` when it is within the
/// device's supported range, otherwise fall back to `Default`.
pub fn select_buffer_size(buffer: BufferLimits, requested: u32) -> BufferSize {
    match buffer {
        BufferLimits::Range { min, max } if requested >= min && requested <= max => {
            BufferSize::Fixed(requested)
        }
        _ => BufferSize::Default,
    }
}
