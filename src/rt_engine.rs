// ABOUTME: Control->audio command bridge for the real-time engine (Sprint 5).
// ABOUTME: Defines the SPSC command set + ring so the audio thread can own MixerState lock-free.

//! The control plane (MQTT/HTTP handlers) builds fully-resolved mutations and pushes
//! them onto a single-producer/single-consumer [`CommandProducer`]. The audio thread
//! drains them with [`drain_commands`] at the top of each callback and applies them to
//! the `MixerState` it owns — so the callback never locks a shared mutex. The expensive,
//! async, or allocating work (cache loads, voice-manager bookkeeping, channel-alias
//! resolution) stays on the control thread; only the finished value travels across the ring.

use crate::audio::ducking::DuckTargetChange;
use crate::audio::mixer::{ActiveSample, FadeState, LiveInput, MixerState};
use crate::mqtt::commands::SampleSelector;
use ringbuf::{HeapConsumer, HeapProducer, HeapRb};

/// A fully-resolved mutation to apply to the audio thread's `MixerState`.
///
/// Every variant carries already-resolved data (selectors are matched on the audio
/// side, but ids/volumes/fades are computed by the control thread), so applying a
/// command never blocks, allocates unboundedly, or does I/O.
///
/// The move-in variants ([`AudioCommand::AddSample`]/[`AudioCommand::AddLiveInput`])
/// carry their payload inline rather than boxed: draining them moves the value
/// straight into the mixer, freeing nothing on the real-time thread. This makes the
/// enum larger, but the command ring is pre-allocated, so it is a one-time memory
/// cost, not a per-callback allocation.
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
    /// Fade out every active sample over `fade_ms` (Stop-all / shutdown).
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
/// ([`AudioCommand::AddSample`]/[`AudioCommand::AddLiveInput`]) are a no-op here: the
/// drainer moves those payloads straight into the mixer.
fn apply_mutation(state: &mut MixerState, cmd: &AudioCommand, output_sample_rate: u32) {
    match cmd {
        // Handled by move in `drain_commands`/`apply_command`, never by reference.
        AudioCommand::AddSample(_) | AudioCommand::AddLiveInput(_) => {}
        AudioCommand::SetDuckTarget(change) => {
            if let Some(ref mut applier) = state.ducking_applier {
                applier.apply_target(change);
            }
        }
        AudioCommand::FadeOutAll { fade_ms } => {
            for sample in state.active_samples.iter_mut() {
                sample.set_fade(FadeState::fade_out(*fade_ms, output_sample_rate));
            }
        }
        AudioCommand::FadeOutSamples { ids, fade_ms } => {
            for sample in state.active_samples.iter_mut() {
                if ids.contains(&sample.id) {
                    sample.set_fade(FadeState::fade_out(*fade_ms, output_sample_rate));
                }
            }
        }
        AudioCommand::FadeOutMatching { selector, fade_ms } => {
            for sample in state.active_samples.iter_mut() {
                if sample_matches(selector, sample) {
                    sample.set_fade(FadeState::fade_out(*fade_ms, output_sample_rate));
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
        }
        AudioCommand::SetInputVolume { input, volume } => {
            apply_input_volume(state, input, *volume);
        }
        AudioCommand::SeekMatching {
            selector,
            position_ms,
        } => {
            for sample in state.active_samples.iter_mut() {
                if sample_matches(selector, sample) {
                    let target =
                        ((position_ms * sample.buffer.sample_rate() as u64) / 1000) as usize;
                    sample.position = target.min(sample.buffer.frames().saturating_sub(1));
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

/// Set a live input's volume, selecting by numeric index first, then by voice id.
/// Mirrors the existing InputVolume/InputMute handler resolution.
fn apply_input_volume(state: &mut MixerState, input: &str, volume: f32) {
    let clamped = volume.clamp(0.0, 1.0);
    if let Ok(idx) = input.parse::<usize>() {
        if let Some(live) = state.live_inputs.get_mut(idx) {
            live.volume = clamped;
            return;
        }
    }
    for live in state.live_inputs.iter_mut() {
        if live.voice_id == input {
            live.volume = clamped;
            return;
        }
    }
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
}
