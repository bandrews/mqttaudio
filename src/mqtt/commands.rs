// ABOUTME: MQTT command parsing and validation.
// ABOUTME: Converts JSON messages to internal command types.

use crate::config::{ChannelRef, FreshnessMode, LoadMode};
use serde::{Deserialize, Serialize};

/// Fade duration used when a fadeall command does not name one
pub const DEFAULT_FADE_ALL_MS: u32 = 1000;

/// Why a command failed, coarse enough for an HTTP status mapping
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandErrorKind {
    /// The request itself was malformed (bad JSON, unknown command, bad parameter)
    InvalidRequest,
    /// The named file, sample, voice, or input does not exist
    NotFound,
    /// The request was valid but not permitted (e.g. path outside allowed_directories)
    Forbidden,
    /// Superseded by a later stopall/fadeall before it could take effect
    Cancelled,
    /// Something failed on our side
    Internal,
}

/// A command failure with a human-readable explanation
#[derive(Debug, Clone)]
pub struct CommandError {
    pub kind: CommandErrorKind,
    pub message: String,
}

impl CommandError {
    pub fn new(kind: CommandErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// What actually happened when a command was processed
pub type CommandOutcome = Result<String, CommandError>;

/// A command traveling to the processing loop, with an optional reply slot.
/// MQTT publishes send no reply; HTTP requests wait on one so clients learn
/// whether the command actually worked.
#[derive(Debug)]
pub struct CommandRequest {
    pub payload: String,
    pub reply: Option<tokio::sync::oneshot::Sender<CommandOutcome>>,
}

impl CommandRequest {
    /// A fire-and-forget request (MQTT publishes)
    pub fn fire_and_forget(payload: String) -> Self {
        Self {
            payload,
            reply: None,
        }
    }

    /// A request paired with a receiver for its outcome
    pub fn with_reply(payload: String) -> (Self, tokio::sync::oneshot::Receiver<CommandOutcome>) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (
            Self {
                payload,
                reply: Some(tx),
            },
            rx,
        )
    }
}

/// MQTT command envelope supporting both flattened and nested formats.
/// Flattened: {"command": "play", "file": "test.wav", "volume": 0.8}
/// Nested (legacy): {"command": "play", "message": {"file": "test.wav", "volume": 0.8}}
#[derive(Debug, Deserialize, Serialize)]
pub struct MqttCommand {
    pub command: String,
    /// Legacy nested format - parameters wrapped in a "message" object
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<serde_json::Value>,
    /// Capture all other fields for the flattened format
    #[serde(flatten)]
    pub params: serde_json::Map<String, serde_json::Value>,
}

impl MqttCommand {
    /// Get the parameters for this command, supporting both formats.
    /// If `message` field exists (legacy format), use those parameters.
    /// Otherwise, use the flattened parameters from the root object.
    pub fn get_params(&self) -> serde_json::Value {
        if let Some(ref message) = self.message {
            message.clone()
        } else if self.params.is_empty() {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            serde_json::Value::Object(self.params.clone())
        }
    }

    /// Check if this command has parameters (either format).
    pub fn has_params(&self) -> bool {
        self.message.is_some() || !self.params.is_empty()
    }
}

/// Channel mapping for routing source channels to destination channels.
/// Channels can be specified as numbers or as aliases defined in config.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct ChannelMapping {
    pub src: ChannelRef,
    pub dest: ChannelRef,
    /// Optional per-route gain (default 1.0). Lets a downmix that sums several
    /// source channels into one destination be attenuated to avoid clipping (D29).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gain: Option<f32>,
}

/// Selector for targeting samples by internal_id, id, file, or voice
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct SampleSelector {
    /// Target specific sample by system-assigned internal ID (unique, precise)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_id: Option<String>,
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
    pub fn matches(
        &self,
        internal_id: u64,
        sample_id: Option<&str>,
        file_path: &str,
        voice_id: &str,
    ) -> bool {
        // Check if selector specifies internal_id and if it matches (highest priority)
        if let Some(ref iid) = self.internal_id {
            if let Ok(parsed) = iid.parse::<u64>() {
                if parsed == internal_id {
                    return true;
                }
            }
        }

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
        self.internal_id.is_none()
            && self.id.is_none()
            && self.file.is_none()
            && self.voice.is_none()
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<LoadMode>, // Load strategy override: auto|full|stream
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_ms: Option<u32>, // Windowed-source ring depth override (streamed plays)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prebuffer_ms: Option<u32>, // Windowed-source prebuffer override (streamed plays)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freshness: Option<FreshnessMode>, // Freshness override: trusting|dev|pinned
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cacheable: Option<bool>, // HTTP windowed plays: persist to disk (true, default) or treat as live (false)
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

/// Fade-all command parameters
#[derive(Debug, Deserialize, Serialize)]
pub struct FadeAllMessage {
    /// Fade duration in milliseconds. Also accepted as "fade_out_ms",
    /// matching the field name the stop command uses.
    #[serde(alias = "fade_out_ms")]
    pub time: Option<u32>,
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

/// Fail-closed talkback lease request. Destination names are validated again
/// by the daemon state machine; the parser only enforces field types/ranges.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TalkbackAcquireMessage {
    pub client_id: String,
    #[serde(default = "default_talkback_source")]
    pub source_id: String,
    pub destination: String,
    pub gain: f32,
    pub lease_ms: u64,
}

fn default_talkback_source() -> String {
    "GM_MIC".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TalkbackReleaseMessage {
    pub client_id: String,
    pub lease_id: String,
}

/// Seek command parameters
#[derive(Debug, Deserialize, Serialize)]
pub struct SeekMessage {
    /// Target specific sample by system-assigned internal ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_id: Option<String>,
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
    /// Target specific sample by system-assigned internal ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_id: Option<String>,
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
    /// Target specific sample by system-assigned internal ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_id: Option<String>,
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
    /// Target specific sample by system-assigned internal ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_id: Option<String>,
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
        fade_in: Option<u32>,             // Fade in duration in milliseconds
        start_position_ms: Option<u64>,   // Start playback at this offset
        loop_mode: bool,                  // Loop playback continuously
        crossfade_ms: u32,                // Crossfade duration at loop boundaries (0 = disabled)
        mode: LoadMode,                   // Resolved load strategy (auto|full|stream)
        window_ms: Option<u32>,           // Windowed-source ring depth override
        prebuffer_ms: Option<u32>,        // Windowed-source prebuffer override
        freshness: Option<FreshnessMode>, // Freshness override (None => config default)
        cacheable: Option<bool>, // HTTP windowed: persist to disk (None/true) or treat as live (false)
    },
    StopAll,
    FadeAll {
        time_ms: u32,
    },
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
    /// Invalidate a cached entry and re-precache it, so the next play is fresh+instant.
    CacheReload {
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
    TalkbackAcquire(TalkbackAcquireMessage),
    TalkbackRelease(TalkbackReleaseMessage),
    TalkbackHardMute,
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
    InvalidParameter(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::JsonError(e) => write!(f, "JSON parse error: {}", e),
            ParseError::MissingMessage => write!(f, "Command missing 'message' field"),
            ParseError::UnknownCommand(cmd) => write!(f, "Unknown command: {}", cmd),
            ParseError::InvalidParameter(msg) => write!(f, "Invalid parameter: {}", msg),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<serde_json::Error> for ParseError {
    fn from(err: serde_json::Error) -> Self {
        ParseError::JsonError(err)
    }
}

/// Expand macros in a command JSON string.
/// Macros are referenced via the "macro" field, which can be a single string or array of strings.
/// Parameters are merged with precedence: command params > earlier macros > later macros.
/// The "macro" field is removed from the output.
///
/// # Arguments
/// * `json` - The raw JSON command string
/// * `macros` - Map of macro names to their parameter values
///
/// # Returns
/// The expanded JSON string with macro parameters merged in
pub fn expand_macros(
    json: &str,
    macros: &std::collections::HashMap<String, serde_json::Value>,
) -> Result<String, ParseError> {
    // Parse the input JSON
    let mut value: serde_json::Value = serde_json::from_str(json)?;

    // Only process if it's an object
    let obj = match value.as_object_mut() {
        Some(o) => o,
        None => return Ok(json.to_string()),
    };

    // Check for macro field
    let macro_names = match obj.remove("macro") {
        None => return serde_json::to_string(&value).map_err(ParseError::from),
        Some(serde_json::Value::String(s)) => vec![s],
        Some(serde_json::Value::Array(arr)) => arr
            .into_iter()
            .filter_map(|v| {
                if let serde_json::Value::String(s) = v {
                    Some(s)
                } else {
                    None
                }
            })
            .collect(),
        Some(_) => return serde_json::to_string(&value).map_err(ParseError::from),
    };

    // Build merged macro parameters: process macros in forward order, earlier macros take precedence
    let mut macro_params = serde_json::Map::new();

    for macro_name in macro_names.iter() {
        match macros.get(macro_name) {
            Some(params) => {
                if let Some(macro_obj) = params.as_object() {
                    for (k, v) in macro_obj {
                        // Only set if not already present (earlier macros take precedence)
                        if !macro_params.contains_key(k) {
                            macro_params.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            None => {
                tracing::warn!(
                    "Unknown macro '{}' referenced by command; ignoring it",
                    macro_name
                );
            }
        }
    }

    // Command parameters always win over macro parameters. For the nested
    // format, parameters are read from the "message" object, so macro
    // parameters must be merged there to take effect.
    if let Some(serde_json::Value::Object(message)) = obj.get_mut("message") {
        for (k, v) in macro_params {
            if !message.contains_key(&k) {
                message.insert(k, v);
            }
        }
        serde_json::to_string(&value).map_err(ParseError::from)
    } else {
        let mut merged = macro_params;
        for (k, v) in obj.iter() {
            merged.insert(k.clone(), v.clone());
        }

        let result = serde_json::Value::Object(merged);
        serde_json::to_string(&result).map_err(ParseError::from)
    }
}

/// Parse MQTT JSON payload into an audio command.
/// Supports both flattened and nested (legacy) formats:
/// - Flattened: {"command": "play", "file": "test.wav", "volume": 0.8}
/// - Nested: {"command": "play", "message": {"file": "test.wav", "volume": 0.8}}
/// Reject a non-numeric internal_id at parse time: the system-assigned ids
/// are numeric, so a non-numeric value could only ever silently match nothing.
fn validate_internal_id(internal_id: &Option<String>) -> Result<(), ParseError> {
    if let Some(iid) = internal_id {
        if iid.parse::<u64>().is_err() {
            return Err(ParseError::InvalidParameter(format!(
                "internal_id '{}' is not numeric - use the internal_id values shown by /status/samples",
                iid
            )));
        }
    }
    Ok(())
}

pub fn parse_command(json: &str) -> Result<AudioCommand, ParseError> {
    let mqtt_cmd: MqttCommand = serde_json::from_str(json)?;

    match mqtt_cmd.command.as_str() {
        "play" | "soundPlay" => {
            // Require parameters for play command
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            let play_msg: PlayMessage = serde_json::from_value(mqtt_cmd.get_params())?;

            Ok(AudioCommand::Play {
                file: play_msg.file,
                id: play_msg.id,
                volume: play_msg
                    .volume
                    .unwrap_or(1.0)
                    .clamp(0.0, crate::config::MAX_GAIN),
                voice: play_msg.voice,
                channel_map: play_msg.channel_map,
                fade_in: play_msg.fade_in,
                start_position_ms: play_msg.start_position_ms,
                loop_mode: play_msg.loop_mode.unwrap_or(false),
                crossfade_ms: play_msg.crossfade_ms.unwrap_or(0),
                mode: play_msg.mode.unwrap_or_default(),
                window_ms: play_msg.window_ms,
                prebuffer_ms: play_msg.prebuffer_ms,
                freshness: play_msg.freshness,
                cacheable: play_msg.cacheable,
            })
        }
        "stopall" | "soundStopAll" => Ok(AudioCommand::StopAll),
        "fadeall" | "soundFadeAll" | "fadeout" | "soundFadeOut" => {
            let time_ms = if mqtt_cmd.has_params() {
                let msg: FadeAllMessage = serde_json::from_value(mqtt_cmd.get_params())?;
                msg.time.unwrap_or(DEFAULT_FADE_ALL_MS)
            } else {
                DEFAULT_FADE_ALL_MS
            };

            Ok(AudioCommand::FadeAll { time_ms })
        }
        "voice_stop" => {
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            let voice_msg: VoiceStopMessage = serde_json::from_value(mqtt_cmd.get_params())?;

            Ok(AudioCommand::VoiceStop {
                voice: voice_msg.voice,
            })
        }
        "voice_fade_out" => {
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            let voice_msg: VoiceFadeOutMessage = serde_json::from_value(mqtt_cmd.get_params())?;

            Ok(AudioCommand::VoiceFadeOut {
                voice: voice_msg.voice,
                time_ms: voice_msg.time,
            })
        }
        "voice_volume" => {
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            let voice_msg: VoiceVolumeMessage = serde_json::from_value(mqtt_cmd.get_params())?;

            Ok(AudioCommand::VoiceVolume {
                voice: voice_msg.voice,
                volume: voice_msg.volume,
            })
        }
        "precache" | "soundPrecache" => {
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            let precache_msg: PrecacheMessage = serde_json::from_value(mqtt_cmd.get_params())?;

            Ok(AudioCommand::Precache {
                file: precache_msg.file,
            })
        }
        "cache_clear" => Ok(AudioCommand::CacheClear),
        "cache_invalidate" => {
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            let invalidate_msg: CacheInvalidateMessage =
                serde_json::from_value(mqtt_cmd.get_params())?;

            Ok(AudioCommand::CacheInvalidate {
                file: invalidate_msg.file,
            })
        }
        "cache_reload" => {
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            let reload_msg: CacheInvalidateMessage = serde_json::from_value(mqtt_cmd.get_params())?;
            Ok(AudioCommand::CacheReload {
                file: reload_msg.file,
            })
        }
        "input_volume" => {
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            let input_msg: InputVolumeMessage = serde_json::from_value(mqtt_cmd.get_params())?;

            Ok(AudioCommand::InputVolume {
                input: input_msg.input,
                volume: input_msg.volume,
            })
        }
        "input_mute" => {
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            let input_msg: InputMuteMessage = serde_json::from_value(mqtt_cmd.get_params())?;

            Ok(AudioCommand::InputMute {
                input: input_msg.input,
                mute: input_msg.mute,
            })
        }
        "talkback_acquire" => {
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            Ok(AudioCommand::TalkbackAcquire(serde_json::from_value(
                mqtt_cmd.get_params(),
            )?))
        }
        "talkback_release" => {
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            Ok(AudioCommand::TalkbackRelease(serde_json::from_value(
                mqtt_cmd.get_params(),
            )?))
        }
        "talkback_hard_mute" => Ok(AudioCommand::TalkbackHardMute),
        "seek" => {
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            let seek_msg: SeekMessage = serde_json::from_value(mqtt_cmd.get_params())?;
            validate_internal_id(&seek_msg.internal_id)?;

            Ok(AudioCommand::Seek {
                selector: SampleSelector {
                    internal_id: seek_msg.internal_id,
                    id: seek_msg.id,
                    file: seek_msg.file,
                    voice: seek_msg.voice,
                },
                position_ms: seek_msg.position_ms,
            })
        }
        "speed" => {
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            let speed_msg: SpeedMessage = serde_json::from_value(mqtt_cmd.get_params())?;
            validate_internal_id(&speed_msg.internal_id)?;

            if speed_msg.speed == 0.0 {
                return Err(ParseError::InvalidParameter(
                    "speed 0 is not supported - use stop to end playback, or a small value like 0.05 to crawl".to_string(),
                ));
            }

            Ok(AudioCommand::Speed {
                selector: SampleSelector {
                    internal_id: speed_msg.internal_id,
                    id: speed_msg.id,
                    file: speed_msg.file,
                    voice: speed_msg.voice,
                },
                speed: speed_msg.speed,
                pitch_correction: speed_msg.pitch_correction,
            })
        }
        "stop" => {
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            let stop_msg: StopMessage = serde_json::from_value(mqtt_cmd.get_params())?;
            validate_internal_id(&stop_msg.internal_id)?;

            Ok(AudioCommand::Stop {
                selector: SampleSelector {
                    internal_id: stop_msg.internal_id,
                    id: stop_msg.id,
                    file: stop_msg.file,
                    voice: stop_msg.voice,
                },
                fade_out_ms: stop_msg.fade_out_ms,
            })
        }
        "volume" => {
            if !mqtt_cmd.has_params() {
                return Err(ParseError::MissingMessage);
            }
            let vol_msg: VolumeMessage = serde_json::from_value(mqtt_cmd.get_params())?;
            validate_internal_id(&vol_msg.internal_id)?;

            Ok(AudioCommand::Volume {
                selector: SampleSelector {
                    internal_id: vol_msg.internal_id,
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
            AudioCommand::Play {
                file,
                volume,
                voice,
                channel_map,
                ..
            } => {
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
            AudioCommand::Play {
                file,
                volume,
                voice,
                channel_map,
                ..
            } => {
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
            AudioCommand::Play {
                file,
                volume,
                voice,
                channel_map,
                ..
            } => {
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
            AudioCommand::Play { .. } => {} // OK
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_command_with_voice() {
        let json = r#"{"command": "play", "message": {"file": "test.wav", "voice": "ambience"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play {
                file,
                volume,
                voice,
                channel_map,
                ..
            } => {
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
            AudioCommand::Play {
                file,
                volume,
                voice,
                channel_map,
                ..
            } => {
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
            ParseError::MissingMessage => {} // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_voice_fade_out_missing_message() {
        let json = r#"{"command": "voice_fade_out"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {} // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_voice_volume_missing_message() {
        let json = r#"{"command": "voice_volume"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {} // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_stopall_command() {
        let json = r#"{"command": "stopall"}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::StopAll => {} // OK
            _ => panic!("Expected StopAll command"),
        }
    }

    #[test]
    fn test_parse_sound_stopall_alias() {
        let json = r#"{"command": "soundStopAll"}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::StopAll => {} // OK
            _ => panic!("Expected StopAll command"),
        }
    }

    #[test]
    fn test_parse_play_allows_boost() {
        let json = r#"{"command": "play", "file": "quiet.wav", "volume": 2.5}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { volume, .. } => assert_eq!(volume, 2.5),
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_clamps_excessive_volume() {
        let json = r#"{"command": "play", "file": "quiet.wav", "volume": 100.0}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { volume, .. } => {
                assert_eq!(volume, crate::config::MAX_GAIN)
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_fadeall_command() {
        let json = r#"{"command": "fadeall", "time": 2000}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::FadeAll { time_ms } => assert_eq!(time_ms, 2000),
            _ => panic!("Expected FadeAll command"),
        }
    }

    #[test]
    fn test_parse_non_numeric_internal_id_is_rejected() {
        // internal_id is the system-assigned numeric id from /status/samples;
        // a non-numeric value can never match anything, so failing loudly
        // beats the silent "matched no samples" it used to produce
        for command in ["stop", "seek", "speed", "volume"] {
            let json = format!(
                r#"{{"command": "{}", "internal_id": "abc", "position_ms": 1, "speed": 1.0, "volume": 1.0}}"#,
                command
            );
            let err = parse_command(&json).unwrap_err();
            assert!(
                err.to_string().contains("internal_id"),
                "{} should reject a non-numeric internal_id, got: {}",
                command,
                err
            );
        }
    }

    #[test]
    fn test_parse_speed_zero_is_rejected() {
        // speed 0 would otherwise be coerced to an unintelligible 0.01x
        // drone; someone sending 0 almost certainly wanted stop or pause
        let json = r#"{"command": "speed", "id": "x", "speed": 0}"#;
        let err = parse_command(json).unwrap_err();
        assert!(
            err.to_string().contains("speed"),
            "the error should explain the speed problem, got: {}",
            err
        );
    }

    #[test]
    fn test_parse_fadeout_aliases() {
        // The legacy app called global fade "fadeout" / "soundFadeOut"
        for command in ["fadeout", "soundFadeOut"] {
            let json = format!(r#"{{"command": "{}", "time": 800}}"#, command);
            let cmd = parse_command(&json).unwrap();

            match cmd {
                AudioCommand::FadeAll { time_ms } => assert_eq!(time_ms, 800),
                _ => panic!("Expected FadeAll command for {}", command),
            }
        }
    }

    #[test]
    fn test_parse_fadeall_nested() {
        let json = r#"{"command": "fadeall", "message": {"time": 1500}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::FadeAll { time_ms } => assert_eq!(time_ms, 1500),
            _ => panic!("Expected FadeAll command"),
        }
    }

    #[test]
    fn test_parse_fadeall_without_time() {
        // "fade everything out" should work as a bare command, the way stopall does
        let json = r#"{"command": "fadeall"}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::FadeAll { time_ms } => assert_eq!(time_ms, DEFAULT_FADE_ALL_MS),
            _ => panic!("Expected FadeAll command"),
        }
    }

    #[test]
    fn test_parse_sound_fadeall_alias() {
        let json = r#"{"command": "soundFadeAll"}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::FadeAll { time_ms } => assert_eq!(time_ms, DEFAULT_FADE_ALL_MS),
            _ => panic!("Expected FadeAll command"),
        }
    }

    #[test]
    fn test_parse_fadeall_accepts_fade_out_ms() {
        // "fade_out_ms" is what stop uses for the same idea
        let json = r#"{"command": "fadeall", "fade_out_ms": 750}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::FadeAll { time_ms } => assert_eq!(time_ms, 750),
            _ => panic!("Expected FadeAll command"),
        }
    }

    #[test]
    fn test_parse_play_missing_message() {
        let json = r#"{"command": "play"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {} // OK
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
            ParseError::JsonError(_) => {} // OK
            e => panic!("Expected JsonError, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_channel_map_stereo() {
        let json = r#"{"command": "play", "message": {"file": "test.wav", "channel_map": [{"src": 0, "dest": 6}, {"src": 1, "dest": 7}]}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play {
                file,
                volume,
                voice,
                channel_map,
                ..
            } => {
                assert_eq!(file, "test.wav");
                assert_eq!(volume, 1.0);
                assert!(voice.is_none());

                let map = channel_map.unwrap();
                assert_eq!(map.len(), 2);
                assert_eq!(
                    map[0],
                    ChannelMapping {
                        src: ChannelRef::Index(0),
                        dest: ChannelRef::Index(6),
                        gain: None,
                    }
                );
                assert_eq!(
                    map[1],
                    ChannelMapping {
                        src: ChannelRef::Index(1),
                        dest: ChannelRef::Index(7),
                        gain: None,
                    }
                );
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
            AudioCommand::Play {
                file, channel_map, ..
            } => {
                assert_eq!(file, "quad.wav");

                let map = channel_map.unwrap();
                assert_eq!(map.len(), 4);
                assert_eq!(
                    map[0],
                    ChannelMapping {
                        src: ChannelRef::Index(0),
                        dest: ChannelRef::Index(0),
                        gain: None,
                    }
                );
                assert_eq!(
                    map[1],
                    ChannelMapping {
                        src: ChannelRef::Index(1),
                        dest: ChannelRef::Index(1),
                        gain: None,
                    }
                );
                assert_eq!(
                    map[2],
                    ChannelMapping {
                        src: ChannelRef::Index(2),
                        dest: ChannelRef::Index(2),
                        gain: None,
                    }
                );
                assert_eq!(
                    map[3],
                    ChannelMapping {
                        src: ChannelRef::Index(3),
                        dest: ChannelRef::Index(3),
                        gain: None,
                    }
                );
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_channel_map_with_per_route_gain() {
        // D29: an optional per-route `gain` parses into ChannelMapping.gain; routes
        // that omit it stay None (resolved to unity downstream).
        let json = r#"{"command": "play", "message": {
            "file": "quad.wav",
            "channel_map": [
                {"src": 2, "dest": 0, "gain": 0.5},
                {"src": 3, "dest": 1}
            ]
        }}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { channel_map, .. } => {
                let map = channel_map.unwrap();
                assert_eq!(map[0].gain, Some(0.5), "explicit gain must parse");
                assert_eq!(map[1].gain, None, "omitted gain stays None");
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
            AudioCommand::Play {
                file,
                volume,
                voice,
                channel_map,
                ..
            } => {
                assert_eq!(file, "surround.wav");
                assert_eq!(volume, 0.7);
                assert_eq!(voice, Some("ambience".to_string()));

                let map = channel_map.unwrap();
                assert_eq!(map.len(), 4);
                assert_eq!(
                    map[0],
                    ChannelMapping {
                        src: ChannelRef::Index(0),
                        dest: ChannelRef::Index(8),
                        gain: None,
                    }
                );
                assert_eq!(
                    map[1],
                    ChannelMapping {
                        src: ChannelRef::Index(1),
                        dest: ChannelRef::Index(9),
                        gain: None,
                    }
                );
                assert_eq!(
                    map[2],
                    ChannelMapping {
                        src: ChannelRef::Index(2),
                        dest: ChannelRef::Index(10),
                        gain: None,
                    }
                );
                assert_eq!(
                    map[3],
                    ChannelMapping {
                        src: ChannelRef::Index(3),
                        dest: ChannelRef::Index(11),
                        gain: None,
                    }
                );
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
                assert_eq!(
                    map[0],
                    ChannelMapping {
                        src: ChannelRef::Index(0),
                        dest: ChannelRef::Index(0),
                        gain: None,
                    }
                );
                assert_eq!(
                    map[1],
                    ChannelMapping {
                        src: ChannelRef::Index(0),
                        dest: ChannelRef::Index(1),
                        gain: None,
                    }
                );
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
                assert_eq!(
                    map[0],
                    ChannelMapping {
                        src: ChannelRef::Index(0),
                        dest: ChannelRef::Alias("front_left".to_string()),
                        gain: None,
                    }
                );
                assert_eq!(
                    map[1],
                    ChannelMapping {
                        src: ChannelRef::Index(1),
                        dest: ChannelRef::Alias("front_right".to_string()),
                        gain: None,
                    }
                );
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
                assert_eq!(
                    map[0],
                    ChannelMapping {
                        src: ChannelRef::Alias("left".to_string()),
                        dest: ChannelRef::Alias("speaker_1".to_string()),
                        gain: None,
                    }
                );
                assert_eq!(
                    map[1],
                    ChannelMapping {
                        src: ChannelRef::Alias("right".to_string()),
                        dest: ChannelRef::Alias("speaker_2".to_string()),
                        gain: None,
                    }
                );
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_precache_command() {
        let json =
            r#"{"command": "precache", "message": {"file": "http://example.com/bigfile.wav"}}"#;
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
            ParseError::MissingMessage => {} // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_cache_clear_command() {
        let json = r#"{"command": "cache_clear"}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::CacheClear => {} // OK
            _ => panic!("Expected CacheClear command"),
        }
    }

    #[test]
    fn test_parse_cache_invalidate_command() {
        let json =
            r#"{"command": "cache_invalidate", "message": {"file": "http://example.com/old.wav"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::CacheInvalidate { file } => {
                assert_eq!(file, "http://example.com/old.wav");
            }
            _ => panic!("Expected CacheInvalidate command"),
        }
    }

    #[test]
    fn test_parse_cache_reload_command() {
        let json = r#"{"command": "cache_reload", "message": {"file": "/sounds/x.wav"}}"#;
        match parse_command(json).unwrap() {
            AudioCommand::CacheReload { file } => assert_eq!(file, "/sounds/x.wav"),
            _ => panic!("Expected CacheReload command"),
        }
    }

    #[test]
    fn test_parse_cache_invalidate_missing_message() {
        let json = r#"{"command": "cache_invalidate"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {} // OK
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
        let json =
            r#"{"command": "input_volume", "message": {"input": "gamemaster_mic", "volume": 0.8}}"#;
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
            ParseError::MissingMessage => {} // OK
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
                assert!(mute);
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
                assert!(!mute);
            }
            _ => panic!("Expected InputMute command"),
        }
    }

    #[test]
    fn test_parse_talkback_lease_commands() {
        let acquire = parse_command(r#"{"command":"talkback_acquire","message":{"client_id":"gm-1","destination":"GUEST_ALL","gain":0.0,"lease_ms":500}}"#).unwrap();
        match acquire {
            AudioCommand::TalkbackAcquire(request) => {
                assert_eq!(request.client_id, "gm-1");
                assert_eq!(request.source_id, "GM_MIC");
                assert_eq!(request.lease_ms, 500);
            }
            _ => panic!("Expected TalkbackAcquire command"),
        }
        let release = parse_command(r#"{"command":"talkback_release","message":{"client_id":"gm-1","lease_id":"lease-0001"}}"#).unwrap();
        assert!(matches!(release, AudioCommand::TalkbackRelease(_)));
        assert!(matches!(
            parse_command(r#"{"command":"talkback_hard_mute"}"#).unwrap(),
            AudioCommand::TalkbackHardMute
        ));
    }

    #[test]
    fn test_parse_input_mute_missing_message() {
        let json = r#"{"command": "input_mute"}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::MissingMessage => {} // OK
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
            AudioCommand::Play {
                file,
                id,
                voice,
                volume,
                fade_in,
                start_position_ms,
                channel_map,
                ..
            } => {
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
            AudioCommand::Play {
                file,
                start_position_ms,
                ..
            } => {
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
            AudioCommand::Play {
                start_position_ms, ..
            } => {
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
            AudioCommand::Play {
                file, loop_mode, ..
            } => {
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
            AudioCommand::Play {
                file, loop_mode, ..
            } => {
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
            AudioCommand::Play {
                file,
                loop_mode,
                crossfade_ms,
                ..
            } => {
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
    fn test_parse_play_with_stream_mode_and_window() {
        let json = r#"{"command": "play", "message": {"file": "long.wav", "mode": "stream", "window_ms": 2000, "prebuffer_ms": 100}}"#;
        match parse_command(json).unwrap() {
            AudioCommand::Play {
                mode,
                window_ms,
                prebuffer_ms,
                ..
            } => {
                assert_eq!(mode, LoadMode::Stream);
                assert_eq!(window_ms, Some(2000));
                assert_eq!(prebuffer_ms, Some(100));
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_cacheable_override() {
        // `cacheable: false` tags an HTTP source live (window, never persist).
        let json = r#"{"command": "play", "message": {"file": "https://example.com/live.wav", "cacheable": false}}"#;
        match parse_command(json).unwrap() {
            AudioCommand::Play { cacheable, .. } => assert_eq!(cacheable, Some(false)),
            _ => panic!("Expected Play command"),
        }
        // Omitting it is backward compatible and resolves to None (cacheable default).
        let json = r#"{"command": "play", "message": {"file": "https://example.com/cue.wav"}}"#;
        match parse_command(json).unwrap() {
            AudioCommand::Play { cacheable, .. } => assert_eq!(cacheable, None),
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_parse_play_defaults_mode_to_auto() {
        // Omitting mode/window/prebuffer is backward compatible and resolves to Auto.
        let json = r#"{"command": "play", "message": {"file": "sfx.wav"}}"#;
        match parse_command(json).unwrap() {
            AudioCommand::Play {
                mode,
                window_ms,
                prebuffer_ms,
                ..
            } => {
                assert_eq!(mode, LoadMode::Auto);
                assert_eq!(window_ms, None);
                assert_eq!(prebuffer_ms, None);
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
    fn test_selector_matches_by_internal_id() {
        let selector = SampleSelector {
            internal_id: Some("42".to_string()),
            id: None,
            file: None,
            voice: None,
        };

        assert!(selector.matches(42, None, "test.wav", "voice1"));
        assert!(!selector.matches(99, None, "test.wav", "voice1"));
    }

    #[test]
    fn test_selector_matches_by_id() {
        let selector = SampleSelector {
            internal_id: None,
            id: Some("my-sound".to_string()),
            file: None,
            voice: None,
        };

        assert!(selector.matches(1, Some("my-sound"), "test.wav", "voice1"));
        assert!(!selector.matches(1, Some("other-sound"), "test.wav", "voice1"));
        assert!(!selector.matches(1, None, "test.wav", "voice1"));
    }

    #[test]
    fn test_selector_matches_by_file() {
        let selector = SampleSelector {
            internal_id: None,
            id: None,
            file: Some("music.mp3".to_string()),
            voice: None,
        };

        assert!(selector.matches(1, None, "music.mp3", "voice1"));
        assert!(selector.matches(1, Some("any-id"), "music.mp3", "voice1"));
        assert!(!selector.matches(1, None, "other.mp3", "voice1"));
    }

    #[test]
    fn test_selector_matches_by_voice() {
        let selector = SampleSelector {
            internal_id: None,
            id: None,
            file: None,
            voice: Some("background".to_string()),
        };

        assert!(selector.matches(1, None, "any.wav", "background"));
        assert!(!selector.matches(1, None, "any.wav", "foreground"));
    }

    #[test]
    fn test_selector_matches_any_criterion() {
        // Selector with multiple criteria matches if ANY matches
        let selector = SampleSelector {
            internal_id: None,
            id: Some("specific-sound".to_string()),
            file: Some("music.mp3".to_string()),
            voice: None,
        };

        // Matches by id
        assert!(selector.matches(1, Some("specific-sound"), "other.wav", "voice1"));
        // Matches by file
        assert!(selector.matches(1, Some("other-id"), "music.mp3", "voice1"));
        // Matches neither
        assert!(!selector.matches(1, Some("other-id"), "other.wav", "voice1"));
    }

    #[test]
    fn test_selector_empty() {
        let empty = SampleSelector {
            internal_id: None,
            id: None,
            file: None,
            voice: None,
        };
        assert!(empty.is_empty());
        assert!(!empty.matches(1, Some("any"), "any", "any"));

        let not_empty = SampleSelector {
            internal_id: None,
            id: Some("test".to_string()),
            file: None,
            voice: None,
        };
        assert!(!not_empty.is_empty());

        let not_empty_internal = SampleSelector {
            internal_id: Some("1".to_string()),
            id: None,
            file: None,
            voice: None,
        };
        assert!(!not_empty_internal.is_empty());
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
            AudioCommand::Seek {
                selector,
                position_ms,
            } => {
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
            AudioCommand::Seek {
                selector,
                position_ms,
            } => {
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
            AudioCommand::Seek {
                selector,
                position_ms,
            } => {
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
            AudioCommand::Seek {
                selector,
                position_ms,
            } => {
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
            ParseError::MissingMessage => {} // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_seek_missing_position() {
        let json = r#"{"command": "seek", "message": {"id": "my-sound"}}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::JsonError(_) => {} // OK - position_ms is required
            e => panic!("Expected JsonError, got: {:?}", e),
        }
    }

    // === Stop Command Tests (with selector) ===

    #[test]
    fn test_parse_stop_by_id() {
        let json = r#"{"command": "stop", "message": {"id": "effect-1"}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Stop {
                selector,
                fade_out_ms,
            } => {
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
            AudioCommand::Stop {
                selector,
                fade_out_ms,
            } => {
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
            AudioCommand::Speed {
                selector,
                speed,
                pitch_correction,
            } => {
                assert_eq!(selector.id, Some("music-track".to_string()));
                assert_eq!(speed, 1.5);
                assert!(!pitch_correction); // default
            }
            _ => panic!("Expected Speed command"),
        }
    }

    #[test]
    fn test_parse_speed_by_voice() {
        let json = r#"{"command": "speed", "message": {"voice": "background", "speed": 0.5}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Speed {
                selector,
                speed,
                pitch_correction,
            } => {
                assert_eq!(selector.voice, Some("background".to_string()));
                assert_eq!(speed, 0.5);
                assert!(!pitch_correction);
            }
            _ => panic!("Expected Speed command"),
        }
    }

    #[test]
    fn test_parse_speed_with_pitch_correction() {
        let json = r#"{"command": "speed", "message": {"file": "music.mp3", "speed": 2.0, "pitch_correction": true}}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Speed {
                selector,
                speed,
                pitch_correction,
            } => {
                assert_eq!(selector.file, Some("music.mp3".to_string()));
                assert_eq!(speed, 2.0);
                assert!(pitch_correction);
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
            ParseError::MissingMessage => {} // OK
            e => panic!("Expected MissingMessage error, got: {:?}", e),
        }
    }

    #[test]
    fn test_parse_speed_missing_speed_value() {
        let json = r#"{"command": "speed", "message": {"id": "my-sound"}}"#;
        let result = parse_command(json);

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::JsonError(_) => {} // OK - speed is required
            e => panic!("Expected JsonError, got: {:?}", e),
        }
    }

    // ==========================================================================
    // Flattened Format Tests
    // ==========================================================================
    // These tests verify that the flattened JSON format (parameters at root level)
    // works alongside the legacy nested format (parameters in "message" object).

    #[test]
    fn test_flat_play_command_minimal() {
        let json = r#"{"command": "play", "file": "test.wav"}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play {
                file,
                volume,
                voice,
                ..
            } => {
                assert_eq!(file, "test.wav");
                assert_eq!(volume, 1.0);
                assert!(voice.is_none());
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_flat_play_command_with_all_fields() {
        let json = r#"{
            "command": "play",
            "file": "music.mp3",
            "id": "bg-music",
            "voice": "background",
            "volume": 0.5,
            "fade_in": 2000,
            "loop": true,
            "crossfade_ms": 100
        }"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play {
                file,
                id,
                voice,
                volume,
                fade_in,
                loop_mode,
                crossfade_ms,
                ..
            } => {
                assert_eq!(file, "music.mp3");
                assert_eq!(id, Some("bg-music".to_string()));
                assert_eq!(voice, Some("background".to_string()));
                assert_eq!(volume, 0.5);
                assert_eq!(fade_in, Some(2000));
                assert!(loop_mode);
                assert_eq!(crossfade_ms, 100);
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_flat_stopall_command() {
        let json = r#"{"command": "stopall"}"#;
        let cmd = parse_command(json).unwrap();
        assert!(matches!(cmd, AudioCommand::StopAll));
    }

    #[test]
    fn test_flat_voice_stop() {
        let json = r#"{"command": "voice_stop", "voice": "music"}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::VoiceStop { voice } => {
                assert_eq!(voice, "music");
            }
            _ => panic!("Expected VoiceStop command"),
        }
    }

    #[test]
    fn test_flat_voice_fade_out() {
        let json = r#"{"command": "voice_fade_out", "voice": "ambient", "time": 3000}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::VoiceFadeOut { voice, time_ms } => {
                assert_eq!(voice, "ambient");
                assert_eq!(time_ms, 3000);
            }
            _ => panic!("Expected VoiceFadeOut command"),
        }
    }

    #[test]
    fn test_flat_voice_volume() {
        let json = r#"{"command": "voice_volume", "voice": "effects", "volume": 0.3}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::VoiceVolume { voice, volume } => {
                assert_eq!(voice, "effects");
                assert_eq!(volume, 0.3);
            }
            _ => panic!("Expected VoiceVolume command"),
        }
    }

    #[test]
    fn test_flat_precache() {
        let json = r#"{"command": "precache", "file": "https://example.com/audio.mp3"}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Precache { file } => {
                assert_eq!(file, "https://example.com/audio.mp3");
            }
            _ => panic!("Expected Precache command"),
        }
    }

    #[test]
    fn test_flat_cache_clear() {
        let json = r#"{"command": "cache_clear"}"#;
        let cmd = parse_command(json).unwrap();
        assert!(matches!(cmd, AudioCommand::CacheClear));
    }

    #[test]
    fn test_flat_cache_invalidate() {
        let json = r#"{"command": "cache_invalidate", "file": "/sounds/old.wav"}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::CacheInvalidate { file } => {
                assert_eq!(file, "/sounds/old.wav");
            }
            _ => panic!("Expected CacheInvalidate command"),
        }
    }

    #[test]
    fn test_flat_input_volume() {
        let json = r#"{"command": "input_volume", "input": "mic1", "volume": 0.8}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::InputVolume { input, volume } => {
                assert_eq!(input, "mic1");
                assert_eq!(volume, 0.8);
            }
            _ => panic!("Expected InputVolume command"),
        }
    }

    #[test]
    fn test_flat_input_mute() {
        let json = r#"{"command": "input_mute", "input": "0", "mute": true}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::InputMute { input, mute } => {
                assert_eq!(input, "0");
                assert!(mute);
            }
            _ => panic!("Expected InputMute command"),
        }
    }

    #[test]
    fn test_flat_seek() {
        let json = r#"{"command": "seek", "id": "track1", "position_ms": 60000}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Seek {
                selector,
                position_ms,
            } => {
                assert_eq!(selector.id, Some("track1".to_string()));
                assert_eq!(position_ms, 60000);
            }
            _ => panic!("Expected Seek command"),
        }
    }

    #[test]
    fn test_flat_speed() {
        let json =
            r#"{"command": "speed", "id": "playback", "speed": 1.5, "pitch_correction": true}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Speed {
                selector,
                speed,
                pitch_correction,
            } => {
                assert_eq!(selector.id, Some("playback".to_string()));
                assert_eq!(speed, 1.5);
                assert!(pitch_correction);
            }
            _ => panic!("Expected Speed command"),
        }
    }

    #[test]
    fn test_flat_stop() {
        let json = r#"{"command": "stop", "voice": "effects", "fade_out_ms": 500}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Stop {
                selector,
                fade_out_ms,
            } => {
                assert_eq!(selector.voice, Some("effects".to_string()));
                assert_eq!(fade_out_ms, Some(500));
            }
            _ => panic!("Expected Stop command"),
        }
    }

    #[test]
    fn test_flat_volume() {
        let json = r#"{"command": "volume", "file": "music.mp3", "volume": 0.4}"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Volume { selector, volume } => {
                assert_eq!(selector.file, Some("music.mp3".to_string()));
                assert_eq!(volume, 0.4);
            }
            _ => panic!("Expected Volume command"),
        }
    }

    #[test]
    fn test_flat_play_with_channel_map() {
        let json = r#"{
            "command": "play",
            "file": "stereo.wav",
            "channel_map": [
                {"src": 0, "dest": 2},
                {"src": 1, "dest": 3}
            ]
        }"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play {
                file, channel_map, ..
            } => {
                assert_eq!(file, "stereo.wav");
                let map = channel_map.unwrap();
                assert_eq!(map.len(), 2);
                assert_eq!(
                    map[0],
                    ChannelMapping {
                        src: ChannelRef::Index(0),
                        dest: ChannelRef::Index(2),
                        gain: None,
                    }
                );
                assert_eq!(
                    map[1],
                    ChannelMapping {
                        src: ChannelRef::Index(1),
                        dest: ChannelRef::Index(3),
                        gain: None,
                    }
                );
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_nested_format_takes_precedence() {
        // When both "message" and flat params exist, "message" should be used
        let json = r#"{
            "command": "play",
            "file": "flat.wav",
            "volume": 0.1,
            "message": {"file": "nested.wav", "volume": 0.9}
        }"#;
        let cmd = parse_command(json).unwrap();

        match cmd {
            AudioCommand::Play { file, volume, .. } => {
                assert_eq!(file, "nested.wav");
                assert_eq!(volume, 0.9);
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_both_formats_produce_same_result() {
        let nested =
            r#"{"command": "play", "message": {"file": "test.wav", "volume": 0.7, "voice": "fx"}}"#;
        let flat = r#"{"command": "play", "file": "test.wav", "volume": 0.7, "voice": "fx"}"#;

        let cmd_nested = parse_command(nested).unwrap();
        let cmd_flat = parse_command(flat).unwrap();

        match (cmd_nested, cmd_flat) {
            (
                AudioCommand::Play {
                    file: f1,
                    volume: v1,
                    voice: voice1,
                    ..
                },
                AudioCommand::Play {
                    file: f2,
                    volume: v2,
                    voice: voice2,
                    ..
                },
            ) => {
                assert_eq!(f1, f2);
                assert_eq!(v1, v2);
                assert_eq!(voice1, voice2);
            }
            _ => panic!("Expected matching Play commands"),
        }
    }

    // ==========================================================================
    // Macro Expansion Tests
    // ==========================================================================

    use std::collections::HashMap;

    fn make_macros() -> HashMap<String, serde_json::Value> {
        let mut macros = HashMap::new();
        macros.insert(
            "wholeroom".to_string(),
            serde_json::json!({
                "channel_map": [{"src": 0, "dest": "left"}, {"src": 1, "dest": "right"}],
                "volume": 0.2
            }),
        );
        macros.insert(
            "quiet".to_string(),
            serde_json::json!({
                "volume": 0.1
            }),
        );
        macros.insert(
            "music_voice".to_string(),
            serde_json::json!({
                "voice": "music"
            }),
        );
        macros
    }

    #[test]
    fn test_expand_macros_single_macro() {
        let macros = make_macros();
        let json = r#"{"command": "play", "file": "test.mp3", "macro": "wholeroom"}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let value: serde_json::Value = serde_json::from_str(&expanded).unwrap();

        assert_eq!(value["command"], "play");
        assert_eq!(value["file"], "test.mp3");
        assert_eq!(value["volume"], 0.2);
        assert!(value["channel_map"].is_array());
        assert!(value.get("macro").is_none()); // macro field should be removed
    }

    #[test]
    fn test_expand_macros_command_overrides_macro() {
        let macros = make_macros();
        let json =
            r#"{"command": "play", "file": "test.mp3", "macro": "wholeroom", "volume": 0.3}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let value: serde_json::Value = serde_json::from_str(&expanded).unwrap();

        assert_eq!(value["volume"], 0.3); // Command value takes precedence
        assert!(value["channel_map"].is_array()); // Macro value still applied
    }

    #[test]
    fn test_expand_macros_multiple_macros_precedence() {
        let macros = make_macros();
        // Earlier macros take precedence over later ones
        let json = r#"{"command": "play", "file": "test.mp3", "macro": ["quiet", "wholeroom"]}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let value: serde_json::Value = serde_json::from_str(&expanded).unwrap();

        assert_eq!(value["volume"], 0.1); // "quiet" macro takes precedence (first in array)
        assert!(value["channel_map"].is_array()); // "wholeroom" channel_map still applied
    }

    #[test]
    fn test_expand_macros_later_macro_fills_gaps() {
        let macros = make_macros();
        // music_voice has voice, wholeroom has volume and channel_map
        let json =
            r#"{"command": "play", "file": "test.mp3", "macro": ["music_voice", "wholeroom"]}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let value: serde_json::Value = serde_json::from_str(&expanded).unwrap();

        assert_eq!(value["voice"], "music"); // From music_voice
        assert_eq!(value["volume"], 0.2); // From wholeroom
        assert!(value["channel_map"].is_array()); // From wholeroom
    }

    #[test]
    fn test_expand_macros_no_macro_field() {
        let macros = make_macros();
        let json = r#"{"command": "play", "file": "test.mp3"}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let value: serde_json::Value = serde_json::from_str(&expanded).unwrap();

        assert_eq!(value["command"], "play");
        assert_eq!(value["file"], "test.mp3");
        assert!(value.get("volume").is_none());
    }

    #[test]
    fn test_expand_macros_unknown_macro_ignored() {
        let macros = make_macros();
        let json = r#"{"command": "play", "file": "test.mp3", "macro": "nonexistent"}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let value: serde_json::Value = serde_json::from_str(&expanded).unwrap();

        assert_eq!(value["command"], "play");
        assert_eq!(value["file"], "test.mp3");
        assert!(value.get("volume").is_none()); // No macro params added
    }

    #[test]
    fn test_expand_macros_empty_macros_map() {
        let macros = HashMap::new();
        let json = r#"{"command": "play", "file": "test.mp3", "macro": "anything"}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let value: serde_json::Value = serde_json::from_str(&expanded).unwrap();

        assert_eq!(value["command"], "play");
        assert_eq!(value["file"], "test.mp3");
        assert!(value.get("macro").is_none()); // macro field still removed
    }

    #[test]
    fn test_expand_macros_invalid_json() {
        let macros = make_macros();
        let json = r#"not valid json"#;

        let result = expand_macros(json, &macros);
        assert!(result.is_err());
    }

    #[test]
    fn test_expand_macros_preserves_nested_message() {
        let macros = make_macros();
        // Macro should work with legacy nested format too
        let json = r#"{"command": "play", "message": {"file": "test.mp3"}, "macro": "quiet"}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let value: serde_json::Value = serde_json::from_str(&expanded).unwrap();

        assert_eq!(value["command"], "play");
        assert!(value["message"].is_object());
        // Parameters for the nested format are read from "message", so macro
        // params must land there to take effect
        assert_eq!(value["message"]["volume"], 0.1);
        assert_eq!(value["message"]["file"], "test.mp3");
    }

    #[test]
    fn test_expand_macros_nested_message_params_win_over_macro() {
        let macros = make_macros();
        let json = r#"{"command": "play", "message": {"file": "test.mp3", "volume": 0.9}, "macro": "quiet"}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let value: serde_json::Value = serde_json::from_str(&expanded).unwrap();

        assert_eq!(value["message"]["volume"], 0.9); // Explicit param beats macro
    }

    #[test]
    fn test_expand_macros_nested_message_integration_with_parse() {
        let macros = make_macros();
        let json = r#"{"command": "play", "message": {"file": "test.mp3"}, "macro": "quiet"}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let cmd = parse_command(&expanded).unwrap();

        match cmd {
            AudioCommand::Play { volume, .. } => assert_eq!(volume, 0.1),
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_expand_macros_integration_with_parse() {
        let macros = make_macros();
        let json = r#"{"command": "play", "file": "test.mp3", "macro": "quiet"}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let cmd = parse_command(&expanded).unwrap();

        match cmd {
            AudioCommand::Play { file, volume, .. } => {
                assert_eq!(file, "test.mp3");
                assert_eq!(volume, 0.1); // From quiet macro
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_expand_macros_command_override_integration() {
        let macros = make_macros();
        let json = r#"{"command": "play", "file": "test.mp3", "macro": "quiet", "volume": 0.5}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let cmd = parse_command(&expanded).unwrap();

        match cmd {
            AudioCommand::Play { file, volume, .. } => {
                assert_eq!(file, "test.mp3");
                assert_eq!(volume, 0.5); // Command override
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_expand_macros_multiple_integration() {
        let macros = make_macros();
        let json = r#"{"command": "play", "file": "test.mp3", "macro": ["music_voice", "quiet"]}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let cmd = parse_command(&expanded).unwrap();

        match cmd {
            AudioCommand::Play {
                file,
                volume,
                voice,
                ..
            } => {
                assert_eq!(file, "test.mp3");
                assert_eq!(volume, 0.1); // From quiet macro
                assert_eq!(voice, Some("music".to_string())); // From music_voice macro
            }
            _ => panic!("Expected Play command"),
        }
    }

    #[test]
    fn test_expand_macros_array_with_invalid_entries() {
        let macros = make_macros();
        // Mix of valid strings and invalid entries in array
        let json = r#"{"command": "play", "file": "test.mp3", "macro": ["quiet", 123, "music_voice", null]}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let value: serde_json::Value = serde_json::from_str(&expanded).unwrap();

        assert_eq!(value["volume"], 0.1); // quiet still applied
        assert_eq!(value["voice"], "music"); // music_voice still applied
    }

    #[test]
    fn test_expand_macros_invalid_macro_type() {
        let macros = make_macros();
        // macro field is neither string nor array
        let json = r#"{"command": "play", "file": "test.mp3", "macro": 123}"#;

        let expanded = expand_macros(json, &macros).unwrap();
        let value: serde_json::Value = serde_json::from_str(&expanded).unwrap();

        // Should just remove the invalid macro field and continue
        assert_eq!(value["command"], "play");
        assert_eq!(value["file"], "test.mp3");
        assert!(value.get("macro").is_none());
    }
}
