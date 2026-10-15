// ABOUTME: Property test: carrying commands onto plays that are still loading leaves every sound
// ABOUTME: as processing the same commands one at a time would, whatever order the loads finish in.

use mqttaudio::config::LoadMode;
use mqttaudio::mqtt::commands::{AudioCommand, SampleSelector};
use mqttaudio::pending_plays::{PendingPlays, PlayIdentity};
use proptest::prelude::*;
use proptest::test_runner::{Config as RunnerConfig, TestRunner};
use std::cell::Cell;
use std::collections::BTreeMap;

const VOICES: [&str; 3] = ["A", "B", "C"];
const IDS: [&str; 2] = ["x", "y"];
const FILES: [&str; 2] = ["f.wav", "g.wav"];

/// A selector over the small alphabets above. `internal` names a play by its
/// position in the stream, standing in for the internal id it gets when it starts.
#[derive(Clone, Debug)]
struct Sel {
    voice: Option<usize>,
    id: Option<usize>,
    file: Option<usize>,
    internal: Option<usize>,
}

#[derive(Clone, Debug)]
enum Op {
    Play {
        voice: Option<usize>,
        id: Option<usize>,
        file: usize,
        speed: Option<f32>,
        volume: Option<f32>,
        position: Option<u64>,
    },
    Speed {
        sel: Sel,
        speed: f32,
        pitch: bool,
    },
    Volume {
        sel: Sel,
        volume: f32,
    },
    Seek {
        sel: Sel,
        position: u64,
    },
    Stop {
        sel: Sel,
    },
    VoiceStop {
        voice: usize,
    },
    VoiceFadeOut {
        voice: usize,
    },
    VoiceVolume {
        voice: usize,
        volume: f32,
    },
    StopAll,
    /// Finish the load of the `which`-th unstarted play (modulo how many there are).
    Complete {
        which: usize,
        ok: bool,
    },
}

fn arb_sel() -> impl Strategy<Value = Sel> {
    (
        proptest::option::weighted(0.6, 0..VOICES.len()),
        proptest::option::weighted(0.3, 0..IDS.len()),
        proptest::option::weighted(0.3, 0..FILES.len()),
        proptest::option::weighted(0.15, 0..12usize),
    )
        .prop_map(|(voice, id, file, internal)| Sel {
            voice,
            id,
            file,
            internal,
        })
}

fn arb_op() -> impl Strategy<Value = Op> {
    let speed = prop_oneof![Just(0.5f32), Just(1.25), Just(2.0)];
    let volume = prop_oneof![Just(0.2f32), Just(0.6), Just(1.5)];
    let position = prop_oneof![Just(0u64), Just(250), Just(1000)];
    prop_oneof![
        5 => (
            proptest::option::weighted(0.8, 0..VOICES.len()),
            proptest::option::weighted(0.5, 0..IDS.len()),
            0..FILES.len(),
            proptest::option::weighted(0.2, speed.clone()),
            proptest::option::weighted(0.2, volume.clone()),
            proptest::option::weighted(0.2, position.clone()),
        )
            .prop_map(|(voice, id, file, speed, volume, position)| Op::Play {
                voice,
                id,
                file,
                speed,
                volume,
                position,
            }),
        2 => (arb_sel(), speed, any::<bool>())
            .prop_map(|(sel, speed, pitch)| Op::Speed { sel, speed, pitch }),
        2 => (arb_sel(), volume.clone()).prop_map(|(sel, volume)| Op::Volume { sel, volume }),
        1 => (arb_sel(), position).prop_map(|(sel, position)| Op::Seek { sel, position }),
        1 => arb_sel().prop_map(|sel| Op::Stop { sel }),
        1 => (0..VOICES.len()).prop_map(|voice| Op::VoiceStop { voice }),
        1 => (0..VOICES.len()).prop_map(|voice| Op::VoiceFadeOut { voice }),
        2 => (0..VOICES.len(), volume).prop_map(|(voice, volume)| Op::VoiceVolume { voice, volume }),
        1 => Just(Op::StopAll),
        5 => (0..8usize, proptest::bool::weighted(0.85))
            .prop_map(|(which, ok)| Op::Complete { which, ok }),
    ]
}

/// The state commands act on: every playing sound, keyed by the stream position
/// of the play that started it, and the volume of every voice that exists. A
/// voice exists while it has a sound or a play naming it is still loading.
#[derive(Clone, Debug, Default, PartialEq)]
struct World {
    sounds: BTreeMap<usize, Sound>,
    voice_volumes: BTreeMap<String, f32>,
    /// Plays still loading, per voice they name.
    loading: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, PartialEq)]
struct Sound {
    voice: String,
    id: Option<String>,
    file: String,
    speed: (f32, bool),
    volume: f32,
    position: Option<u64>,
}

impl World {
    /// Start the play at stream position `play` from its (possibly carried-over)
    /// command. A play without a voice gets one named after it.
    fn start(&mut self, play: usize, command: &AudioCommand) {
        let AudioCommand::Play {
            file,
            id,
            volume,
            voice,
            start_position_ms,
            speed,
            pitch_correction,
            ..
        } = command
        else {
            panic!("only plays start");
        };
        let voice = voice.clone().unwrap_or_else(|| format!("auto{play}"));
        self.voice_volumes.entry(voice.clone()).or_insert(1.0);
        self.sounds.insert(
            play,
            Sound {
                voice,
                id: id.clone(),
                file: file.clone(),
                speed: (*speed, *pitch_correction),
                volume: *volume,
                position: *start_position_ms,
            },
        );
    }

    /// Apply a command to the sounds that are playing.
    fn apply(&mut self, command: &AudioCommand) {
        let matches = |selector: &SampleSelector, play: usize, sound: &Sound| {
            selector.matches(play as u64, sound.id.as_deref(), &sound.file, &sound.voice)
        };
        match command {
            AudioCommand::Speed {
                selector,
                speed,
                pitch_correction,
            } => {
                for (play, sound) in self.sounds.iter_mut() {
                    if matches(selector, *play, sound) {
                        sound.speed = (*speed, *pitch_correction);
                    }
                }
            }
            AudioCommand::Volume { selector, volume } => {
                for (play, sound) in self.sounds.iter_mut() {
                    if matches(selector, *play, sound) {
                        sound.volume = *volume;
                    }
                }
            }
            AudioCommand::Seek {
                selector,
                position_ms,
            } => {
                for (play, sound) in self.sounds.iter_mut() {
                    if matches(selector, *play, sound) {
                        sound.position = Some(*position_ms);
                    }
                }
            }
            AudioCommand::Stop { selector, .. } => {
                self.sounds
                    .retain(|play, sound| !matches(selector, *play, sound));
            }
            AudioCommand::VoiceStop { voice } | AudioCommand::VoiceFadeOut { voice, .. } => {
                self.sounds.retain(|_, sound| &sound.voice != voice);
            }
            AudioCommand::VoiceVolume { voice, volume } => {
                if let Some(level) = self.voice_volumes.get_mut(voice) {
                    *level = *volume;
                }
            }
            AudioCommand::StopAll => self.sounds.clear(),
            other => panic!("not modelled: {other:?}"),
        }
        self.prune_empty_voices();
    }

    fn prune_empty_voices(&mut self) {
        let sounds = &self.sounds;
        let loading = &self.loading;
        self.voice_volumes.retain(|voice, _| {
            sounds.values().any(|sound| &sound.voice == voice) || loading.contains_key(voice)
        });
    }

    /// A play naming `voice` started loading: the voice exists until it ends.
    fn hold_open(&mut self, voice: &str) {
        self.voice_volumes.entry(voice.to_string()).or_insert(1.0);
        *self.loading.entry(voice.to_string()).or_insert(0) += 1;
    }

    /// A play naming `voice` stopped loading (it started, failed or was
    /// cancelled).
    fn let_go(&mut self, voice: &str) {
        let count = self
            .loading
            .get_mut(voice)
            .expect("a loading play holds the voice");
        *count -= 1;
        if *count == 0 {
            self.loading.remove(voice);
        }
        self.prune_empty_voices();
    }

    /// The sound of a play whose load failed, as one-at-a-time processing ends it.
    fn end(&mut self, play: usize) {
        self.sounds.remove(&play);
        self.prune_empty_voices();
    }

    /// Whether `voice` exists only because a play naming it is still loading.
    fn held_open_only(&self, voice: &str) -> bool {
        self.loading.contains_key(voice) && !self.sounds.values().any(|sound| sound.voice == voice)
    }

    fn is_playing(&self, play: usize) -> bool {
        self.sounds.contains_key(&play)
    }
}

fn play_command(op: &Op) -> AudioCommand {
    let Op::Play {
        voice,
        id,
        file,
        speed,
        volume,
        position,
    } = op
    else {
        panic!("not a play: {op:?}");
    };
    AudioCommand::Play {
        file: FILES[*file].to_string(),
        id: id.map(|i| IDS[i].to_string()),
        volume: volume.unwrap_or(1.0),
        voice: voice.map(|v| VOICES[v].to_string()),
        channel_map: None,
        fade_in: None,
        start_position_ms: *position,
        loop_mode: false,
        crossfade_ms: 0,
        mode: LoadMode::Auto,
        window_ms: None,
        prebuffer_ms: None,
        freshness: None,
        cacheable: None,
        speed: speed.unwrap_or(1.0),
        pitch_correction: false,
    }
}

/// The selector for `sel`, naming play `internal` by its internal id only when
/// that play is playing: a client can only learn an internal id once its sound
/// has started.
fn selector(sel: &Sel, world: &World) -> SampleSelector {
    SampleSelector {
        internal_id: sel
            .internal
            .filter(|play| world.is_playing(*play))
            .map(|play| play.to_string()),
        id: sel.id.map(|i| IDS[i].to_string()),
        file: sel.file.map(|f| FILES[f].to_string()),
        voice: sel.voice.map(|v| VOICES[v].to_string()),
    }
}

/// The command for a non-play, non-completion op, built against `world` (which
/// decides whether its internal id is nameable yet).
fn command(op: &Op, world: &World) -> AudioCommand {
    match op {
        Op::Speed { sel, speed, pitch } => AudioCommand::Speed {
            selector: selector(sel, world),
            speed: *speed,
            pitch_correction: *pitch,
        },
        Op::Volume { sel, volume } => AudioCommand::Volume {
            selector: selector(sel, world),
            volume: *volume,
        },
        Op::Seek { sel, position } => AudioCommand::Seek {
            selector: selector(sel, world),
            position_ms: *position,
        },
        Op::Stop { sel } => AudioCommand::Stop {
            selector: selector(sel, world),
            fade_out_ms: None,
        },
        Op::VoiceStop { voice } => AudioCommand::VoiceStop {
            voice: VOICES[*voice].to_string(),
        },
        Op::VoiceFadeOut { voice } => AudioCommand::VoiceFadeOut {
            voice: VOICES[*voice].to_string(),
            time_ms: 1000,
        },
        Op::VoiceVolume { voice, volume } => AudioCommand::VoiceVolume {
            voice: VOICES[*voice].to_string(),
            volume: *volume,
        },
        Op::StopAll => AudioCommand::StopAll,
        Op::Play { .. } | Op::Complete { .. } => panic!("not a command: {op:?}"),
    }
}

/// How often the interesting paths ran across all cases, so the property cannot
/// pass because nothing ever waited.
#[derive(Default)]
struct Coverage {
    carried: Cell<usize>,
    cancelled: Cell<usize>,
    failed: Cell<usize>,
    started_with_carried: Cell<usize>,
    voice_volume_on_loading_voice: Cell<usize>,
    failed_holding_voice: Cell<usize>,
    voiceless_started_with_carried: Cell<usize>,
    internal_named: Cell<usize>,
}

fn bump(counter: &Cell<usize>, by: usize) {
    counter.set(counter.get() + by);
}

/// One step of the stream as one-at-a-time processing sees it. A play whose load
/// fails counts as a sound from its arrival until the failure, because it held
/// its voice open while it loaded.
#[derive(Clone, Debug)]
enum Replay {
    Play(usize, AudioCommand),
    LoadFailed(usize),
    Command(AudioCommand),
}

/// Runs a stream the way the scheduler does: commands act on the sounds playing
/// when they arrive and carry onto (or cancel) the plays still loading, and each
/// play starts with its carried settings when its load finishes.
struct CarryOver<'a> {
    world: World,
    pending: PendingPlays<usize>,
    /// The pending key of each play still loading, by its stream position.
    loading: BTreeMap<usize, u64>,
    plays: BTreeMap<usize, AudioCommand>,
    /// The stream as one-at-a-time processing must replay it.
    replay: Vec<Replay>,
    coverage: &'a Coverage,
}

impl<'a> CarryOver<'a> {
    fn new(coverage: &'a Coverage) -> Self {
        Self {
            world: World::default(),
            pending: PendingPlays::new(),
            loading: BTreeMap::new(),
            plays: BTreeMap::new(),
            replay: Vec::new(),
            coverage,
        }
    }

    fn run(mut self, ops: &[Op]) -> (World, Vec<Replay>) {
        for (index, op) in ops.iter().enumerate() {
            match op {
                Op::Play { .. } => self.arrive_play(index, play_command(op)),
                Op::Complete { which, ok } => {
                    if let Some(index) = self.nth_loading(*which) {
                        self.finish(index, *ok);
                    }
                }
                _ => {
                    let command = command(op, &self.world);
                    self.arrive_command(command);
                }
            }
        }
        // Every load still running finishes, in arrival order.
        while let Some(index) = self.nth_loading(0) {
            self.finish(index, true);
        }
        (self.world, self.replay)
    }

    fn arrive_play(&mut self, index: usize, command: AudioCommand) {
        let identity = PlayIdentity::of(&command).expect("a play has an identity");
        if let Some(voice) = &identity.voice {
            self.world.hold_open(voice);
        }
        let key = self.pending.insert(identity, index);
        self.loading.insert(index, key);
        self.plays.insert(index, command.clone());
        self.replay.push(Replay::Play(index, command));
    }

    fn arrive_command(&mut self, command: AudioCommand) {
        if let AudioCommand::Speed { selector, .. }
        | AudioCommand::Volume { selector, .. }
        | AudioCommand::Seek { selector, .. }
        | AudioCommand::Stop { selector, .. } = &command
        {
            if selector.internal_id.is_some() {
                bump(&self.coverage.internal_named, 1);
            }
        }
        let cancelled = if matches!(command, AudioCommand::StopAll) {
            self.pending.take_all()
        } else {
            self.pending.cancel(&command)
        };
        bump(&self.coverage.cancelled, cancelled.len());
        for play in cancelled {
            self.loading.remove(&play.payload);
            if let Some(voice) = &play.identity.voice {
                self.world.let_go(voice);
            }
        }
        let carried = self.pending.carry(&command);
        bump(&self.coverage.carried, carried);
        if let AudioCommand::VoiceVolume { voice, .. } = &command {
            if self.world.held_open_only(voice) {
                bump(&self.coverage.voice_volume_on_loading_voice, 1);
            }
        }
        self.world.apply(&command);
        self.replay.push(Replay::Command(command));
    }

    fn nth_loading(&self, which: usize) -> Option<usize> {
        if self.loading.is_empty() {
            return None;
        }
        self.loading.keys().nth(which % self.loading.len()).copied()
    }

    fn finish(&mut self, index: usize, ok: bool) {
        let key = self.loading.remove(&index).expect("a loading play");
        let play = self.pending.remove(key).expect("a pending play");
        if !ok {
            bump(&self.coverage.failed, 1);
            if let Some(voice) = &play.identity.voice {
                bump(&self.coverage.failed_holding_voice, 1);
                self.world.let_go(voice);
            }
            self.replay.push(Replay::LoadFailed(index));
            return;
        }
        if play.carried != Default::default() {
            bump(&self.coverage.started_with_carried, 1);
            if play.identity.voice.is_none() {
                bump(&self.coverage.voiceless_started_with_carried, 1);
            }
        }
        let mut start = self.plays[&index].clone();
        play.carried.apply_to(&mut start);
        self.world.start(index, &start);
        if let Some(voice) = &play.identity.voice {
            self.world.let_go(voice);
        }
    }
}

/// Process the stream one command at a time, each play starting the moment it
/// arrives.
fn run_one_at_a_time(replay: &[Replay]) -> World {
    let mut world = World::default();
    for step in replay {
        match step {
            Replay::Play(index, command) => world.start(*index, command),
            Replay::LoadFailed(index) => world.end(*index),
            Replay::Command(command) => world.apply(command),
        }
    }
    world
}

#[test]
fn carry_over_leaves_every_sound_as_one_at_a_time_processing_would() {
    let coverage = Coverage::default();
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|cases| cases.parse().ok())
        .unwrap_or(2000);
    let mut runner = TestRunner::new(RunnerConfig {
        cases,
        ..RunnerConfig::default()
    });
    let result = runner.run(&proptest::collection::vec(arb_op(), 1..40), |ops| {
        let (carried_over, replay) = CarryOver::new(&coverage).run(&ops);
        let one_at_a_time = run_one_at_a_time(&replay);
        prop_assert_eq!(carried_over, one_at_a_time, "ops: {:?}", ops);
        Ok(())
    });
    if let Err(failure) = result {
        panic!("{failure}");
    }

    for (path, count) in [
        ("carried a setting", coverage.carried.get()),
        ("cancelled a play", coverage.cancelled.get()),
        ("failed a load", coverage.failed.get()),
        (
            "started a play with carried settings",
            coverage.started_with_carried.get(),
        ),
        (
            "set the volume of a voice held open by a loading play",
            coverage.voice_volume_on_loading_voice.get(),
        ),
        (
            "failed a play that held its voice open",
            coverage.failed_holding_voice.get(),
        ),
        (
            "started a play with no voice with carried settings",
            coverage.voiceless_started_with_carried.get(),
        ),
        (
            "named a sound by internal id",
            coverage.internal_named.get(),
        ),
    ] {
        assert!(
            count > 0,
            "no generated case {path}; the property is vacuous"
        );
    }
}
