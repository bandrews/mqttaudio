// ABOUTME: Control->audio command bridge for the real-time engine (Sprint 5).
// ABOUTME: Defines the SPSC command set + ring so the audio thread can own MixerState lock-free.

//! The control plane (MQTT/HTTP handlers) builds fully-resolved mutations and pushes
//! them onto a single-producer/single-consumer [`CommandProducer`]. The audio thread
//! drains them with [`drain_commands`] at the top of each callback and applies them to
//! the `MixerState` it owns — so the callback never locks a shared mutex. The expensive,
//! async, or allocating work (cache loads, voice-manager bookkeeping, channel-alias
//! resolution) stays on the control thread; only the finished value travels across the ring.

use crate::audio::ducking::DuckTargetChange;
use crate::audio::mixer::{ActiveSample, FadeState, LiveInput, MixerState, StreamedSource};
use crate::mqtt::commands::SampleSelector;
use ringbuf::{HeapConsumer, HeapProducer, HeapRb};

/// A fully-resolved mutation to apply to the audio thread's `MixerState`.
///
/// Every variant carries already-resolved data (selectors are matched on the audio
/// side, but ids/volumes/fades are computed by the control thread), so applying a
/// command never blocks, allocates unboundedly, or does I/O.
///
/// The move-in variants ([`AudioCommand::AddSample`]/[`AudioCommand::AddLiveInput`]/
/// [`AudioCommand::AddStreamedSource`]) carry their payload inline rather than boxed:
/// draining them moves the value straight into the mixer, freeing nothing on the
/// real-time thread. This makes the enum larger, but the command ring is
/// pre-allocated, so it is a one-time memory cost, not a per-callback allocation.
///
/// `large_enum_variant` is therefore allowed deliberately: boxing `AddSample` (as the
/// lint suggests) would put the sample on the heap and free it on the audio thread
/// when drained, re-introducing exactly the RT allocation Sprint 5 removed (and that
/// the allocation harness guards).
#[allow(clippy::large_enum_variant)]
pub enum AudioCommand {
    /// Start playing a fully-built sample. Ducking for the sample's voice is
    /// driven separately via [`AudioCommand::SetDuckTarget`].
    AddSample(ActiveSample),
    /// Apply a resolved ducking target change (computed on the control thread).
    SetDuckTarget(DuckTargetChange),
    /// Add a live (microphone) input to the mix.
    AddLiveInput(LiveInput),
    /// Start playing a fully-built windowed streamed source (long/large/live asset).
    AddStreamedSource(StreamedSource),
    /// Fade out every active sample and streamed source over `fade_ms` (Stop-all /
    /// shutdown).
    FadeOutAll { fade_ms: u32 },
    /// Fade out the samples whose internal id is in `ids` (voice stop / fade).
    FadeOutSamples { ids: Vec<u64>, fade_ms: u32 },
    /// Fade out the samples matching `selector` over `fade_ms`.
    FadeOutMatching {
        selector: SampleSelector,
        fade_ms: u32,
    },
    /// Set the target voice volume for samples and live inputs in `voice`.
    SetVoiceVolume { voice: String, volume: f32 },
    /// Set a live input's volume, selected by numeric index or by voice id.
    SetInputVolume { input: String, volume: f32 },
    /// Mute or unmute a live input, selected by numeric index or by voice id.
    /// Unmuting restores the volume the input had when it was muted (D34).
    SetInputMute { input: String, mute: bool },
    /// Seek the samples matching `selector` to `position_ms`.
    SeekMatching {
        selector: SampleSelector,
        position_ms: u64,
    },
    /// Set the playback speed (and pitch-correction mode) for matching samples.
    SetSpeedMatching {
        selector: SampleSelector,
        speed: f32,
        pitch_correction: bool,
    },
    /// Set the per-sample volume for matching samples.
    SetVolumeMatching {
        selector: SampleSelector,
        volume: f32,
    },
}

/// The producer half of the control->audio command ring (held by the control thread).
pub type CommandProducer = HeapProducer<AudioCommand>;
/// The consumer half of the control->audio command ring (held by the audio thread).
pub type CommandConsumer = HeapConsumer<AudioCommand>;

/// Create a bounded SPSC command ring with room for `capacity` queued commands.
pub fn command_channel(capacity: usize) -> (CommandProducer, CommandConsumer) {
    HeapRb::<AudioCommand>::new(capacity).split()
}

/// Producer half of the audio->reaper command-return ring (held by the audio thread).
pub type CommandReturnProducer = HeapProducer<AudioCommand>;
/// Consumer half of the command-return ring, drained by the off-RT reaper.
pub type CommandReturnConsumer = HeapConsumer<AudioCommand>;

/// Create the audio->reaper command-return ring. Spent mutation commands are moved
/// here by the callback (after their effect is applied) and dropped off the
/// real-time thread by the reaper, so their `String`/`Vec`/`SampleSelector` heap is
/// never freed in the callback. Mirrors the sample [`graveyard_channel`].
pub fn command_return_channel(capacity: usize) -> (CommandReturnProducer, CommandReturnConsumer) {
    HeapRb::<AudioCommand>::new(capacity).split()
}

/// Apply one resolved command to the audio thread's `MixerState`, consuming it.
///
/// `output_sample_rate` is needed to convert fade durations (ms) into sample counts.
///
/// This is the by-value entry point used by unit tests, which are not real-time:
/// it may drop the consumed command's heap (`String`/`Vec`/`SampleSelector`) itself.
/// The audio callback never calls this — it uses [`drain_commands`], which routes a
/// spent command's heap to a return ring for off-RT drop. Reachable from the library
/// API and the rt_engine unit tests; the binary's non-test code only uses
/// `drain_commands`, so it is dead there.
#[allow(dead_code)]
pub fn apply_command(state: &mut MixerState, cmd: AudioCommand, output_sample_rate: u32) {
    match cmd {
        AudioCommand::AddSample(sample) => {
            state.active_samples.push(sample);
        }
        AudioCommand::AddLiveInput(input) => {
            state.live_inputs.push(input);
        }
        AudioCommand::AddStreamedSource(source) => {
            state.streamed_sources.push(source);
        }
        other => apply_mutation(state, &other, output_sample_rate),
    }
}

/// Apply a read-only mutation command to the `MixerState` by reference.
///
/// Handles every variant that only *reads* its payload (selector/ids/voice/change)
/// and mutates the mixer: [`AudioCommand::FadeOutAll`], [`AudioCommand::FadeOutSamples`],
/// [`AudioCommand::FadeOutMatching`], [`AudioCommand::SetVoiceVolume`],
/// [`AudioCommand::SetInputVolume`], [`AudioCommand::SeekMatching`],
/// [`AudioCommand::SetSpeedMatching`], [`AudioCommand::SetVolumeMatching`], and
/// [`AudioCommand::SetDuckTarget`]. Because it borrows the command, the caller still
/// owns the heap-carrying husk afterward and can move it off the real-time thread for
/// drop instead of freeing it in the callback. The move-in variants
/// ([`AudioCommand::AddSample`]/[`AudioCommand::AddLiveInput`]/
/// [`AudioCommand::AddStreamedSource`]) are a no-op here: the drainer moves those
/// payloads straight into the mixer.
fn apply_mutation(state: &mut MixerState, cmd: &AudioCommand, output_sample_rate: u32) {
    match cmd {
        // Handled by move in `drain_commands`/`apply_command`, never by reference.
        AudioCommand::AddSample(_)
        | AudioCommand::AddLiveInput(_)
        | AudioCommand::AddStreamedSource(_) => {}
        AudioCommand::SetDuckTarget(change) => {
            if let Some(ref mut applier) = state.ducking_applier {
                applier.apply_target(change);
            }
        }
        AudioCommand::FadeOutAll { fade_ms } => {
            for sample in state.active_samples.iter_mut() {
                sample.set_fade(FadeState::fade_out(*fade_ms, output_sample_rate));
            }
            // Stop-all / shutdown fades windowed sources too.
            for source in state.streamed_sources.iter_mut() {
                source.set_fade(FadeState::fade_out(*fade_ms, output_sample_rate));
            }
        }
        AudioCommand::FadeOutSamples { ids, fade_ms } => {
            for sample in state.active_samples.iter_mut() {
                if ids.contains(&sample.id) {
                    sample.set_fade(FadeState::fade_out(*fade_ms, output_sample_rate));
                }
            }
            // A streamed source shares the sample id space (it is registered with the
            // voice manager), so a voice stop fades it the same way.
            for source in state.streamed_sources.iter_mut() {
                if ids.contains(&source.id) {
                    source.set_fade(FadeState::fade_out(*fade_ms, output_sample_rate));
                }
            }
        }
        AudioCommand::FadeOutMatching { selector, fade_ms } => {
            for sample in state.active_samples.iter_mut() {
                if sample_matches(selector, sample) {
                    sample.set_fade(FadeState::fade_out(*fade_ms, output_sample_rate));
                }
            }
            for source in state.streamed_sources.iter_mut() {
                if streamed_matches(selector, source) {
                    source.set_fade(FadeState::fade_out(*fade_ms, output_sample_rate));
                }
            }
        }
        AudioCommand::SetVoiceVolume { voice, volume } => {
            for sample in state.active_samples.iter_mut() {
                if sample.voice_id == *voice {
                    sample.set_target_voice_volume(*volume);
                }
            }
            for input in state.live_inputs.iter_mut() {
                if input.voice_id == *voice {
                    input.set_target_voice_volume(*volume);
                }
            }
            for source in state.streamed_sources.iter_mut() {
                if source.voice_id == *voice {
                    source.set_target_voice_volume(*volume);
                }
            }
        }
        AudioCommand::SetInputVolume { input, volume } => {
            apply_input_volume(state, input, *volume);
        }
        AudioCommand::SetInputMute { input, mute } => {
            apply_input_mute(state, input, *mute);
        }
        AudioCommand::SeekMatching {
            selector,
            position_ms,
        } => {
            for sample in state.active_samples.iter_mut() {
                if sample_matches(selector, sample) {
                    let target =
                        ((position_ms * sample.buffer.sample_rate() as u64) / 1000) as usize;
                    // Clamp against the total (or streaming estimate), matching
                    // `start_position_ms` (F6/D41): a forward seek into a still-loading
                    // region lands at the requested frame (the mixer returns silence
                    // until it loads) instead of snapping back to the loaded edge.
                    let max_frame = sample
                        .buffer
                        .total_frames_or_estimate()
                        .unwrap_or(usize::MAX)
                        .saturating_sub(1);
                    sample.position = target.min(max_frame);
                }
            }
        }
        AudioCommand::SetSpeedMatching {
            selector,
            speed,
            pitch_correction,
        } => {
            for sample in state.active_samples.iter_mut() {
                if sample_matches(selector, sample) {
                    sample.set_speed_with_mode(*speed, *pitch_correction);
                }
            }
        }
        AudioCommand::SetVolumeMatching { selector, volume } => {
            for sample in state.active_samples.iter_mut() {
                if sample_matches(selector, sample) {
                    sample.volume = volume.clamp(0.0, 1.0);
                }
            }
        }
    }
}

/// Drain up to `max` queued commands and apply them, returning how many were applied.
/// Bounded so a flood of commands can never make a single callback do unbounded work.
///
/// A heap-owning mutation command is applied by reference and then *moved* into
/// `returns` (the command-return ring) for drop on the off-RT reaper, so its
/// `String`/`Vec`/`SampleSelector` heap is never freed on the audio thread. The
/// move-in variants are moved straight into the mixer (no free either). If the
/// return ring is momentarily full the spent command is dropped in place as a
/// fallback (rare; the ring is sized generously).
pub fn drain_commands(
    consumer: &mut CommandConsumer,
    state: &mut MixerState,
    returns: &mut CommandReturnProducer,
    output_sample_rate: u32,
    max: usize,
) -> usize {
    let mut applied = 0;
    while applied < max {
        match consumer.pop() {
            Some(AudioCommand::AddSample(sample)) => {
                state.active_samples.push(sample);
            }
            Some(AudioCommand::AddLiveInput(input)) => {
                state.live_inputs.push(input);
            }
            Some(AudioCommand::AddStreamedSource(source)) => {
                state.streamed_sources.push(source);
            }
            Some(cmd) => {
                apply_mutation(state, &cmd, output_sample_rate);
                // Move the spent husk to the return ring; the reaper drops it off-RT.
                let _ = returns.push(cmd);
            }
            None => break,
        }
        applied += 1;
    }
    applied
}

/// Producer half of the audio->reaper return ring ("graveyard"), held by the audio thread.
pub type GraveyardProducer = HeapProducer<ActiveSample>;
/// Consumer half of the graveyard, drained by the off-RT reaper.
pub type GraveyardConsumer = HeapConsumer<ActiveSample>;

/// Create the audio->reaper return ring. Finished samples are moved here by the
/// callback and dropped off the real-time thread by the reaper.
pub fn graveyard_channel(capacity: usize) -> (GraveyardProducer, GraveyardConsumer) {
    HeapRb::<ActiveSample>::new(capacity).split()
}

/// The state the cpal output callback owns behind one `Arc<Mutex<>>`: the
/// `MixerState` it mixes, the command-ring consumer it drains, and the graveyard
/// producer it reaps finished samples into. The control thread never locks this;
/// it mutates only through the command ring and reads status from a snapshot, so
/// the lock is uncontended (held only by the callback and, briefly, the
/// supervisor during a device rebuild).
pub struct AudioCallbackState {
    /// The mixer the callback advances each block.
    pub mixer: MixerState,
    /// Control->audio command ring consumer, drained at the top of each callback.
    pub commands: CommandConsumer,
    /// Audio->reaper command-return ring producer. Spent heap-owning mutation
    /// commands are moved here for off-RT drop instead of being freed in the callback.
    pub command_returns: CommandReturnProducer,
    /// Audio->reaper return ring producer for off-RT drop of finished samples.
    pub graveyard: GraveyardProducer,
    /// Audio->reaper return ring producer for off-RT drop of finished streamed sources.
    pub streamed_graveyard: StreamedGraveyardProducer,
    /// Output sample rate, needed to convert fade durations (ms) into frames
    /// when draining commands.
    pub output_sample_rate: u32,
}

/// Move every finished sample out of `state` into the graveyard for off-RT drop,
/// returning how many were moved. The callback calls this instead of `retain`, so the
/// per-sample `channel_map`/`PitchCorrector` frees happen on the reaper thread, never
/// on the RT thread. If the graveyard is momentarily full, the finished sample is
/// *left in place* (not removed and not dropped) so the callback never frees a buffer;
/// it is reaped on a later block once the reaper has drained the ring (rare; the ring
/// is sized generously).
pub fn reap_finished(state: &mut MixerState, graveyard: &mut GraveyardProducer) -> usize {
    let mut moved = 0;
    let mut i = 0;
    while i < state.active_samples.len() {
        if state.active_samples[i].is_finished() {
            if graveyard.is_full() {
                // No room to hand off; leave the sample for a later block rather
                // than freeing it here. Skip past it so we keep scanning the rest.
                i += 1;
                continue;
            }
            let finished = state.active_samples.swap_remove(i);
            let _ = graveyard.push(finished);
            moved += 1;
        } else {
            i += 1;
        }
    }
    moved
}

/// Producer half of the audio->reaper return ring for finished streamed sources.
pub type StreamedGraveyardProducer = HeapProducer<StreamedSource>;
/// Consumer half of the streamed-source graveyard, drained by the off-RT reaper.
pub type StreamedGraveyardConsumer = HeapConsumer<StreamedSource>;

/// Create the audio->reaper return ring for finished streamed sources. A ring
/// separate from the sample [`graveyard_channel`] because the payload type differs;
/// both are drained by the same off-RT reaper. Moving a finished source here frees
/// its ring buffer and `Arc`s off the real-time thread.
pub fn streamed_graveyard_channel(
    capacity: usize,
) -> (StreamedGraveyardProducer, StreamedGraveyardConsumer) {
    HeapRb::<StreamedSource>::new(capacity).split()
}

/// Move every finished streamed source out of `state` into the graveyard for off-RT
/// drop, returning how many were moved. Mirrors [`reap_finished`]: if the graveyard is
/// momentarily full the finished source is left in place (not dropped on the RT
/// thread) and reaped on a later block.
pub fn reap_finished_streamed(
    state: &mut MixerState,
    graveyard: &mut StreamedGraveyardProducer,
) -> usize {
    let mut moved = 0;
    let mut i = 0;
    while i < state.streamed_sources.len() {
        if state.streamed_sources[i].is_finished() {
            if graveyard.is_full() {
                // No room to hand off; leave the source for a later block rather than
                // freeing its ring here. Skip past it so we keep scanning the rest.
                i += 1;
                continue;
            }
            let finished = state.streamed_sources.swap_remove(i);
            let _ = graveyard.push(finished);
            moved += 1;
        } else {
            i += 1;
        }
    }
    moved
}

/// Whether a sample matches a selector (matching is done on the audio side because the
/// control thread cannot see the live sample list without locking the state).
fn sample_matches(selector: &SampleSelector, sample: &ActiveSample) -> bool {
    selector.matches(
        sample.id,
        sample.sample_id.as_deref(),
        &sample.file_path,
        &sample.voice_id,
    )
}

/// Whether a streamed source matches a selector. A streamed source carries the same
/// id/sample_id/file/voice as a sample, so a selector-based fade reaches both.
fn streamed_matches(selector: &SampleSelector, source: &StreamedSource) -> bool {
    selector.matches(
        source.id,
        source.sample_id.as_deref(),
        &source.file_path,
        &source.voice_id,
    )
}

/// Resolve a live input by numeric index first, then by voice id, and apply `f` to
/// the first match. Shared by the input volume and mute handlers so both select an
/// input the same way.
fn with_live_input(state: &mut MixerState, input: &str, f: impl FnOnce(&mut LiveInput)) {
    if let Ok(idx) = input.parse::<usize>() {
        if let Some(live) = state.live_inputs.get_mut(idx) {
            f(live);
            return;
        }
    }
    if let Some(live) = state.live_inputs.iter_mut().find(|l| l.voice_id == input) {
        f(live);
    }
}

/// Set a live input's volume, selecting by numeric index first, then by voice id.
/// An explicit volume clears any muted state (D34).
fn apply_input_volume(state: &mut MixerState, input: &str, volume: f32) {
    with_live_input(state, input, |live| live.set_volume(volume));
}

/// Mute or unmute a live input, selecting by numeric index first, then by voice id.
/// Unmuting restores the input's pre-mute volume rather than 1.0 (D34).
fn apply_input_mute(state: &mut MixerState, input: &str, mute: bool) {
    with_live_input(state, input, |live| live.set_muted(mute));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::ducking::{DuckTargetChange, DuckingApplier};
    use crate::audio::types::DecodedBuffer;
    use std::sync::Arc;

    fn sample(id: u64, voice: &str, sample_id: Option<&str>, file: &str) -> ActiveSample {
        let buf = Arc::new(DecodedBuffer::new(vec![0.1f32; 200], 2, 48000)); // 100 frames
        ActiveSample::new_with_id(
            id,
            voice.to_string(),
            buf,
            1.0,
            1.0,
            file.to_string(),
            sample_id.map(|s| s.to_string()),
            false,
            0,
        )
    }

    fn state_with(samples: Vec<ActiveSample>) -> MixerState {
        let mut state = MixerState::new(2);
        state.active_samples = samples;
        state
    }

    fn selector_voice(voice: &str) -> SampleSelector {
        SampleSelector {
            internal_id: None,
            id: None,
            file: None,
            voice: Some(voice.to_string()),
        }
    }

    fn is_fading_out(sample: &ActiveSample) -> bool {
        matches!(sample.fade_state, FadeState::Out { .. })
    }

    #[test]
    fn add_sample_pushes_onto_state() {
        let mut state = state_with(vec![]);
        apply_command(
            &mut state,
            AudioCommand::AddSample(sample(1, "music", None, "a.wav")),
            48000,
        );
        assert_eq!(state.active_samples.len(), 1);
        assert_eq!(state.active_samples[0].id, 1);
    }

    #[test]
    fn set_duck_target_ducks_the_voice() {
        let mut state = state_with(vec![]);
        state.ducking_applier = Some(DuckingApplier::new());

        apply_command(
            &mut state,
            AudioCommand::SetDuckTarget(DuckTargetChange {
                voice: "music".to_string(),
                target_volume: 0.2,
                fade_frames: 0, // instant, for a deterministic assertion
            }),
            48000,
        );

        let applier = state.ducking_applier.as_mut().unwrap();
        let multiplier = applier.get_multiplier("music", 1);
        assert!(
            (multiplier - 0.2).abs() < 1e-4,
            "expected music ducked to 0.2, got {multiplier}"
        );
    }

    #[test]
    fn fade_out_all_fades_every_sample() {
        let mut state = state_with(vec![sample(1, "a", None, "a"), sample(2, "b", None, "b")]);
        apply_command(&mut state, AudioCommand::FadeOutAll { fade_ms: 10 }, 48000);
        assert!(state.active_samples.iter().all(is_fading_out));
    }

    #[test]
    fn fade_out_samples_only_matches_ids() {
        let mut state = state_with(vec![sample(1, "a", None, "a"), sample(2, "a", None, "a")]);
        apply_command(
            &mut state,
            AudioCommand::FadeOutSamples {
                ids: vec![2],
                fade_ms: 25,
            },
            48000,
        );
        assert!(!is_fading_out(&state.active_samples[0]));
        assert!(is_fading_out(&state.active_samples[1]));
    }

    #[test]
    fn fade_out_matching_uses_selector() {
        let mut state = state_with(vec![
            sample(1, "music", None, "a"),
            sample(2, "sfx", None, "b"),
        ]);
        apply_command(
            &mut state,
            AudioCommand::FadeOutMatching {
                selector: selector_voice("music"),
                fade_ms: 10,
            },
            48000,
        );
        assert!(is_fading_out(&state.active_samples[0]));
        assert!(!is_fading_out(&state.active_samples[1]));
    }

    #[test]
    fn set_voice_volume_targets_matching_voice() {
        let mut state = state_with(vec![
            sample(1, "music", None, "a"),
            sample(2, "sfx", None, "b"),
        ]);
        apply_command(
            &mut state,
            AudioCommand::SetVoiceVolume {
                voice: "music".to_string(),
                volume: 0.25,
            },
            48000,
        );
        assert_eq!(state.active_samples[0].target_voice_volume, 0.25);
        assert_eq!(state.active_samples[1].target_voice_volume, 1.0);
    }

    fn live_input(voice: &str, volume: f32) -> LiveInput {
        use ringbuf::HeapRb;
        let consumer = HeapRb::<f32>::new(16).split().1;
        LiveInput::new(voice.to_string(), consumer, 1, volume, vec![(0, 0)])
    }

    #[test]
    fn input_mute_then_unmute_restores_the_pre_mute_volume() {
        // D34: muting stores the current (calibrated) volume and zeroes it; unmuting
        // restores the stored value, NOT a hardcoded 1.0.
        let mut state = MixerState::new(2);
        state.live_inputs.push(live_input("mic", 0.7));

        // Mute by voice id: volume drops to 0.0 but the pre-mute 0.7 is remembered.
        apply_command(
            &mut state,
            AudioCommand::SetInputMute {
                input: "mic".to_string(),
                mute: true,
            },
            48000,
        );
        assert_eq!(state.live_inputs[0].volume, 0.0);

        // Unmute restores the stored 0.7, not 1.0.
        apply_command(
            &mut state,
            AudioCommand::SetInputMute {
                input: "mic".to_string(),
                mute: false,
            },
            48000,
        );
        assert_eq!(
            state.live_inputs[0].volume, 0.7,
            "unmute must restore the pre-mute volume (0.7), not 1.0"
        );
    }

    #[test]
    fn explicit_input_volume_while_muted_takes_effect_and_unmute_is_a_noop() {
        // Setting an explicit volume is the operator overriding the level; it clears
        // the muted state so a later unmute does not revert their change.
        let mut state = MixerState::new(2);
        state.live_inputs.push(live_input("mic", 0.7));

        apply_command(
            &mut state,
            AudioCommand::SetInputMute {
                input: "mic".to_string(),
                mute: true,
            },
            48000,
        );
        assert_eq!(state.live_inputs[0].volume, 0.0);

        // Explicit volume while muted: it applies immediately.
        apply_command(
            &mut state,
            AudioCommand::SetInputVolume {
                input: "0".to_string(),
                volume: 0.4,
            },
            48000,
        );
        assert_eq!(state.live_inputs[0].volume, 0.4);

        // A later unmute is a no-op: the explicit set already cleared the mute.
        apply_command(
            &mut state,
            AudioCommand::SetInputMute {
                input: "mic".to_string(),
                mute: false,
            },
            48000,
        );
        assert_eq!(
            state.live_inputs[0].volume, 0.4,
            "unmute after an explicit volume must not revert to the old pre-mute value"
        );
    }

    #[test]
    fn set_volume_matching_clamps() {
        let mut state = state_with(vec![sample(1, "music", None, "a")]);
        apply_command(
            &mut state,
            AudioCommand::SetVolumeMatching {
                selector: selector_voice("music"),
                volume: 2.0,
            },
            48000,
        );
        assert_eq!(state.active_samples[0].volume, 1.0);
    }

    #[test]
    fn seek_matching_sets_clamped_position() {
        let mut state = state_with(vec![sample(1, "music", None, "a")]);
        // 100-frame buffer @ 48kHz: 10_000 ms would be frame 480000, clamped to 99.
        apply_command(
            &mut state,
            AudioCommand::SeekMatching {
                selector: selector_voice("music"),
                position_ms: 10_000,
            },
            48000,
        );
        assert_eq!(state.active_samples[0].position, 99);
    }

    /// Build an ActiveSample over a streaming buffer that has only `loaded` frames
    /// decoded but an `estimate`d total, mirroring a still-downloading stream. The
    /// loaded edge and the estimate intentionally differ so a seek past the edge can
    /// be distinguished from a clamp to the loaded frames.
    fn streaming_sample(loaded: usize, estimate: usize) -> ActiveSample {
        use crate::audio::streaming::{SampleBuffer, StreamingBuffer};
        use std::sync::RwLock;
        let mut streaming = StreamingBuffer::new(2, 48000, Some(estimate));
        streaming.append(&vec![0.1f32; loaded * 2]); // `loaded` stereo frames
        let buf = SampleBuffer::Streaming(Arc::new(RwLock::new(streaming)));
        ActiveSample::new_with_id(
            1,
            "music".to_string(),
            buf,
            1.0,
            1.0,
            "s".to_string(),
            None,
            false,
            0,
        )
    }

    #[test]
    fn seek_on_streaming_buffer_clamps_to_estimate_not_loaded_edge() {
        // F6/D41: a forward seek into the not-yet-loaded region of a streaming buffer
        // must land at the requested frame (clamped only to the total estimate), the
        // same rule `start_position_ms` uses — not at the loaded edge. Here 100 frames
        // are loaded but the estimate is 100_000; a seek to 1000 ms (frame 48000) is
        // past the loaded edge yet within the estimate, so it must land at 48000, not
        // be clamped back to frame 99.
        let mut state = state_with(vec![streaming_sample(100, 100_000)]);
        apply_command(
            &mut state,
            AudioCommand::SeekMatching {
                selector: selector_voice("music"),
                position_ms: 1000,
            },
            48000,
        );
        assert_eq!(
            state.active_samples[0].position, 48000,
            "forward seek past the loaded edge must land at the requested frame (clamped to the estimate)"
        );
    }

    #[test]
    fn seek_past_estimate_clamps_to_estimate_edge() {
        // Beyond the estimated total, seek still clamps to the last estimated frame so
        // it can never run off the end. 100_000-frame estimate -> last frame 99_999.
        let mut state = state_with(vec![streaming_sample(100, 100_000)]);
        apply_command(
            &mut state,
            AudioCommand::SeekMatching {
                selector: selector_voice("music"),
                position_ms: 10_000, // frame 480_000, past the 100_000 estimate
            },
            48000,
        );
        assert_eq!(state.active_samples[0].position, 99_999);
    }

    #[test]
    fn set_speed_matching_changes_speed() {
        let mut state = state_with(vec![sample(1, "music", None, "a")]);
        apply_command(
            &mut state,
            AudioCommand::SetSpeedMatching {
                selector: selector_voice("music"),
                speed: 2.0,
                pitch_correction: false,
            },
            48000,
        );
        assert_eq!(state.active_samples[0].speed, 2.0);
    }

    #[test]
    fn ring_delivers_commands_in_order() {
        let (mut tx, mut rx) = command_channel(8);
        tx.push(AudioCommand::AddSample(sample(1, "music", None, "a")))
            .ok()
            .expect("push add");
        tx.push(AudioCommand::FadeOutAll { fade_ms: 10 })
            .ok()
            .expect("push fade");

        let mut state = state_with(vec![]);
        let (mut returns, _ret_rx) = command_return_channel(8);
        let applied = drain_commands(&mut rx, &mut state, &mut returns, 48000, 16);
        assert_eq!(applied, 2);
        assert_eq!(state.active_samples.len(), 1);
        assert!(is_fading_out(&state.active_samples[0]));
    }

    #[test]
    fn drain_routes_spent_mutation_to_return_ring() {
        // A heap-owning mutation command must be moved to the return ring after its
        // effect is applied, so the audio thread never drops its heap. The move-in
        // AddSample is consumed into the mixer and must NOT appear in the ring.
        let (mut tx, mut rx) = command_channel(8);
        tx.push(AudioCommand::AddSample(sample(1, "music", None, "a")))
            .ok()
            .expect("push add");
        tx.push(AudioCommand::FadeOutMatching {
            selector: selector_voice("music"),
            fade_ms: 10,
        })
        .ok()
        .expect("push fade");

        let mut state = state_with(vec![]);
        let (mut returns, mut ret_rx) = command_return_channel(8);
        let applied = drain_commands(&mut rx, &mut state, &mut returns, 48000, 16);

        assert_eq!(applied, 2);
        // The AddSample was moved into the mixer and its fade applied.
        assert_eq!(state.active_samples.len(), 1);
        assert!(is_fading_out(&state.active_samples[0]));
        // Exactly one spent husk (the FadeOutMatching) was returned for off-RT drop.
        assert!(
            matches!(ret_rx.pop(), Some(AudioCommand::FadeOutMatching { .. })),
            "the spent mutation command must be routed to the return ring"
        );
        assert!(
            ret_rx.pop().is_none(),
            "the move-in AddSample must not be routed to the return ring"
        );
    }

    #[test]
    fn reap_leaves_finished_sample_when_graveyard_full() {
        // With no room in the graveyard, a finished sample must stay in
        // active_samples (not be freed on the RT thread); it is reaped next block.
        let mut done = sample(1, "a", None, "a");
        done.position = 1000; // past the 100-frame buffer -> is_finished()
        let mut state = state_with(vec![done]);

        // A capacity-1 ring that is already full.
        let (mut tx, mut rx) = graveyard_channel(1);
        tx.push(sample(2, "b", None, "b")).ok().expect("fill ring");
        assert!(tx.is_full());

        let moved = reap_finished(&mut state, &mut tx);
        assert_eq!(moved, 0, "nothing can be handed off when the ring is full");
        assert_eq!(
            state.active_samples.len(),
            1,
            "the finished sample is left in place, not dropped on the RT thread"
        );
        assert_eq!(state.active_samples[0].id, 1);

        // Once the reaper drains the ring, the next reap hands the sample off.
        let _ = rx.pop();
        let moved = reap_finished(&mut state, &mut tx);
        assert_eq!(moved, 1);
        assert!(state.active_samples.is_empty());
        assert_eq!(rx.pop().map(|s| s.id), Some(1));
    }

    #[test]
    fn reap_moves_finished_samples_to_graveyard() {
        let playing = sample(1, "a", None, "a");
        let mut done = sample(2, "b", None, "b");
        done.position = 1000; // past the 100-frame buffer -> is_finished()
        let mut state = state_with(vec![playing, done]);

        let (mut tx, mut rx) = graveyard_channel(8);
        let moved = reap_finished(&mut state, &mut tx);

        assert_eq!(moved, 1);
        assert_eq!(state.active_samples.len(), 1);
        assert_eq!(state.active_samples[0].id, 1, "the playing sample stays");
        assert_eq!(
            rx.pop().map(|s| s.id),
            Some(2),
            "the finished sample is reaped"
        );
    }

    #[test]
    fn drain_respects_the_max_bound() {
        let (mut tx, mut rx) = command_channel(8);
        for _ in 0..5 {
            tx.push(AudioCommand::FadeOutAll { fade_ms: 10 })
                .ok()
                .expect("push");
        }
        let mut state = state_with(vec![]);
        let (mut returns, _ret_rx) = command_return_channel(8);
        // Only two drained this pass; the rest remain queued for the next callback.
        assert_eq!(
            drain_commands(&mut rx, &mut state, &mut returns, 48000, 2),
            2
        );
        assert_eq!(
            drain_commands(&mut rx, &mut state, &mut returns, 48000, 16),
            3
        );
    }

    /// Build a streamed source over a ring pre-filled with `data` (interleaved), with
    /// the EOF flag set to `producer_done`. An empty ring + EOF == finished.
    fn streamed_with(id: u64, voice: &str, data: &[f32], producer_done: bool) -> StreamedSource {
        use ringbuf::HeapRb;
        use std::sync::atomic::AtomicBool;
        let rb = HeapRb::<f32>::new(data.len() + 16);
        let (mut prod, consumer) = rb.split();
        prod.push_slice(data);
        drop(prod); // the consumer keeps the buffered data
        StreamedSource::new(
            id,
            voice.to_string(),
            "s.wav".to_string(),
            None,
            consumer,
            2,
            1.0,
            vec![(0, 0), (1, 1)],
            Arc::new(AtomicBool::new(producer_done)),
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn is_fading_out_streamed(source: &StreamedSource) -> bool {
        matches!(source.fade_state, FadeState::Out { .. })
    }

    #[test]
    fn add_streamed_source_pushes_onto_state() {
        let mut state = state_with(vec![]);
        apply_command(
            &mut state,
            AudioCommand::AddStreamedSource(streamed_with(1, "music", &[], false)),
            48000,
        );
        assert_eq!(state.streamed_sources.len(), 1);
        assert_eq!(state.streamed_sources[0].id, 1);
    }

    #[test]
    fn add_streamed_source_via_drain_moves_into_state() {
        let (mut tx, mut rx) = command_channel(8);
        tx.push(AudioCommand::AddStreamedSource(streamed_with(
            1,
            "music",
            &[],
            false,
        )))
        .ok()
        .expect("push add streamed");
        let mut state = state_with(vec![]);
        let (mut returns, mut ret_rx) = command_return_channel(8);
        let applied = drain_commands(&mut rx, &mut state, &mut returns, 48000, 16);
        assert_eq!(applied, 1);
        assert_eq!(state.streamed_sources.len(), 1);
        // A move-in command leaves nothing on the return ring (no heap to drop off-RT).
        assert!(ret_rx.pop().is_none());
    }

    #[test]
    fn fade_out_samples_fades_matching_streamed_source() {
        // A streamed source shares the sample id space (registered with the voice
        // manager), so a voice stop (FadeOutSamples by id) fades it too.
        let mut state = state_with(vec![]);
        state
            .streamed_sources
            .push(streamed_with(1, "music", &[0.5; 8], false));
        state
            .streamed_sources
            .push(streamed_with(2, "bed", &[0.5; 8], false));
        apply_command(
            &mut state,
            AudioCommand::FadeOutSamples {
                ids: vec![2],
                fade_ms: 10,
            },
            48000,
        );
        assert!(!is_fading_out_streamed(&state.streamed_sources[0]));
        assert!(is_fading_out_streamed(&state.streamed_sources[1]));
    }

    #[test]
    fn fade_out_matching_fades_streamed_source_by_voice() {
        let mut state = state_with(vec![]);
        state
            .streamed_sources
            .push(streamed_with(1, "music", &[0.5; 8], false));
        state
            .streamed_sources
            .push(streamed_with(2, "bed", &[0.5; 8], false));
        apply_command(
            &mut state,
            AudioCommand::FadeOutMatching {
                selector: selector_voice("bed"),
                fade_ms: 10,
            },
            48000,
        );
        assert!(!is_fading_out_streamed(&state.streamed_sources[0]));
        assert!(is_fading_out_streamed(&state.streamed_sources[1]));
    }

    #[test]
    fn fade_out_all_fades_streamed_sources_too() {
        let mut state = state_with(vec![]);
        state
            .streamed_sources
            .push(streamed_with(1, "music", &[0.5; 8], false));
        apply_command(&mut state, AudioCommand::FadeOutAll { fade_ms: 10 }, 48000);
        assert!(is_fading_out_streamed(&state.streamed_sources[0]));
    }

    #[test]
    fn set_voice_volume_targets_streamed_sources() {
        let mut state = state_with(vec![]);
        state
            .streamed_sources
            .push(streamed_with(1, "music", &[], false));
        apply_command(
            &mut state,
            AudioCommand::SetVoiceVolume {
                voice: "music".to_string(),
                volume: 0.3,
            },
            48000,
        );
        assert!((state.streamed_sources[0].target_voice_volume - 0.3).abs() < 1e-6);
    }

    #[test]
    fn reap_moves_finished_streamed_source_to_graveyard() {
        let mut state = state_with(vec![]);
        // Finished: EOF signalled and the ring is empty.
        state
            .streamed_sources
            .push(streamed_with(1, "music", &[], true));
        // Still playing: audio queued, no EOF.
        state
            .streamed_sources
            .push(streamed_with(2, "bed", &[0.5; 8], false));

        let (mut tx, mut rx) = streamed_graveyard_channel(8);
        let moved = reap_finished_streamed(&mut state, &mut tx);

        assert_eq!(moved, 1);
        assert_eq!(state.streamed_sources.len(), 1);
        assert_eq!(state.streamed_sources[0].id, 2, "the playing source stays");
        assert_eq!(
            rx.pop().map(|s| s.id),
            Some(1),
            "the finished one is reaped"
        );
    }

    #[test]
    fn reap_leaves_finished_streamed_source_when_graveyard_full() {
        let mut state = state_with(vec![]);
        state
            .streamed_sources
            .push(streamed_with(1, "music", &[], true));

        let (mut tx, mut rx) = streamed_graveyard_channel(1);
        tx.push(streamed_with(2, "x", &[], true))
            .ok()
            .expect("fill ring");
        assert!(tx.is_full());

        let moved = reap_finished_streamed(&mut state, &mut tx);
        assert_eq!(moved, 0, "nothing can be handed off when the ring is full");
        assert_eq!(
            state.streamed_sources.len(),
            1,
            "the finished source is left in place, not dropped on the RT thread"
        );

        // Once the reaper drains the ring, the next reap hands the source off.
        let _ = rx.pop();
        let moved = reap_finished_streamed(&mut state, &mut tx);
        assert_eq!(moved, 1);
        assert!(state.streamed_sources.is_empty());
        assert_eq!(rx.pop().map(|s| s.id), Some(1));
    }
}
