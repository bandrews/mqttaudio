// ABOUTME: MQTT command parsing and validation.
// ABOUTME: Converts JSON messages to internal command types.

use serde::{Deserialize, Serialize};
use crate::config::ChannelRef;

/// MQTT command envelope
#[derive(Debug, Deserialize, Serialize)]
pub struct MqttCommand {
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<serde_json::Value>,
}

/// Channel mapping for routing source channels to destination channels.
/// Channels can be specified as numbers or as aliases defined in config.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct ChannelMapping {
    pub src: ChannelRef,
    pub dest: ChannelRef,
}

/// Selector for targeting samples by id, file, or voice
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct SampleSelector {
    /// Target specific sample by user-provided ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Target all samples playing this file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Target all samples in this voice
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
}

impl SampleSelector {
    /// Check if a sample matches this selector
    pub fn matches(&self, sample_id: Option<&str>, file_path: &str, voice_id: &str) -> bool {
        // Check if selector specifies id and if it matches
        if let Some(ref id) = self.id {
            if sample_id == Some(id.as_str()) {
                return true;
            }
        }

        // Check if selector specifies file and if it matches
        if let Some(ref file) = self.file {
            if file_path == file {
                return true;
            }
        }

        // Check if selector specifies voice and if it matches
        if let Some(ref voice) = self.voice {
            if voice_id == voice {
                return true;
            }
        }

        // No matches
        false
    }

    /// Check if this selector is empty (no criteria specified)
    pub fn is_empty(&self) -> bool {
        self.id.is_none() && self.file.is_none() && self.voice.is_none()
    }
}

/// Play command parameters
#[derive(Debug, Deserialize, Serialize)]
pub struct PlayMessage {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>, // User-provided sample identifier for targeting commands
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_map: Option<Vec<ChannelMapping>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fade_in: Option<u32>, // Fade in duration in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_position_ms: Option<u64>, // Start playback at this offset
    #[serde(rename = "loop")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_mode: Option<bool>, // Loop playback continuously
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crossfade_ms: Option<u32>, // Crossfade duration at loop boundaries (0 = disabled)
    // Future fields for later phases:
    // pub max_play_length: Option<i32>,    // Future
}

/// Voice stop command parameters
#[derive(Debug, Deserialize, Serialize)]
pub struct VoiceStopMessage {
    pub voice: String,
}

/// Voice fade out command parameters
#[derive(Debug, Deserialize, Serialize)]
pub struct VoiceFadeOutMessage {
    pub voice: String,
    pub time: u32, // milliseconds
}

/// Voice volume command parameters
#[derive(Debug, Deserialize, Serialize)]
pub struct VoiceVolumeMessage {
    pub voice: String,
    pub volume: f32,
}

/// Precache command parameters
#[derive(Debug, Deserialize, Serialize)]
pub struct PrecacheMessage {
    pub file: String,
}

/// Cache invalidate command parameters
#[derive(Debug, Deserialize, Serialize)]
pub struct CacheInvalidateMessage {
    pub file: String,
}

/// Input volume command parameters
#[derive(Debug, Deserialize, Serialize)]
pub struct InputVolumeMessage {
    /// Input index (0-based) or voice_id
    pub input: String,
    pub volume: f32,
}

/// Input mute command parameters
#[derive(Debug, Deserialize, Serialize)]
pub struct InputMuteMessage {
    /// Input index (0-based) or voice_id
    pub input: String,
    pub mute: bool,
}

/// Seek command parameters
#[derive(Debug, Deserialize, Serialize)]
pub struct SeekMessage {
    /// Target specific sample by user-provided ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Target all samples playing this file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Target all samples in this voice
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// Position to seek to in milliseconds
    pub position_ms: u64,
}

/// Speed command parameters
#[derive(Debug, Deserialize, Serialize)]
pub struct SpeedMessage {
    /// Target specific sample by user-provided ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Target all samples playing this file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Target all samples in this voice
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// Playback speed multiplier (1.0 = normal, 2.0 = double speed)
    pub speed: f32,
    /// If true, maintain original pitch when changing speed (tempo only)
    #[serde(default)]
    pub pitch_correction: bool,
}

/// Stop command parameters (with sample selector)
#[derive(Debug, Deserialize, Serialize)]
pub struct StopMessage {
    /// Target specific sample by user-provided ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Target all samples playing this file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Target all samples in this voice
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// Optional fade out duration in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fade_out_ms: Option<u32>,
}

/// Volume command parameters (with sample selector)
#[derive(Debug, Deserialize, Serialize)]
pub struct VolumeMessage {
    /// Target specific sample by user-provided ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Target all samples playing this file
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Target all samples in this voice
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// New volume level (0.0 - 1.0)
    pub volume: f32,
}

/// Internal audio command after parsing
#[derive(Debug, Clone)]
pub enum AudioCommand {
    Play {
        file: String,
        id: Option<String>, // User-provided sample identifier
        volume: f32,
        voice: Option<String>,
        channel_map: Option<Vec<ChannelMapping>>,
        fade_in: Option<u32>, // Fade in duration in milliseconds
        start_position_ms: Option<u64>, // Start playback at this offset
        loop_mode: bool, // Loop playback continuously
        crossfade_ms: u32, // Crossfade duration at loop boundaries (0 = disabled)
    },
    StopAll,
    VoiceStop {
        voice: String,
    },
    VoiceFadeOut {
        voice: String,
        time_ms: u32,
    },
    VoiceVolume {
        voice: String,
        volume: f32,
    },
    Precache {
        file: String,
    },
    CacheClear,
    CacheInvalidate {
        file: String,
    },
    InputVolume {
        input: String,
        volume: f32,
    },
    InputMute {
        input: String,
        mute: bool,
    },
    Seek {
        selector: SampleSelector,
        position_ms: u64,
    },
    Speed {
        selector: SampleSelector,
        speed: f32,
        pitch_correction: bool,
    },
    Stop {
        selector: SampleSelector,
        fade_out_ms: Option<u32>,
    },
    Volume {
        selector: SampleSelector,
        volume: f32,
    },
}

#[derive(Debug)]
pub enum ParseError {
    JsonError(serde_json::Error),
    MissingMessage,
    UnknownCommand(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::JsonError(e) => write!(f, "JSON parse error: {}", e),
            ParseError::MissingMessage => write!(f, "Command missing 'message' field"),
            ParseError::UnknownCommand(cmd) => write!(f, "Unknown command: {}", cmd),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<serde_json::Error> for ParseError {
    fn from(err: serde_json::Error) -> Self {
        ParseError::JsonError(err)
    }
}

/// Parse MQTT JSON payload into an audio command
pub fn parse_command(json: &str) -> Result<AudioCommand, ParseError> {
    let mqtt_cmd: MqttCommand = serde_json::from_str(json)?;

    match mqtt_cmd.command.as_str() {
        "play" | "soundPlay" => {
            // Require message field for play command
            let message = mqtt_cmd.message.ok_or(ParseError::MissingMessage)?;
            let play_msg: PlayMessage = serde_json::from_value(message)?;

            Ok(AudioCommand::Play {
                file: play_msg.file,
                id: play_msg.id,
                volume: play_msg.volume.unwrap_or(1.0),
                voice: play_msg.voice,
                channel_map: play_msg.channel_map,
                fade_in: play_msg.fade_in,
                start_position_ms: play_msg.start_position_ms,
                loop_mode: play_msg.loop_mode.unwrap_or(false),
                crossfade_ms: play_msg.crossfade_ms.unwrap_or(0),
            })
        }
        "stopall" | "soundStopAll" => {
            Ok(AudioCommand::StopAll)
        }
        "voice_stop" => {
            let message = mqtt_cmd.message.ok_or(ParseError::MissingMessage)?;
            let voice_msg: VoiceStopMessage = serde_json::from_value(message)?;

            Ok(AudioCommand::VoiceStop {
                voice: voice_msg.voice,
            })
        }
        "voice_fade_out" => {
            let message = mqtt_cmd.message.ok_or(ParseError::MissingMessage)?;
            let voice_msg: VoiceFadeOutMessage = serde_json::from_value(message)?;

            Ok(AudioCommand::VoiceFadeOut {
                voice: voice_msg.voice,
                time_ms: voice_msg.time,
            })
        }
        "voice_volume" => {
            let message = mqtt_cmd.message.ok_or(ParseError::MissingMessage)?;
            let voice_msg: VoiceVolumeMessage = serde_json::from_value(message)?;

            Ok(AudioCommand::VoiceVolume {
                voice: voice_msg.voice,
                volume: voice_msg.volume,
            })
        }
        "precache" | "soundPrecache" => {
            let message = mqtt_cmd.message.ok_or(ParseError::MissingMessage)?;
            let precache_msg: PrecacheMessage = serde_json::from_value(message)?;

            Ok(AudioCommand::Precache {
                file: precache_msg.file,
            })
        }
        "cache_clear" => {
            Ok(AudioCommand::CacheClear)
        }
        "cache_invalidate" => {
            let message = mqtt_cmd.message.ok_or(ParseError::MissingMessage)?;
            let invalidate_msg: CacheInvalidateMessage = serde_json::from_value(message)?;

            Ok(AudioCommand::CacheInvalidate {
                file: invalidate_msg.file,
            })
        }
        "input_volume" => {
            let message = mqtt_cmd.message.ok_or(ParseError::MissingMessage)?;
            let input_msg: InputVolumeMessage = serde_json::from_value(message)?;

            Ok(AudioCommand::InputVolume {
                input: input_msg.input,
                volume: input_msg.volume,
            })
        }
        "input_mute" => {
            let message = mqtt_cmd.message.ok_or(ParseError::MissingMessage)?;
            let input_msg: InputMuteMessage = serde_json::from_value(message)?;

            Ok(AudioCommand::InputMute {
                input: input_msg.input,
                mute: input_msg.mute,
            })
        }
        "seek" => {
            let message = mqtt_cmd.message.ok_or(ParseError::MissingMessage)?;
            let seek_msg: SeekMessage = serde_json::from_value(message)?;

            Ok(AudioCommand::Seek {
                selector: SampleSelector {
                    id: seek_msg.id,
                    file: seek_msg.file,
                    voice: seek_msg.voice,
                },
                position_ms: seek_msg.position_ms,
            })
        }
        "speed" => {
            let message = mqtt_cmd.message.ok_or(ParseError::MissingMessage)?;
            let speed_msg: SpeedMessage = serde_json::from_value(message)?;

            Ok(AudioCommand::Speed {
                selector: SampleSelector {
                    id: speed_msg.id,
                    file: speed_msg.file,
                    voice: speed_msg.voice,
                },
                speed: speed_msg.speed,
                pitch_correction: speed_msg.pitch_correction,
            })
        }
        "stop" => {
            let message = mqtt_cmd.message.ok_or(ParseError::MissingMessage)?;
            let stop_msg: StopMessage = serde_json::from_value(message)?;

            Ok(AudioCommand::Stop {
                selector: SampleSelector {
                    id: stop_msg.id,
                    file: stop_msg.file,
                    voice: stop_msg.voice,
                },
                fade_out_ms: stop_msg.fade_out_ms,
            })
        }
        "volume" => {
            let message = mqtt_cmd.message.ok_or(ParseError::MissingMessage)?;
            let vol_msg: VolumeMessage = serde_json::from_value(message)?;

            Ok(AudioCommand::Volume {
                selector: SampleSelector {
                    id: vol_msg.id,
                    file: vol_msg.file,
                    voice: vol_msg.voice,
                },
                volume: vol_msg.volume,
            })
        }
        unknown => Err(ParseError::UnknownCommand(unknown.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_play_command_minimal() {
        let json = r#"{"command": "play", "message": {"file": "test.wav"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, volume, voice, channel_map, .. } => {
                assert_eq!(file, "test.wav");
                assert_eq!(volume, 1.0); // Default volume
                assert!(voice.is_none()); // No voice specified
                assert!(channel_map.is_none()); // No channel map specified
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_command_with_volume() {
        let json = r#"{"command": "play", "message": {"file": "test.wav", "volume": 0.5}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, volume, voice, channel_map, .. } => {
                assert_eq!(file, "test.wav");
                assert_eq!(volume, 0.5);
                assert!(voice.is_none());
                assert!(channel_map.is_none());
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_command_url() {
        let json = r#"{"command": "play", "message": {"file": "http://example.com/audio.mp3", "volume": 0.8}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, volume, voice, channel_map, .. } => {
                assert_eq!(file, "http://example.com/audio.mp3");
                assert_eq!(volume, 0.8);
                assert!(voice.is_none());
                assert!(channel_map.is_none());
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_sound_play_alias() {
        // Legacy command name
        let json = r#"{"command": "soundPlay", "message": {"file": "test.wav"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { .. } => {}, // OK
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_command_with_voice() {
        let json = r#"{"command": "play", "message": {"file": "test.wav", "voice": "ambience"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, volume, voice, channel_map, .. } => {
                assert_eq!(file, "test.wav");
                assert_eq!(volume, 1.0);
                assert_eq!(voice, Some("ambience".to_string()));
                assert!(channel_map.is_none());
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_command_with_voice_and_volume() {
        let json = r#"{"command": "play", "message": {"file": "music.mp3", "voice": "background", "volume": 0.6}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, volume, voice, channel_map, .. } => {
                assert_eq!(file, "music.mp3");
                assert_eq!(volume, 0.6);
                assert_eq!(voice, Some("background".to_string()));
                assert!(channel_map.is_none());
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_voice_stop_command() {
        let json = r#"{"command": "voice_stop", "message": {"voice": "ambience"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::VoiceStop { voice } => {
                assert_eq!(voice, "ambience");
            }
            _ => panic!("Expected VoiceStop command"),
        }
    }

    #[test]
    fn test_parse_voice_fade_out_command() {
        let json = r#"{"command": "voice_fade_out", "message": {"voice": "music", "time": 2000}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::VoiceFadeOut { voice, time_ms } => {
                assert_eq!(voice, "music");
                assert_eq!(time_ms, 2000);
            }
            _ => panic!("Expected VoiceFadeOut command"),
        }
    }

    #[test]
    fn test_parse_voice_volume_command() {
        let json = r#"{"command": "voice_volume", "message": {"voice": "effects", "volume": 0.7}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::VoiceVolume { voice, volume } => {
                assert_eq!(voice, "effects");
                assert_eq!(volume, 0.7);
            }
            _ => panic!("Expected VoiceVolume command"),
        }
    }

    #[test]
    fn test_parse_voice_stop_missing_message() {
        let json = r#"{"command": "voice_stop"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {}, // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_voice_fade_out_missing_message() {
        let json = r#"{"command": "voice_fade_out"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {}, // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_voice_volume_missing_message() {
        let json = r#"{"command": "voice_volume"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {}, // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_stopall_command() {
        let json = r#"{"command": "stopall"}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::StopAll => {}, // OK
            _ => panic!("Expected StopAll command"),
        }
    }

    #[test]
    fn test_parse_sound_stopall_alias() {
        let json = r#"{"command": "soundStopAll"}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::StopAll => {}, // OK
            _ => panic!("Expected StopAll command"),
        }
    }

    #[test]
    fn test_parse_play_missing_message() {
        let json = r#"{"command": "play"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {}, // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_unknown_command() {
        let json = r#"{"command": "invalid"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::UnknownCommand(cmd) => assert_eq!(cmd, "invalid"),
            e => panic!("Expected UnknownCommand error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_invalid_json() {
        let json = r#"{"command": not valid json}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::JsonError(_) => {}, // OK
            e => panic!("Expected JsonError, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_channel_map_stereo() {
        let json = r#"{"command": "play", "message": {"file": "test.wav", "channel_map": [{"src": 0, "dest": 6}, {"src": 1, "dest": 7}]}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, volume, voice, channel_map, .. } => {
                assert_eq!(file, "test.wav");
                assert_eq!(volume, 1.0);
                assert!(voice.is_none());

                let map = channel_map.unwrap();
                assert_eq!(map.len(), 2);
                assert_eq!(map[0], ChannelMapping { src: ChannelRef::Index(0), dest: ChannelRef::Index(6) });
                assert_eq!(map[1], ChannelMapping { src: ChannelRef::Index(1), dest: ChannelRef::Index(7) });
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_channel_map_quad() {
        let json = r#"{"command": "play", "message": {
            "file": "quad.wav",
            "channel_map": [
                {"src": 0, "dest": 0},
                {"src": 1, "dest": 1},
                {"src": 2, "dest": 2},
                {"src": 3, "dest": 3}
            ]
        }}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, channel_map, .. } => {
                assert_eq!(file, "quad.wav");

                let map = channel_map.unwrap();
                assert_eq!(map.len(), 4);
                assert_eq!(map[0], ChannelMapping { src: ChannelRef::Index(0), dest: ChannelRef::Index(0) });
                assert_eq!(map[1], ChannelMapping { src: ChannelRef::Index(1), dest: ChannelRef::Index(1) });
                assert_eq!(map[2], ChannelMapping { src: ChannelRef::Index(2), dest: ChannelRef::Index(2) });
                assert_eq!(map[3], ChannelMapping { src: ChannelRef::Index(3), dest: ChannelRef::Index(3) });
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_channel_map_complex_routing() {
        let json = r#"{"command": "play", "message": {
            "file": "surround.wav",
            "voice": "ambience",
            "volume": 0.7,
            "channel_map": [
                {"src": 0, "dest": 8},
                {"src": 1, "dest": 9},
                {"src": 2, "dest": 10},
                {"src": 3, "dest": 11}
            ]
        }}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, volume, voice, channel_map, .. } => {
                assert_eq!(file, "surround.wav");
                assert_eq!(volume, 0.7);
                assert_eq!(voice, Some("ambience".to_string()));

                let map = channel_map.unwrap();
                assert_eq!(map.len(), 4);
                assert_eq!(map[0], ChannelMapping { src: ChannelRef::Index(0), dest: ChannelRef::Index(8) });
                assert_eq!(map[1], ChannelMapping { src: ChannelRef::Index(1), dest: ChannelRef::Index(9) });
                assert_eq!(map[2], ChannelMapping { src: ChannelRef::Index(2), dest: ChannelRef::Index(10) });
                assert_eq!(map[3], ChannelMapping { src: ChannelRef::Index(3), dest: ChannelRef::Index(11) });
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_channel_map_mono_to_multiple() {
        // One source channel to multiple destinations
        let json = r#"{"command": "play", "message": {
            "file": "mono.wav",
            "channel_map": [
                {"src": 0, "dest": 0},
                {"src": 0, "dest": 1}
            ]
        }}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { channel_map, .. } => {
                let map = channel_map.unwrap();
                assert_eq!(map.len(), 2);
                assert_eq!(map[0], ChannelMapping { src: ChannelRef::Index(0), dest: ChannelRef::Index(0) });
                assert_eq!(map[1], ChannelMapping { src: ChannelRef::Index(0), dest: ChannelRef::Index(1) });
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_channel_map_empty() {
        let json = r#"{"command": "play", "message": {"file": "test.wav", "channel_map": []}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { channel_map, .. } => {
                let map = channel_map.unwrap();
                assert_eq!(map.len(), 0);
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_channel_map_with_aliases() {
        // Channel map can use string aliases instead of numbers
        let json = r#"{"command": "play", "message": {
            "file": "test.wav",
            "channel_map": [
                {"src": 0, "dest": "front_left"},
                {"src": 1, "dest": "front_right"}
            ]
        }}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { channel_map, .. } => {
                let map = channel_map.unwrap();
                assert_eq!(map.len(), 2);
                assert_eq!(map[0], ChannelMapping {
                    src: ChannelRef::Index(0),
                    dest: ChannelRef::Alias("front_left".to_string())
                });
                assert_eq!(map[1], ChannelMapping {
                    src: ChannelRef::Index(1),
                    dest: ChannelRef::Alias("front_right".to_string())
                });
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_channel_map_all_aliases() {
        // Both src and dest can be aliases
        let json = r#"{"command": "play", "message": {
            "file": "test.wav",
            "channel_map": [
                {"src": "left", "dest": "speaker_1"},
                {"src": "right", "dest": "speaker_2"}
            ]
        }}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { channel_map, .. } => {
                let map = channel_map.unwrap();
                assert_eq!(map.len(), 2);
                assert_eq!(map[0], ChannelMapping {
                    src: ChannelRef::Alias("left".to_string()),
                    dest: ChannelRef::Alias("speaker_1".to_string())
                });
                assert_eq!(map[1], ChannelMapping {
                    src: ChannelRef::Alias("right".to_string()),
                    dest: ChannelRef::Alias("speaker_2".to_string())
                });
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_precache_command() {
        let json = r#"{"command": "precache", "message": {"file": "http://example.com/bigfile.wav"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Precache { file } => {
                assert_eq!(file, "http://example.com/bigfile.wav");
            }
            _ => panic!("Expected Precache command"),
        }
    }

    #[test]
    fn test_parse_sound_precache_alias() {
        let json = r#"{"command": "soundPrecache", "message": {"file": "test.wav"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Precache { file } => {
                assert_eq!(file, "test.wav");
            }
            _ => panic!("Expected Precache command"),
        }
    }

    #[test]
    fn test_parse_precache_missing_message() {
        let json = r#"{"command": "precache"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {}, // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_cache_clear_command() {
        let json = r#"{"command": "cache_clear"}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::CacheClear => {}, // OK
            _ => panic!("Expected CacheClear command"),
        }
    }

    #[test]
    fn test_parse_cache_invalidate_command() {
        let json = r#"{"command": "cache_invalidate", "message": {"file": "http://example.com/old.wav"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::CacheInvalidate { file } => {
                assert_eq!(file, "http://example.com/old.wav");
            }
            _ => panic!("Expected CacheInvalidate command"),
        }
    }

    #[test]
    fn test_parse_cache_invalidate_missing_message() {
        let json = r#"{"command": "cache_invalidate"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {}, // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_play_with_fade_in() {
        let json = r#"{"command": "play", "message": {"file": "test.wav", "fade_in": 2000}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, fade_in, .. } => {
                assert_eq!(file, "test.wav");
                assert_eq!(fade_in, Some(2000));
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_without_fade_in() {
        let json = r#"{"command": "play", "message": {"file": "test.wav"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { fade_in, .. } => {
                assert_eq!(fade_in, None);
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_input_volume_by_index() {
        let json = r#"{"command": "input_volume", "message": {"input": "0", "volume": 0.5}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::InputVolume { input, volume } => {
                assert_eq!(input, "0");
                assert_eq!(volume, 0.5);
            }
            _ => panic!("Expected InputVolume command"),
        }
    }

    #[test]
    fn test_parse_input_volume_by_voice_id() {
        let json = r#"{"command": "input_volume", "message": {"input": "gamemaster_mic", "volume": 0.8}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::InputVolume { input, volume } => {
                assert_eq!(input, "gamemaster_mic");
                assert_eq!(volume, 0.8);
            }
            _ => panic!("Expected InputVolume command"),
        }
    }

    #[test]
    fn test_parse_input_volume_missing_message() {
        let json = r#"{"command": "input_volume"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {}, // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_input_mute() {
        let json = r#"{"command": "input_mute", "message": {"input": "0", "mute": true}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::InputMute { input, mute } => {
                assert_eq!(input, "0");
                assert_eq!(mute, true);
            }
            _ => panic!("Expected InputMute command"),
        }
    }

    #[test]
    fn test_parse_input_mute_unmute() {
        let json = r#"{"command": "input_mute", "message": {"input": "mic1", "mute": false}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::InputMute { input, mute } => {
                assert_eq!(input, "mic1");
                assert_eq!(mute, false);
            }
            _ => panic!("Expected InputMute command"),
        }
    }

    #[test]
    fn test_parse_input_mute_missing_message() {
        let json = r#"{"command": "input_mute"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {}, // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    // === Sample ID Tests ===

    #[test]
    fn test_parse_play_with_sample_id() {
        let json = r#"{"command": "play", "message": {"file": "test.wav", "id": "my-sound-1"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, id, .. } => {
                assert_eq!(file, "test.wav");
                assert_eq!(id, Some("my-sound-1".to_string()));
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_without_sample_id() {
        let json = r#"{"command": "play", "message": {"file": "test.wav"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, id, .. } => {
                assert_eq!(file, "test.wav");
                assert_eq!(id, None);
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_with_all_fields() {
        let json = r#"{"command": "play", "message": {
            "file": "background.mp3",
            "id": "background-music",
            "voice": "music",
            "volume": 0.5,
            "fade_in": 2000,
            "start_position_ms": 30000,
            "channel_map": [{"src": 0, "dest": 2}, {"src": 1, "dest": 3}]
        }}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, id, voice, volume, fade_in, start_position_ms, channel_map, .. } => {
                assert_eq!(file, "background.mp3");
                assert_eq!(id, Some("background-music".to_string()));
                assert_eq!(voice, Some("music".to_string()));
                assert_eq!(volume, 0.5);
                assert_eq!(fade_in, Some(2000));
                assert_eq!(start_position_ms, Some(30000));
                assert!(channel_map.is_some());
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_with_start_position() {
        let json = r#"{"command": "play", "message": {"file": "long_track.mp3", "start_position_ms": 60000}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, start_position_ms, .. } => {
                assert_eq!(file, "long_track.mp3");
                assert_eq!(start_position_ms, Some(60000));
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_without_start_position() {
        let json = r#"{"command": "play", "message": {"file": "test.wav"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { start_position_ms, .. } => {
                assert_eq!(start_position_ms, None);
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_with_loop_true() {
        let json = r#"{"command": "play", "message": {"file": "ambient.mp3", "loop": true}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, loop_mode, .. } => {
                assert_eq!(file, "ambient.mp3");
                assert!(loop_mode);
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_with_loop_false() {
        let json = r#"{"command": "play", "message": {"file": "effect.wav", "loop": false}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, loop_mode, .. } => {
                assert_eq!(file, "effect.wav");
                assert!(!loop_mode);
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_without_loop() {
        let json = r#"{"command": "play", "message": {"file": "test.wav"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { loop_mode, .. } => {
                assert!(!loop_mode); // Defaults to false
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_with_crossfade() {
        let json = r#"{"command": "play", "message": {"file": "ambient.mp3", "loop": true, "crossfade_ms": 100}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, loop_mode, crossfade_ms, .. } => {
                assert_eq!(file, "ambient.mp3");
                assert!(loop_mode);
                assert_eq!(crossfade_ms, 100);
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_without_crossfade() {
        let json = r#"{"command": "play", "message": {"file": "test.wav", "loop": true}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { crossfade_ms, .. } => {
                assert_eq!(crossfade_ms, 0); // Defaults to 0 (disabled)
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_crossfade_zero() {
        let json = r#"{"command": "play", "message": {"file": "test.wav", "loop": true, "crossfade_ms": 0}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { crossfade_ms, .. } => {
                assert_eq!(crossfade_ms, 0);
            }
            _ => panic!("Expected Play command"),
        }
    }

    // === SampleSelector Tests ===

    #[test]
    fn test_selector_matches_by_id() {
        let selector = SampleSelector {
            id: Some("my-sound".to_string()),
            file: None,
            voice: None,
        };

        assert!(selector.matches(Some("my-sound"), "test.wav", "voice1"));
        assert!(!selector.matches(Some("other-sound"), "test.wav", "voice1"));
        assert!(!selector.matches(None, "test.wav", "voice1"));
    }

    #[test]
    fn test_selector_matches_by_file() {
        let selector = SampleSelector {
            id: None,
            file: Some("music.mp3".to_string()),
            voice: None,
        };

        assert!(selector.matches(None, "music.mp3", "voice1"));
        assert!(selector.matches(Some("any-id"), "music.mp3", "voice1"));
        assert!(!selector.matches(None, "other.mp3", "voice1"));
    }

    #[test]
    fn test_selector_matches_by_voice() {
        let selector = SampleSelector {
            id: None,
            file: None,
            voice: Some("background".to_string()),
        };

        assert!(selector.matches(None, "any.wav", "background"));
        assert!(!selector.matches(None, "any.wav", "foreground"));
    }

    #[test]
    fn test_selector_matches_any_criterion() {
        // Selector with multiple criteria matches if ANY matches
        let selector = SampleSelector {
            id: Some("specific-sound".to_string()),
            file: Some("music.mp3".to_string()),
            voice: None,
        };

        // Matches by id
        assert!(selector.matches(Some("specific-sound"), "other.wav", "voice1"));
        // Matches by file
        assert!(selector.matches(Some("other-id"), "music.mp3", "voice1"));
        // Matches neither
        assert!(!selector.matches(Some("other-id"), "other.wav", "voice1"));
    }

    #[test]
    fn test_selector_empty() {
        let empty = SampleSelector {
            id: None,
            file: None,
            voice: None,
        };
        assert!(empty.is_empty());
        assert!(!empty.matches(Some("any"), "any", "any"));

        let not_empty = SampleSelector {
            id: Some("test".to_string()),
            file: None,
            voice: None,
        };
        assert!(!not_empty.is_empty());
    }

    #[test]
    fn test_selector_parse_from_json() {
        let json = r#"{"id": "my-sound"}"#;
        let selector: SampleSelector = serde_json::from_str(json).unwrap();
        assert_eq!(selector.id, Some("my-sound".to_string()));
        assert_eq!(selector.file, None);
        assert_eq!(selector.voice, None);

        let json = r#"{"file": "test.wav", "voice": "effects"}"#;
        let selector: SampleSelector = serde_json::from_str(json).unwrap();
        assert_eq!(selector.id, None);
        assert_eq!(selector.file, Some("test.wav".to_string()));
        assert_eq!(selector.voice, Some("effects".to_string()));
    }

    // === Seek Command Tests ===

    #[test]
    fn test_parse_seek_by_id() {
        let json = r#"{"command": "seek", "message": {"id": "my-sound", "position_ms": 5000}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Seek { selector, position_ms } => {
                assert_eq!(selector.id, Some("my-sound".to_string()));
                assert_eq!(selector.file, None);
                assert_eq!(selector.voice, None);
                assert_eq!(position_ms, 5000);
            }
            _ => panic!("Expected Seek command"),
        }
    }

    #[test]
    fn test_parse_seek_by_file() {
        let json = r#"{"command": "seek", "message": {"file": "music.mp3", "position_ms": 30000}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Seek { selector, position_ms } => {
                assert_eq!(selector.id, None);
                assert_eq!(selector.file, Some("music.mp3".to_string()));
                assert_eq!(selector.voice, None);
                assert_eq!(position_ms, 30000);
            }
            _ => panic!("Expected Seek command"),
        }
    }

    #[test]
    fn test_parse_seek_by_voice() {
        let json = r#"{"command": "seek", "message": {"voice": "background", "position_ms": 0}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Seek { selector, position_ms } => {
                assert_eq!(selector.id, None);
                assert_eq!(selector.file, None);
                assert_eq!(selector.voice, Some("background".to_string()));
                assert_eq!(position_ms, 0);
            }
            _ => panic!("Expected Seek command"),
        }
    }

    #[test]
    fn test_parse_seek_multiple_selectors() {
        let json = r#"{"command": "seek", "message": {"id": "bg-music", "file": "music.mp3", "position_ms": 15000}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Seek { selector, position_ms } => {
                assert_eq!(selector.id, Some("bg-music".to_string()));
                assert_eq!(selector.file, Some("music.mp3".to_string()));
                assert_eq!(position_ms, 15000);
            }
            _ => panic!("Expected Seek command"),
        }
    }

    #[test]
    fn test_parse_seek_missing_message() {
        let json = r#"{"command": "seek"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {}, // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_seek_missing_position() {
        let json = r#"{"command": "seek", "message": {"id": "my-sound"}}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::JsonError(_) => {}, // OK - position_ms is required
            e => panic!("Expected JsonError, got: {:?}", e),
        }
    }

    // === Stop Command Tests (with selector) ===

    #[test]
    fn test_parse_stop_by_id() {
        let json = r#"{"command": "stop", "message": {"id": "effect-1"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Stop { selector, fade_out_ms } => {
                assert_eq!(selector.id, Some("effect-1".to_string()));
                assert_eq!(fade_out_ms, None);
            }
            _ => panic!("Expected Stop command"),
        }
    }

    #[test]
    fn test_parse_stop_by_file_with_fade() {
        let json = r#"{"command": "stop", "message": {"file": "music.mp3", "fade_out_ms": 500}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Stop { selector, fade_out_ms } => {
                assert_eq!(selector.file, Some("music.mp3".to_string()));
                assert_eq!(fade_out_ms, Some(500));
            }
            _ => panic!("Expected Stop command"),
        }
    }

    #[test]
    fn test_parse_stop_by_voice() {
        let json = r#"{"command": "stop", "message": {"voice": "effects"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Stop { selector, .. } => {
                assert_eq!(selector.voice, Some("effects".to_string()));
            }
            _ => panic!("Expected Stop command"),
        }
    }

    // === Volume Command Tests (with selector) ===

    #[test]
    fn test_parse_volume_by_id() {
        let json = r#"{"command": "volume", "message": {"id": "background-music", "volume": 0.5}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Volume { selector, volume } => {
                assert_eq!(selector.id, Some("background-music".to_string()));
                assert_eq!(volume, 0.5);
            }
            _ => panic!("Expected Volume command"),
        }
    }

    #[test]
    fn test_parse_volume_by_file() {
        let json = r#"{"command": "volume", "message": {"file": "ambient.wav", "volume": 0.3}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Volume { selector, volume } => {
                assert_eq!(selector.file, Some("ambient.wav".to_string()));
                assert_eq!(volume, 0.3);
            }
            _ => panic!("Expected Volume command"),
        }
    }

    // === Speed Command Tests ===

    #[test]
    fn test_parse_speed_by_id() {
        let json = r#"{"command": "speed", "message": {"id": "music-track", "speed": 1.5}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Speed { selector, speed, pitch_correction } => {
                assert_eq!(selector.id, Some("music-track".to_string()));
                assert_eq!(speed, 1.5);
                assert_eq!(pitch_correction, false); // default
            }
            _ => panic!("Expected Speed command"),
        }
    }

    #[test]
    fn test_parse_speed_by_voice() {
        let json = r#"{"command": "speed", "message": {"voice": "background", "speed": 0.5}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Speed { selector, speed, pitch_correction } => {
                assert_eq!(selector.voice, Some("background".to_string()));
                assert_eq!(speed, 0.5);
                assert_eq!(pitch_correction, false);
            }
            _ => panic!("Expected Speed command"),
        }
    }

    #[test]
    fn test_parse_speed_with_pitch_correction() {
        let json = r#"{"command": "speed", "message": {"file": "music.mp3", "speed": 2.0, "pitch_correction": true}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Speed { selector, speed, pitch_correction } => {
                assert_eq!(selector.file, Some("music.mp3".to_string()));
                assert_eq!(speed, 2.0);
                assert_eq!(pitch_correction, true);
            }
            _ => panic!("Expected Speed command"),
        }
    }

    #[test]
    fn test_parse_speed_missing_message() {
        let json = r#"{"command": "speed"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {}, // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_speed_missing_speed_value() {
        let json = r#"{"command": "speed", "message": {"id": "my-sound"}}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::JsonError(_) => {}, // OK - speed is required
            e => panic!("Expected JsonError, got: {:?}", e),
        }
    }
}
