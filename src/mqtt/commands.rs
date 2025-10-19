// ABOUTME: MQTT command parsing and validation.
// ABOUTME: Converts JSON messages to internal command types.

use serde::{Deserialize, Serialize};

/// MQTT command envelope
#[derive(Debug, Deserialize, Serialize)]
pub struct MqttCommand {
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<serde_json::Value>,
}

/// Play command parameters
#[derive(Debug, Deserialize, Serialize)]
pub struct PlayMessage {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<f32>,
    // Future fields for later phases:
    // pub voice: Option<String>,           // Phase 6
    // pub channel_map: Option<Vec<...>>,   // Phase 7
    // #[serde(rename = "loop")]
    // pub loop_mode: Option<bool>,         // Future
    // pub fade_in: Option<u32>,            // Phase 9
    // pub max_play_length: Option<i32>,    // Future
}

/// Internal audio command after parsing
#[derive(Debug, Clone)]
pub enum AudioCommand {
    Play {
        file: String,
        volume: f32,
    },
    StopAll,
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
                volume: play_msg.volume.unwrap_or(1.0),
            })
        }
        "stopall" | "soundStopAll" => {
            Ok(AudioCommand::StopAll)
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
            AudioCommand::Play { file, volume } => {
                assert_eq!(file, "test.wav");
                assert_eq!(volume, 1.0); // Default volume
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_command_with_volume() {
        let json = r#"{"command": "play", "message": {"file": "test.wav", "volume": 0.5}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, volume } => {
                assert_eq!(file, "test.wav");
                assert_eq!(volume, 0.5);
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_command_url() {
        let json = r#"{"command": "play", "message": {"file": "http://example.com/audio.mp3", "volume": 0.8}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, volume } => {
                assert_eq!(file, "http://example.com/audio.mp3");
                assert_eq!(volume, 0.8);
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
}
