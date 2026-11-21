// ABOUTME: Exercises motion cue validation and rendered speaker levels.
// ABOUTME: Checks timing against the output clock across source transports and callback sizes.

use mqttaudio::audio::mixer::{mix_audio, ActiveSample};
use mqttaudio::audio::test_support::{decoded, SceneBuilder};
use mqttaudio::config::Config;
use mqttaudio::motion::Motion;
use mqttaudio::mqtt::commands::parse_command;
use serde_json::{json, Value};

fn motion(value: Value) -> Motion {
    serde_json::from_value(value).unwrap()
}

#[test]
fn malformed_motion_is_rejected_before_loading() {
    for spec in [
        json!({"path":[]}),
        json!({"path":[0,1]}),
        json!({"path":[0,1],"duration_ms":0}),
        json!({"path":[{"speaker":0,"at_ms":1},1],"duration_ms":100}),
        json!({"path":[0,{"place":"door","at_ms":40},1],"duration_ms":40}),
        json!({"path":[0,{"speaker":1,"at_ms":60},2],"duration_ms":40}),
        json!({"path":[0,{"speaker":1,"at_ms":60}],"duration_ms":100}),
        json!({"path":[0],"handoff":1.1}),
        json!({"path":[0],"handoff":-0.1}),
        json!({"path":[{"place":"door","speaker":1}]}),
        json!({"path":[{"x":1,"y":1}]}),
        json!({"path":[0],"easing":"typo"}),
    ] {
        assert!(
            parse_command(&json!({"command":"play","file":"a.wav","motion":spec}).to_string())
                .is_err(),
            "{spec}"
        );
    }
    assert!(parse_command(
        r#"{"command":"play","file":"a.wav","channel_map":[],"motion":{"path":[0]}}"#
    )
    .is_err());
}

#[test]
fn place_and_source_validation_uses_installation_and_actual_device_channels() {
    for (name, weights) in [
        ("0", json!({"0":1})),
        ("left", json!({"1":1})),
        ("", json!({"1":1})),
        ("door", json!({})),
        ("door", json!({"1":0})),
        ("door", json!({"1":-1})),
        ("door", json!({"absent":1})),
        ("door", json!({"left":1,"0":2})),
        ("door", json!({"2":1})),
    ] {
        let config: Config = serde_json::from_value(json!({"audio":{
            "channels":2,"channel_aliases":{"left":0},"places":{(name):weights}
        }}))
        .unwrap();
        let errors = config.validate().unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("audio.places")),
            "{errors:?}"
        );
    }
    let config: Config =
        serde_json::from_value(json!({"audio":{"places":{"far":{"3":1}}}})).unwrap();
    for (spec, inputs) in [
        (json!({"path":["far"]}), 1),
        (json!({"path":[{"place":"missing"}]}), 1),
        (json!({"path":[2]}), 1),
        (json!({"path":[0],"source_channel":2}), 2),
        (json!({"path":[0]}), 6),
    ] {
        assert!(motion(spec).prepare(&config, 2, 48000, inputs).is_err());
    }
    assert!(motion(json!({"path":["far"],"source_channel":5}))
        .prepare(&config, 4, 48000, 6)
        .is_ok());
}

#[test]
fn untimed_points_share_one_eased_span_and_zero_handoff_cuts_at_its_midpoint() {
    let spec =
        motion(json!({"path":[0,1,{"speaker":2,"at_ms":100}],"easing":"ease_in","handoff":0}));
    let mut sample = ActiveSample::new(
        0,
        "cue".into(),
        decoded(vec![0.25; 2], 1, 1000),
        1.0,
        1.0,
        "cue".into(),
    );
    sample.loop_mode = true;
    sample.motion = Some(Box::new(
        spec.prepare(&Config::default(), 3, 1000, 1).unwrap(),
    ));
    let mut state = SceneBuilder::new(3).sample(sample).build();
    let mut audio = vec![0.0; 120 * 3];
    mix_audio(&mut audio, &mut state);
    // Two equal legs use positions 2*t². The cuts fall at 50 ms and ceil(sqrt(.75)*100) ms.
    for (i, frame) in audio.as_chunks::<3>().0.iter().enumerate() {
        let speaker = if i < 50 {
            0
        } else if i < 87 {
            1
        } else {
            2
        };
        for (ch, &out) in frame.iter().enumerate() {
            assert_eq!(
                out,
                if ch == speaker { 0.25 } else { 0.0 },
                "frame {i}, ch {ch}"
            );
        }
    }
}

#[test]
fn pitch_and_seek_preserve_motion_phase_and_selected_input() {
    let rate = 48000;
    let data = [0.0, 0.2].repeat(60000);
    let mut sample = ActiveSample::new(
        1,
        "cue".into(),
        decoded(data, 2, rate),
        1.0,
        1.0,
        "cue".into(),
    );
    sample.position = 16000;
    assert!(sample.set_speed_with_mode(1.25, true));
    sample.motion = Some(Box::new(
        motion(json!({"path":[0,1],"duration_ms":20,"source_channel":1}))
            .prepare(&Config::default(), 2, rate, 2)
            .unwrap(),
    ));
    let mut mixer = SceneBuilder::new(2).sample(sample).build();
    let mut first = vec![0.; 480 * 2];
    mix_audio(&mut first, &mut mixer);
    assert!(first[0] > 0.15 && first[1] == 0.0);
    mqttaudio::rt_engine::apply_command(
        &mut mixer,
        mqttaudio::rt_engine::AudioCommand::SeekMatching {
            selector: serde_json::from_value(json!({"internal_id":"1"})).unwrap(),
            position_ms: 450,
        },
        rate,
    );
    let mut next = vec![0.; 600 * 2];
    mix_audio(&mut next, &mut mixer);
    // The stretcher controls signal amplitude. The ratio of the two outputs
    // must follow the same show clock at every non-silent frame, across the seek.
    assert!(next[..960].iter().any(|s| s.abs() > 0.01));
    for (i, frame) in next.as_chunks::<2>().0.iter().enumerate() {
        let t = ((480 + i) as f32 / 960.0).min(1.0);
        let (right, left) = (t * std::f32::consts::FRAC_PI_2).sin_cos();
        assert!(
            (frame[0] * right - frame[1] * left).abs() < 1e-5,
            "frame {i}: {frame:?}"
        );
    }
    assert!(next[960..]
        .as_chunks::<2>()
        .0
        .iter()
        .any(|f| f[1].abs() > 0.02));
}

#[test]
fn windowed_motion_waits_for_first_frame_then_keeps_time_across_underruns() {
    use mqttaudio::audio::mixer::StreamedSource;
    use ringbuf::HeapRb;
    use std::sync::{atomic::AtomicBool, Arc};

    let (mut tx, rx) = HeapRb::<f32>::new(20).split();
    let mut source = StreamedSource::new(
        1,
        "cue".into(),
        "steps".into(),
        None,
        rx,
        1,
        1.0,
        vec![],
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    source.motion = Some(Box::new(
        motion(json!({"path":[0,1],"duration_ms":8}))
            .prepare(&Config::default(), 2, 1000, 1)
            .unwrap(),
    ));
    let mut state = SceneBuilder::new(2).build();
    state.streamed_sources.push(source);
    mix_audio(&mut [0.; 64], &mut state);
    tx.push(0.25).unwrap();
    let mut from_start = [0.; 12];
    mix_audio(&mut from_start, &mut state);
    assert_eq!(&from_start[..2], &[0.25, 0.0]);
    tx.push(0.25).unwrap();
    let mut resumed = [0.; 2];
    mix_audio(&mut resumed, &mut state);
    let angle = std::f32::consts::FRAC_PI_2 * 6.0 / 8.0;
    assert!((resumed[0] - 0.25 * angle.cos()).abs() < 1e-6);
    assert!((resumed[1] - 0.25 * angle.sin()).abs() < 1e-6);
}
