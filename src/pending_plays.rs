// ABOUTME: Plays whose audio is still loading, and the commands that reach them before they start.
// ABOUTME: Setting commands carry over onto a matching play; stops and fades cancel it.

use crate::mqtt::commands::{AudioCommand, VoiceTarget};
use std::collections::BTreeMap;

/// What a command can select a play by before it starts.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayIdentity {
    /// The voice the play names. A play without one gets its voice when it
    /// starts, so no command can name it before then.
    pub voice: Option<String>,
    pub id: Option<String>,
    pub file: String,
}

impl PlayIdentity {
    /// The identity of a play command, or `None` for any other command.
    pub fn of(command: &AudioCommand) -> Option<Self> {
        match command {
            AudioCommand::Play {
                file, id, voice, ..
            } => Some(Self {
                voice: voice.clone(),
                id: id.clone(),
                file: file.clone(),
            }),
            _ => None,
        }
    }

    /// Whether `command` would reach the sound this play starts: `seek`, `speed`,
    /// `stop` and `volume` by their selector, `voice_stop`, `voice_fade_out`,
    /// `voice_speed` and `voice_clear` by the voice the play names, and
    /// `voice_clear` of every voice and `reset` always. No other command reaches
    /// a play.
    fn reached_by(&self, command: &AudioCommand) -> bool {
        match command {
            AudioCommand::Seek { selector, .. }
            | AudioCommand::Speed { selector, .. }
            | AudioCommand::Stop { selector, .. }
            | AudioCommand::Volume { selector, .. } => {
                selector.matches_unstarted(self.id.as_deref(), &self.file, self.voice.as_deref())
            }
            AudioCommand::VoiceStop { voice }
            | AudioCommand::VoiceFadeOut { voice, .. }
            | AudioCommand::VoiceSpeed { voice, .. } => {
                self.voice.as_deref() == Some(voice.as_str())
            }
            AudioCommand::VoiceClear {
                voice: VoiceTarget::Named(voice),
            } => self.voice.as_deref() == Some(voice.as_str()),
            AudioCommand::VoiceClear {
                voice: VoiceTarget::All,
            }
            | AudioCommand::Reset => true,
            _ => false,
        }
    }
}

/// Settings that commands carried onto a play before it started. The play starts
/// with them, as if each command had reached it right after it started.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CarriedSettings {
    /// Speed and pitch correction, from `speed` or `voice_speed`.
    pub speed: Option<(f32, bool)>,
    /// The sound's own volume, from `volume`.
    pub volume: Option<f32>,
    /// Start position in milliseconds, from `seek`.
    pub position_ms: Option<u64>,
}

impl CarriedSettings {
    /// Write the carried speed, volume and position into a play command's start
    /// parameters.
    pub fn apply_to(&self, command: &mut AudioCommand) {
        if let AudioCommand::Play {
            speed,
            pitch_correction,
            volume,
            start_position_ms,
            ..
        } = command
        {
            if let Some((carried_speed, carried_pitch)) = self.speed {
                *speed = Some(carried_speed);
                *pitch_correction = carried_pitch;
            }
            if let Some(carried_volume) = self.volume {
                *volume = carried_volume.clamp(0.0, crate::config::MAX_GAIN);
            }
            if let Some(position_ms) = self.position_ms {
                *start_position_ms = Some(position_ms);
            }
        }
    }
}

/// A play that has not started.
#[derive(Debug)]
pub struct PendingPlay<T> {
    pub identity: PlayIdentity,
    pub carried: CarriedSettings,
    /// Whatever the caller keeps with the play until it starts.
    pub payload: T,
}

/// The plays received but not yet started, in arrival order.
#[derive(Debug)]
pub struct PendingPlays<T> {
    plays: BTreeMap<u64, PendingPlay<T>>,
    next_key: u64,
}

impl<T> Default for PendingPlays<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> PendingPlays<T> {
    pub fn new() -> Self {
        Self {
            plays: BTreeMap::new(),
            next_key: 0,
        }
    }

    /// Record a play that has not started. The returned key names it until it
    /// starts, fails or is cancelled.
    pub fn insert(&mut self, identity: PlayIdentity, payload: T) -> u64 {
        let key = self.next_key;
        self.next_key += 1;
        self.plays.insert(
            key,
            PendingPlay {
                identity,
                carried: CarriedSettings::default(),
                payload,
            },
        );
        key
    }

    /// Remove a play that is starting, failed, or whose client has gone.
    pub fn remove(&mut self, key: u64) -> Option<PendingPlay<T>> {
        self.plays.remove(&key)
    }

    /// Remove every play, in arrival order.
    pub fn take_all(&mut self) -> Vec<PendingPlay<T>> {
        std::mem::take(&mut self.plays).into_values().collect()
    }

    pub fn len(&self) -> usize {
        self.plays.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plays.is_empty()
    }

    /// How many plays `command` reaches, whether or not it carries onto or
    /// cancels them.
    pub fn reached_by(&self, command: &AudioCommand) -> usize {
        self.plays
            .values()
            .filter(|play| play.identity.reached_by(command))
            .count()
    }

    /// Carry a setting command (`speed`, `voice_speed`, `volume`, `seek`) onto
    /// every play it reaches and return how many that is. Any other command
    /// carries nowhere; `voice_volume` acts on the voice itself, which a loading
    /// play keeps open.
    pub fn carry(&mut self, command: &AudioCommand) -> usize {
        let mut carried_onto = 0;
        for play in self.plays.values_mut() {
            if !play.identity.reached_by(command) {
                continue;
            }
            let carried = &mut play.carried;
            match command {
                AudioCommand::Speed {
                    speed,
                    pitch_correction,
                    ..
                }
                | AudioCommand::VoiceSpeed {
                    speed,
                    pitch_correction,
                    ..
                } => carried.speed = Some((*speed, *pitch_correction)),
                AudioCommand::Volume { volume, .. } => carried.volume = Some(*volume),
                AudioCommand::Seek { position_ms, .. } => carried.position_ms = Some(*position_ms),
                _ => continue,
            }
            carried_onto += 1;
        }
        carried_onto
    }

    /// Remove and return, in arrival order, the plays a stop or fade (`stop`,
    /// `voice_stop`, `voice_fade_out`, `voice_clear`, `reset`) cancels. Any other
    /// command cancels none.
    pub fn cancel(&mut self, command: &AudioCommand) -> Vec<PendingPlay<T>> {
        let cancels = matches!(
            command,
            AudioCommand::Stop { .. }
                | AudioCommand::VoiceStop { .. }
                | AudioCommand::VoiceFadeOut { .. }
                | AudioCommand::VoiceClear { .. }
                | AudioCommand::Reset
        );
        if !cancels {
            return Vec::new();
        }
        let keys: Vec<u64> = self
            .plays
            .iter()
            .filter(|(_, play)| play.identity.reached_by(command))
            .map(|(key, _)| *key)
            .collect();
        keys.into_iter()
            .filter_map(|key| self.plays.remove(&key))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LoadMode;
    use crate::mqtt::commands::SampleSelector;

    fn identity(voice: Option<&str>, id: Option<&str>, file: &str) -> PlayIdentity {
        PlayIdentity {
            voice: voice.map(str::to_string),
            id: id.map(str::to_string),
            file: file.to_string(),
        }
    }

    fn selector(voice: Option<&str>, id: Option<&str>, file: Option<&str>) -> SampleSelector {
        SampleSelector {
            internal_id: None,
            id: id.map(str::to_string),
            file: file.map(str::to_string),
            voice: voice.map(str::to_string),
        }
    }

    fn speed(selector: SampleSelector, speed: f32) -> AudioCommand {
        AudioCommand::Speed {
            selector,
            speed,
            pitch_correction: false,
        }
    }

    fn play_command(voice: Option<&str>, id: Option<&str>, file: &str) -> AudioCommand {
        AudioCommand::Play {
            file: file.to_string(),
            id: id.map(str::to_string),
            volume: 1.0,
            voice: voice.map(str::to_string),
            channel_map: None,
            fade_in: None,
            start_position_ms: None,
            loop_mode: false,
            crossfade_ms: 0,
            mode: LoadMode::Auto,
            window_ms: None,
            prebuffer_ms: None,
            freshness: None,
            cacheable: None,
            speed: None,
            pitch_correction: false,
        }
    }

    /// Three plays: "rain" in voice amb (id r), "wind" in voice amb, "bell" with
    /// no voice (id b).
    fn three_plays() -> (PendingPlays<&'static str>, [u64; 3]) {
        let mut plays = PendingPlays::new();
        let rain = plays.insert(identity(Some("amb"), Some("r"), "rain.wav"), "rain");
        let wind = plays.insert(identity(Some("amb"), None, "wind.wav"), "wind");
        let bell = plays.insert(identity(None, Some("b"), "bell.wav"), "bell");
        (plays, [rain, wind, bell])
    }

    #[test]
    fn identity_of_a_play_is_its_voice_id_and_file() {
        let identity_of = PlayIdentity::of(&play_command(Some("amb"), Some("r"), "rain.wav"));
        assert_eq!(
            identity_of,
            Some(identity(Some("amb"), Some("r"), "rain.wav"))
        );
        assert_eq!(PlayIdentity::of(&AudioCommand::StopAll), None);
    }

    #[test]
    fn plays_are_removed_by_key_and_taken_in_arrival_order() {
        let (mut plays, [rain, wind, bell]) = three_plays();
        assert_eq!(plays.len(), 3);

        assert_eq!(plays.remove(wind).map(|p| p.payload), Some("wind"));
        assert!(plays.remove(wind).is_none(), "a removed play is gone");

        let rest: Vec<_> = plays.take_all().into_iter().map(|p| p.payload).collect();
        assert_eq!(rest, vec!["rain", "bell"]);
        assert!(plays.is_empty());
        assert!(plays.remove(rain).is_none());
        assert!(plays.remove(bell).is_none());
    }

    #[test]
    fn a_selector_reaches_plays_by_voice_id_or_file() {
        let (mut plays, [rain, wind, bell]) = three_plays();

        assert_eq!(
            plays.carry(&speed(selector(Some("amb"), None, None), 1.5)),
            2
        );
        assert_eq!(plays.carry(&speed(selector(None, Some("b"), None), 0.5)), 1);
        assert_eq!(
            plays.carry(&speed(selector(None, None, Some("wind.wav")), 2.0)),
            1
        );

        let speed_of = |key| plays.plays.get(&key).unwrap().carried.speed;
        assert_eq!(speed_of(rain), Some((1.5, false)));
        assert_eq!(
            speed_of(wind),
            Some((2.0, false)),
            "a later speed replaces an earlier one"
        );
        assert_eq!(speed_of(bell), Some((0.5, false)));
    }

    #[test]
    fn a_selector_matching_any_one_criterion_reaches_the_play() {
        let (mut plays, [rain, wind, bell]) = three_plays();
        // voice "x" matches nothing, id "b" matches bell: OR, not AND.
        assert_eq!(
            plays.carry(&speed(selector(Some("x"), Some("b"), None), 1.2)),
            1
        );
        assert_eq!(
            plays.plays.get(&bell).unwrap().carried.speed,
            Some((1.2, false))
        );
        assert_eq!(plays.plays.get(&rain).unwrap().carried.speed, None);
        assert_eq!(plays.plays.get(&wind).unwrap().carried.speed, None);
    }

    #[test]
    fn an_internal_id_never_reaches_an_unstarted_play() {
        let (mut plays, _) = three_plays();
        let by_internal_id = SampleSelector {
            internal_id: Some("0".to_string()),
            id: None,
            file: None,
            voice: None,
        };
        assert_eq!(plays.carry(&speed(by_internal_id, 1.5)), 0);
    }

    #[test]
    fn a_voice_selector_cannot_reach_a_play_without_a_voice() {
        let (mut plays, [_, _, bell]) = three_plays();
        // The voice-less play has no voice to match until it starts.
        assert_eq!(plays.carry(&speed(selector(Some(""), None, None), 1.5)), 0);
        assert_eq!(plays.plays.get(&bell).unwrap().carried.speed, None);
    }

    #[test]
    fn each_setting_command_carries_its_own_setting() {
        let (mut plays, [rain, _, _]) = three_plays();
        let rain_selector = || selector(None, Some("r"), None);

        plays.carry(&AudioCommand::Speed {
            selector: rain_selector(),
            speed: 1.25,
            pitch_correction: true,
        });
        plays.carry(&AudioCommand::Volume {
            selector: rain_selector(),
            volume: 0.4,
        });
        plays.carry(&AudioCommand::Seek {
            selector: rain_selector(),
            position_ms: 750,
        });

        assert_eq!(
            plays.plays.get(&rain).unwrap().carried,
            CarriedSettings {
                speed: Some((1.25, true)),
                volume: Some(0.4),
                position_ms: Some(750),
            }
        );
    }

    #[test]
    fn commands_that_set_nothing_carry_nowhere() {
        let (mut plays, keys) = three_plays();
        let everything = selector(Some("amb"), Some("b"), None);
        for command in [
            AudioCommand::Stop {
                selector: everything.clone(),
                fade_out_ms: None,
            },
            AudioCommand::VoiceStop {
                voice: "amb".to_string(),
            },
            AudioCommand::VoiceVolume {
                voice: "amb".to_string(),
                volume: 0.3,
            },
            AudioCommand::StopAll,
            AudioCommand::FadeAll { time_ms: 100 },
            AudioCommand::InputVolume {
                input: "amb".to_string(),
                volume: 0.5,
            },
            play_command(Some("amb"), Some("b"), "rain.wav"),
        ] {
            assert_eq!(plays.carry(&command), 0, "{command:?} carries nowhere");
        }
        for key in keys {
            assert_eq!(
                plays.plays.get(&key).unwrap().carried,
                CarriedSettings::default()
            );
        }
    }

    #[test]
    fn a_stop_cancels_the_plays_its_selector_matches() {
        let (mut plays, _) = three_plays();
        let cancelled: Vec<_> = plays
            .cancel(&AudioCommand::Stop {
                selector: selector(None, Some("b"), Some("rain.wav")),
                fade_out_ms: Some(2000),
            })
            .into_iter()
            .map(|p| p.payload)
            .collect();
        assert_eq!(cancelled, vec!["rain", "bell"], "in arrival order");
        let left: Vec<_> = plays.take_all().into_iter().map(|p| p.payload).collect();
        assert_eq!(left, vec!["wind"]);
    }

    #[test]
    fn voice_stop_and_voice_fade_out_cancel_plays_naming_their_voice() {
        for command in [
            AudioCommand::VoiceStop {
                voice: "amb".to_string(),
            },
            AudioCommand::VoiceFadeOut {
                voice: "amb".to_string(),
                time_ms: 3000,
            },
        ] {
            let (mut plays, _) = three_plays();
            let cancelled: Vec<_> = plays
                .cancel(&command)
                .into_iter()
                .map(|p| p.payload)
                .collect();
            assert_eq!(cancelled, vec!["rain", "wind"], "{command:?}");
            assert_eq!(plays.len(), 1);
        }
    }

    #[test]
    fn commands_that_stop_nothing_cancel_nothing() {
        let (mut plays, _) = three_plays();
        let everything = selector(Some("amb"), Some("b"), None);
        for command in [
            speed(everything.clone(), 1.5),
            AudioCommand::Volume {
                selector: everything.clone(),
                volume: 0.5,
            },
            AudioCommand::VoiceVolume {
                voice: "amb".to_string(),
                volume: 0.5,
            },
            AudioCommand::Stop {
                selector: SampleSelector {
                    internal_id: Some("0".to_string()),
                    id: None,
                    file: None,
                    voice: None,
                },
                fade_out_ms: None,
            },
        ] {
            assert!(
                plays.cancel(&command).is_empty(),
                "{command:?} cancels nothing"
            );
        }
        assert_eq!(plays.len(), 3);
    }

    #[test]
    fn a_command_reaches_the_plays_it_selects_without_changing_them() {
        let (plays, keys) = three_plays();
        let amb = selector(Some("amb"), None, None);
        for (command, reached) in [
            (speed(amb.clone(), 1.5), 2),
            (
                AudioCommand::Volume {
                    selector: selector(None, Some("b"), None),
                    volume: 0.5,
                },
                1,
            ),
            (
                AudioCommand::Seek {
                    selector: selector(None, None, Some("wind.wav")),
                    position_ms: 10,
                },
                1,
            ),
            (
                AudioCommand::Stop {
                    selector: amb,
                    fade_out_ms: None,
                },
                2,
            ),
            (
                AudioCommand::VoiceFadeOut {
                    voice: "amb".to_string(),
                    time_ms: 100,
                },
                2,
            ),
            // Reaching is selection alone: a setting the daemon ignores still
            // reaches the plays it selects.
            (
                AudioCommand::Speed {
                    selector: selector(None, Some("r"), None),
                    speed: -1.0,
                    pitch_correction: true,
                },
                1,
            ),
        ] {
            assert_eq!(plays.reached_by(&command), reached, "{command:?}");
        }
        assert_eq!(plays.len(), 3, "reaching cancels nothing");
        for key in keys {
            assert_eq!(
                plays.plays.get(&key).unwrap().carried,
                CarriedSettings::default(),
                "reaching carries nothing"
            );
        }
    }

    #[test]
    fn commands_without_a_selector_or_voice_reach_no_play() {
        let (plays, _) = three_plays();
        for command in [
            AudioCommand::VoiceVolume {
                voice: "amb".to_string(),
                volume: 0.3,
            },
            AudioCommand::StopAll,
            AudioCommand::FadeAll { time_ms: 100 },
            AudioCommand::InputVolume {
                input: "amb".to_string(),
                volume: 0.5,
            },
            play_command(Some("amb"), Some("b"), "rain.wav"),
            speed(
                SampleSelector {
                    internal_id: Some("0".to_string()),
                    id: None,
                    file: None,
                    voice: None,
                },
                1.5,
            ),
        ] {
            assert_eq!(plays.reached_by(&command), 0, "{command:?} reaches no play");
        }
    }

    #[test]
    fn carried_settings_become_the_play_start_parameters() {
        let mut command = play_command(Some("amb"), None, "rain.wav");
        CarriedSettings {
            speed: Some((-1.0, false)),
            volume: Some(9.0),
            position_ms: Some(1500),
        }
        .apply_to(&mut command);

        match command {
            AudioCommand::Play {
                speed,
                pitch_correction,
                volume,
                start_position_ms,
                voice,
                ..
            } => {
                assert_eq!(speed, Some(-1.0));
                assert!(!pitch_correction);
                assert_eq!(
                    volume,
                    crate::config::MAX_GAIN,
                    "a carried volume is clamped as the volume command clamps it"
                );
                assert_eq!(start_position_ms, Some(1500));
                assert_eq!(voice.as_deref(), Some("amb"));
            }
            other => panic!("expected a play, got {other:?}"),
        }
    }

    #[test]
    fn no_carried_settings_leave_the_play_as_sent() {
        let mut command = play_command(Some("amb"), Some("r"), "rain.wav");
        if let AudioCommand::Play {
            speed,
            volume,
            start_position_ms,
            ..
        } = &mut command
        {
            *speed = Some(1.5);
            *volume = 0.6;
            *start_position_ms = Some(200);
        }
        CarriedSettings::default().apply_to(&mut command);
        match command {
            AudioCommand::Play {
                speed,
                volume,
                start_position_ms,
                ..
            } => {
                assert_eq!(speed, Some(1.5));
                assert_eq!(volume, 0.6);
                assert_eq!(start_position_ms, Some(200));
            }
            other => panic!("expected a play, got {other:?}"),
        }
    }

    #[test]
    fn carried_settings_leave_other_commands_alone() {
        let mut command = AudioCommand::StopAll;
        CarriedSettings {
            speed: Some((2.0, false)),
            ..Default::default()
        }
        .apply_to(&mut command);
        assert!(matches!(command, AudioCommand::StopAll));
    }

    #[test]
    fn voice_speed_carries_onto_plays_naming_its_voice() {
        let (mut plays, [rain, wind, bell]) = three_plays();
        let voice_speed = AudioCommand::VoiceSpeed {
            voice: "amb".to_string(),
            speed: 0.8,
            pitch_correction: true,
        };

        assert_eq!(plays.reached_by(&voice_speed), 2);
        assert_eq!(plays.carry(&voice_speed), 2);

        let speed_of = |key| plays.plays.get(&key).unwrap().carried.speed;
        assert_eq!(speed_of(rain), Some((0.8, true)));
        assert_eq!(speed_of(wind), Some((0.8, true)));
        assert_eq!(
            speed_of(bell),
            None,
            "a play without a voice is not reached"
        );
    }

    #[test]
    fn the_last_speed_or_voice_speed_to_reach_a_play_wins() {
        let (mut plays, [rain, _, _]) = three_plays();
        let voice_speed = |speed| AudioCommand::VoiceSpeed {
            voice: "amb".to_string(),
            speed,
            pitch_correction: false,
        };

        plays.carry(&voice_speed(0.5));
        plays.carry(&speed(selector(None, Some("r"), None), 2.0));
        assert_eq!(
            plays.plays.get(&rain).unwrap().carried.speed,
            Some((2.0, false))
        );

        plays.carry(&voice_speed(0.75));
        assert_eq!(
            plays.plays.get(&rain).unwrap().carried.speed,
            Some((0.75, false))
        );
    }

    #[test]
    fn voice_clear_cancels_plays_naming_its_voice() {
        let (mut plays, _) = three_plays();
        let clear = AudioCommand::VoiceClear {
            voice: VoiceTarget::Named("amb".to_string()),
        };

        assert_eq!(plays.reached_by(&clear), 2);
        let cancelled: Vec<_> = plays
            .cancel(&clear)
            .into_iter()
            .map(|p| p.payload)
            .collect();
        assert_eq!(cancelled, vec!["rain", "wind"]);
        assert_eq!(plays.len(), 1);
    }

    #[test]
    fn clearing_every_voice_and_reset_cancel_every_play() {
        for command in [
            AudioCommand::VoiceClear {
                voice: VoiceTarget::All,
            },
            AudioCommand::Reset,
        ] {
            let (mut plays, _) = three_plays();
            assert_eq!(plays.reached_by(&command), 3, "{command:?}");
            let cancelled: Vec<_> = plays
                .cancel(&command)
                .into_iter()
                .map(|p| p.payload)
                .collect();
            assert_eq!(cancelled, vec!["rain", "wind", "bell"], "{command:?}");
            assert!(plays.is_empty());
        }
    }

    #[test]
    fn voice_clear_and_reset_carry_nothing() {
        let (mut plays, keys) = three_plays();
        for command in [
            AudioCommand::VoiceClear {
                voice: VoiceTarget::Named("amb".to_string()),
            },
            AudioCommand::VoiceClear {
                voice: VoiceTarget::All,
            },
            AudioCommand::Reset,
        ] {
            assert_eq!(plays.carry(&command), 0, "{command:?} carries nowhere");
        }
        for key in keys {
            assert_eq!(
                plays.plays.get(&key).unwrap().carried,
                CarriedSettings::default()
            );
        }
    }
}
