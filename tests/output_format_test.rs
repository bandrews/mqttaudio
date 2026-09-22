// ABOUTME: Proves the f32->device-format convert shim is transparent.
// ABOUTME: Round-trips a sine through each target type and checks the metrics survive.

use cpal::{FromSample, Sample};
use mqttaudio::audio::test_support::{band_energy, max_inter_sample_delta, peak, rms, sine};

const SR: u32 = 48000;

/// Convert an f32 bus to `T` (exactly what the typed output callback does) and
/// back to f32, mirroring the device round-trip.
fn roundtrip<T>(bus: &[f32]) -> Vec<f32>
where
    T: Sample + FromSample<f32>,
    f32: FromSample<T>,
{
    bus.iter()
        .map(|&s| f32::from_sample(T::from_sample(s)))
        .collect()
}

#[test]
fn convert_shim_is_transparent_for_each_format() {
    let freq = 480.0;
    let amp = 0.5;
    let bus = sine(freq, SR, 4096, 2, amp);
    let ref_rms = rms(&bus, 0, 2);
    let ref_peak = peak(&bus);
    let natural_slope = amp * std::f32::consts::TAU * freq / SR as f32;

    let cases: [(&str, Vec<f32>); 4] = [
        ("i16", roundtrip::<i16>(&bus)),
        ("u16", roundtrip::<u16>(&bus)),
        ("i32", roundtrip::<i32>(&bus)),
        ("f32", roundtrip::<f32>(&bus)),
    ];

    for (name, out) in cases {
        assert_eq!(out.len(), bus.len(), "{name}: shim changed sample count");
        // RMS and peak survive the conversion within quantization tolerance.
        assert!(
            (rms(&out, 0, 2) - ref_rms).abs() < 0.01,
            "{name}: rms drifted"
        );
        assert!((peak(&out) - ref_peak).abs() < 0.01, "{name}: peak drifted");
        // The tone still dominates its band.
        assert!(
            band_energy(&out, SR, 0, 2, freq) > 0.4,
            "{name}: lost the tone"
        );
        // No discontinuity beyond the tone's own slope (plus quantization).
        assert!(
            max_inter_sample_delta(&out, 0, 2) < natural_slope * 2.0 + 0.01,
            "{name}: unexpected discontinuity"
        );
    }
}
