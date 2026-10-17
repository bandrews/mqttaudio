// ABOUTME: Lane A unit tests for the pure device-config selection helpers.
// ABOUTME: Verify channel/format/rate/buffer choices without opening a device.

use cpal::{BufferSize, SampleFormat};
use mqttaudio::audio::device_select::{
    select_buffer_size, select_channels, select_sample_format, select_sample_rate, BufferLimits,
    ConfigOption, SelectError,
};

fn opt(channels: u16, fmt: SampleFormat, min: u32, max: u32) -> ConfigOption {
    ConfigOption {
        channels,
        sample_format: fmt,
        min_rate: min,
        max_rate: max,
        buffer: BufferLimits::Unknown,
    }
}

#[test]
fn channels_none_picks_max_capped_at_32() {
    let opts = vec![
        opt(2, SampleFormat::F32, 44100, 48000),
        opt(8, SampleFormat::F32, 44100, 48000),
    ];
    assert_eq!(select_channels(&opts, None).unwrap(), 8);

    // An absurd plugin-reported value is capped: pick the real 2ch option.
    let absurd = vec![
        opt(2, SampleFormat::F32, 44100, 48000),
        opt(10000, SampleFormat::F32, 44100, 48000),
    ];
    assert_eq!(select_channels(&absurd, None).unwrap(), 2);
}

#[test]
fn channels_exact_then_next_larger_fallback() {
    let opts = vec![
        opt(2, SampleFormat::F32, 44100, 48000),
        opt(8, SampleFormat::F32, 44100, 48000),
    ];
    assert_eq!(select_channels(&opts, Some(8)).unwrap(), 8); // exact
    assert_eq!(select_channels(&opts, Some(6)).unwrap(), 8); // next-larger
    assert_eq!(select_channels(&opts, Some(2)).unwrap(), 2);
}

#[test]
fn channels_no_match_errors_without_panic() {
    let opts = vec![
        opt(2, SampleFormat::F32, 44100, 48000),
        opt(8, SampleFormat::F32, 44100, 48000),
    ];
    match select_channels(&opts, Some(9)) {
        Err(SelectError::NoChannelMatch {
            requested,
            available,
        }) => {
            assert_eq!(requested, 9);
            assert_eq!(available, vec![2, 8]);
        }
        other => panic!("expected NoChannelMatch, got {other:?}"),
    }
    assert_eq!(select_channels(&[], Some(2)), Err(SelectError::NoConfigs));
}

#[test]
fn sample_format_prefers_f32_but_can_choose_i16() {
    let both = vec![
        opt(2, SampleFormat::I16, 44100, 48000),
        opt(2, SampleFormat::F32, 44100, 48000),
    ];
    assert_eq!(select_sample_format(&both, 2), Some(SampleFormat::F32));

    let i16_only = vec![opt(2, SampleFormat::I16, 44100, 48000)];
    assert_eq!(select_sample_format(&i16_only, 2), Some(SampleFormat::I16));

    // No option offers the requested channel count.
    assert_eq!(select_sample_format(&i16_only, 6), None);
}

#[test]
fn sample_rate_nearest_discrete_and_continuous() {
    // Explicit discrete set (ALSA hw probe) wins; 64000 is nearest 48000.
    assert_eq!(
        select_sample_rate(&[(44100, 96000)], Some(&[44100, 48000, 96000]), 64000),
        48000
    );
    // A continuous span honors an in-range request.
    assert_eq!(select_sample_rate(&[(44100, 192000)], None, 96000), 96000);
    // cpal exposes discrete rates as min==max ranges: snap to the nearest.
    let points = [(44100, 44100), (48000, 48000), (96000, 96000)];
    assert_eq!(select_sample_rate(&points, None, 64000), 48000);
    // Above the top of the span clamps down to the max.
    assert_eq!(select_sample_rate(&[(44100, 192000)], None, 400000), 192000);
}

#[test]
fn buffer_size_fixed_within_range_else_default() {
    let range = BufferLimits::Range { min: 64, max: 8192 };
    assert!(matches!(
        select_buffer_size(range, 512),
        BufferSize::Fixed(512)
    ));
    assert!(matches!(select_buffer_size(range, 16), BufferSize::Default));
    assert!(matches!(
        select_buffer_size(range, 9000),
        BufferSize::Default
    ));
    assert!(matches!(
        select_buffer_size(BufferLimits::Unknown, 512),
        BufferSize::Default
    ));
}
