// ABOUTME: Declarative field registry and sparse JSON document model for the config editor.
// ABOUTME: Maps every config field to a JSON path, edit kind, bounds, and help text.

use crate::config::Config;
use serde_json::Value;

/// One segment of a path into the config JSON document.
#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    Key(String),
    Idx(usize),
}

/// Build a path from static key segments.
pub fn path_of(keys: &[&str]) -> Vec<Seg> {
    keys.iter().map(|k| Seg::Key(k.to_string())).collect()
}

/// The config file being edited, kept as raw JSON so that only explicitly-set
/// keys are written on save: untouched fields stay absent (the daemon's
/// compiled-in defaults apply) and unknown keys/comments pass through unchanged.
#[derive(Clone, Debug)]
pub struct ConfigDocument {
    root: Value,
}

impl ConfigDocument {
    /// An empty document (a new config).
    pub fn new() -> Self {
        Self {
            root: Value::Object(serde_json::Map::new()),
        }
    }

    /// Wrap an already-parsed config file. Fails unless the root is an object.
    pub fn from_value(root: Value) -> Result<Self, String> {
        if root.is_object() {
            Ok(Self { root })
        } else {
            Err("config root must be a JSON object".to_string())
        }
    }

    /// Parse a config file's text.
    pub fn parse(text: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        Self::from_value(root)
    }

    /// The raw document. Used by the integration tests to inspect what a save
    /// would write; the binary's editor goes through the typed accessors, so
    /// the binary build would otherwise flag it dead.
    #[allow(dead_code)]
    pub fn root(&self) -> &Value {
        &self.root
    }

    /// The value at `path`, if explicitly set.
    pub fn get(&self, path: &[Seg]) -> Option<&Value> {
        let mut cur = &self.root;
        for seg in path {
            cur = match seg {
                Seg::Key(k) => cur.as_object()?.get(k)?,
                Seg::Idx(i) => cur.as_array()?.get(*i)?,
            };
        }
        Some(cur)
    }

    /// Set the value at `path`, creating intermediate objects as needed.
    /// Intermediate array indices must already exist (lists grow via `push`).
    pub fn set(&mut self, path: &[Seg], value: Value) {
        let Some((last, parents)) = path.split_last() else {
            return;
        };
        let mut cur = &mut self.root;
        for seg in parents {
            match seg {
                Seg::Key(k) => {
                    if !cur.is_object() {
                        *cur = Value::Object(serde_json::Map::new());
                    }
                    cur = cur
                        .as_object_mut()
                        .expect("just ensured object")
                        .entry(k.clone())
                        .or_insert_with(|| Value::Object(serde_json::Map::new()));
                }
                Seg::Idx(i) => {
                    let Some(arr) = cur.as_array_mut() else {
                        return;
                    };
                    let Some(next) = arr.get_mut(*i) else {
                        return;
                    };
                    cur = next;
                }
            }
        }
        match last {
            Seg::Key(k) => {
                if !cur.is_object() {
                    *cur = Value::Object(serde_json::Map::new());
                }
                cur.as_object_mut()
                    .expect("just ensured object")
                    .insert(k.clone(), value);
            }
            Seg::Idx(i) => {
                if let Some(arr) = cur.as_array_mut() {
                    if let Some(slot) = arr.get_mut(*i) {
                        *slot = value;
                    }
                }
            }
        }
    }

    /// Append `value` to the array at `path`, creating the array if absent.
    pub fn push(&mut self, path: &[Seg], value: Value) {
        match self.get(path) {
            Some(Value::Array(_)) => {}
            _ => self.set(path, Value::Array(Vec::new())),
        }
        if let Some(Value::Array(arr)) = self.get_mut(path) {
            arr.push(value);
        }
    }

    fn get_mut(&mut self, path: &[Seg]) -> Option<&mut Value> {
        let mut cur = &mut self.root;
        for seg in path {
            cur = match seg {
                Seg::Key(k) => cur.as_object_mut()?.get_mut(k)?,
                Seg::Idx(i) => cur.as_array_mut()?.get_mut(*i)?,
            };
        }
        Some(cur)
    }

    /// Remove the key (or array element) at `path`, pruning any parent objects
    /// left empty (an empty section object deserializes the same as an absent
    /// one, so pruning keeps saved files sparse without changing meaning).
    pub fn unset(&mut self, path: &[Seg]) {
        let Some((last, parents)) = path.split_last() else {
            return;
        };
        if let Some(parent) = self.get_mut(parents) {
            match last {
                Seg::Key(k) => {
                    if let Some(obj) = parent.as_object_mut() {
                        obj.remove(k);
                    }
                }
                Seg::Idx(i) => {
                    if let Some(arr) = parent.as_array_mut() {
                        if *i < arr.len() {
                            arr.remove(*i);
                        }
                    }
                }
            }
        }
        // Prune empty parent objects (never the root, never arrays: an empty
        // array may be meaningful, e.g. clearing a list).
        for depth in (1..path.len()).rev() {
            let parent_path = &path[..depth];
            let is_empty_obj =
                matches!(self.get(parent_path), Some(Value::Object(o)) if o.is_empty());
            if is_empty_obj {
                let Some((last, parents)) = parent_path.split_last() else {
                    break;
                };
                if let (Some(Value::Object(obj)), Seg::Key(k)) = (self.get_mut(parents), last) {
                    obj.remove(k);
                }
            } else {
                break;
            }
        }
    }

    /// Number of elements in the array at `path` (0 when absent).
    pub fn list_len(&self, path: &[Seg]) -> usize {
        match self.get(path) {
            Some(Value::Array(arr)) => arr.len(),
            _ => 0,
        }
    }

    /// Deserialize this document into a `Config` exactly as the daemon would,
    /// so compiled-in defaults fill every unset field identically.
    pub fn to_config(&self) -> Result<Config, String> {
        serde_json::from_value(self.root.clone()).map_err(|e| e.to_string())
    }

    /// Pretty-printed JSON with a trailing newline, ready to write to disk.
    pub fn to_pretty_string(&self) -> String {
        let mut s = serde_json::to_string_pretty(&self.root).unwrap_or_else(|_| "{}".to_string());
        s.push('\n');
        s
    }
}

impl Default for ConfigDocument {
    fn default() -> Self {
        Self::new()
    }
}

/// How a field is edited and displayed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldKind {
    /// A required string (always has a default).
    Text,
    /// An optional string; unset means the daemon default (often "none").
    OptionalText,
    /// An optional string rendered masked.
    Secret,
    /// A boolean toggle.
    Bool,
    /// Presence of an object acts as an on/off switch ({} = on, absent = off).
    Flag,
    /// An unsigned integer within [min, max].
    UInt { min: u64, max: u64 },
    /// An optional unsigned integer within [min, max]; unset = automatic.
    OptionalUInt { min: u64, max: u64 },
    /// A float within [min, max].
    Float { min: f64, max: f64 },
    /// One of a fixed set of lowercase string options.
    Enum(&'static [&'static str]),
    /// The audio output device name; edited via the device picker.
    OutputDevice,
    /// An audio input device name; edited via the input device picker.
    InputDevice,
    /// A list of strings.
    StringList,
    /// A map of string keys to unsigned integers (channel aliases).
    MapToUInt,
    /// A map of string keys to floats within [min, max] (channel volumes).
    MapToFloat { min: f64, max: f64 },
    /// A map of string keys to arbitrary JSON values (macros).
    MapToJson,
    /// A channel reference: numeric index or alias string.
    ChannelRef,
    /// A list of channel references.
    ChannelRefList,
    /// A voice name; the picker offers voice ids already in the config, free text allowed.
    VoiceRef,
    /// A list of voice names.
    VoiceRefList,
    /// The cache memory budget (tagged mode: auto/explicit/unlimited).
    MemoryBudget,
    /// A list of structs edited via nested sub-forms.
    StructList(&'static StructListMeta),
}

/// A field of a struct inside a list (ducking rules, inputs, routes).
#[derive(Debug, PartialEq)]
pub struct SubFieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub help: &'static str,
}

/// A list-of-structs field: its sub-fields plus how items are named, created,
/// and summarized in the list view.
#[derive(Debug)]
pub struct StructListMeta {
    pub specs: &'static [SubFieldSpec],
    /// Noun for one item ("rule", "input", "route").
    pub item_noun: &'static str,
    /// One-line summary of an item for the list view.
    pub summarize: fn(&Value) -> String,
    /// A new item that deserializes: required fields get placeholder values.
    pub skeleton: fn() -> Value,
    /// Default values shown for unset sub-form fields.
    pub item_defaults: fn() -> Value,
}

/// The metas are statics, so two are equal exactly when they are the same one.
impl PartialEq for StructListMeta {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// The editor's sections, in sidebar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Mqtt,
    Audio,
    Cache,
    Security,
    Logging,
    Http,
    Ducking,
    BassManagement,
    Inputs,
    Advanced,
    Macros,
}

impl Section {
    pub const ALL: [Section; 11] = [
        Section::Mqtt,
        Section::Audio,
        Section::Cache,
        Section::Security,
        Section::Logging,
        Section::Http,
        Section::Ducking,
        Section::BassManagement,
        Section::Inputs,
        Section::Advanced,
        Section::Macros,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            Section::Mqtt => "MQTT",
            Section::Audio => "Audio",
            Section::Cache => "Cache",
            Section::Security => "Security",
            Section::Logging => "Logging",
            Section::Http => "HTTP",
            Section::Ducking => "Ducking Rules",
            Section::BassManagement => "Bass Management",
            Section::Inputs => "Inputs",
            Section::Advanced => "Advanced",
            Section::Macros => "Macros",
        }
    }

    /// A short explanation of the section, shown in the help pane and above
    /// the section's form.
    pub fn intro(&self) -> &'static str {
        match self {
            Section::Mqtt => {
                "How mqttaudio connects to your MQTT broker and which topic it listens on \
                 for commands. If you only use the HTTP API, the topic can stay unset."
            }
            Section::Audio => {
                "The output device and how sound leaves it: sample rate, channel count, \
                 per-channel names and calibration, and the output limiter. The device \
                 picker can play test tones through each speaker."
            }
            Section::Cache => {
                "How decoded audio is kept ready to play: the on-disk cache for remote \
                 files, what to preload at startup, memory limits, and when large files \
                 are streamed instead of fully loaded."
            }
            Section::Security => {
                "Limits which local files MQTT/HTTP commands are allowed to play. With no \
                 entries, any readable path on the machine can be played."
            }
            Section::Logging => {
                "What mqttaudio writes to its console log, in which format, and whether \
                 log records are also published to an MQTT topic."
            }
            Section::Http => {
                "The optional HTTP server: REST endpoints mirroring every MQTT command, \
                 plus a WebSocket for live logs. Useful for web UIs and testing without \
                 a broker."
            }
            Section::Ducking => {
                "Ducking automatically lowers some sounds while another plays — for \
                 example, quiet the background music while an announcement is speaking. \
                 Sounds are grouped by voice name: the \"voice\" parameter of a Play \
                 command, or an input's voice_id."
            }
            Section::BassManagement => {
                "Extracts low frequencies from your main channels and routes them to a \
                 subwoofer (LFE) channel using a proper crossover. Use this when your \
                 main speakers are small and a subwoofer handles the bass."
            }
            Section::Inputs => {
                "Live inputs (microphones, line-in) mixed into the output in real time, \
                 with channel routing and a voice name so ducking rules can react to \
                 them. The device picker shows a live level meter."
            }
            Section::Advanced => {
                "Settings that rarely need changing: resampling quality and the config \
                 schema version marker."
            }
            Section::Macros => {
                "Macros are named bundles of command parameters. A command that includes \
                 \"macro\": \"name\" is merged with the bundle, so you can define \
                 presets like \"quiet\" once and reuse them from every sender."
            }
        }
    }
}

/// A top-level config field: where it lives in the JSON document, how it is
/// edited, and what to tell the user about it.
pub struct FieldSpec {
    pub section: Section,
    pub label: &'static str,
    pub path: &'static [&'static str],
    pub kind: FieldKind,
    pub help: &'static str,
}

/// Fields of one ducking rule.
pub static DUCKING_RULE_FIELDS: [SubFieldSpec; 4] = [
    SubFieldSpec {
        key: "primary_voice",
        label: "primary_voice",
        kind: FieldKind::VoiceRef,
        help: "The voice that triggers this rule: while any sound playing on this voice is \
               active, the ducked voices are lowered. Voices are named by a Play command's \
               \"voice\" parameter or an input's voice_id.",
    },
    SubFieldSpec {
        key: "ducked_voices",
        label: "ducked_voices",
        kind: FieldKind::VoiceRefList,
        help: "The voices to lower while the primary voice is active — typically background \
               music or ambience.",
    },
    SubFieldSpec {
        key: "target_volume",
        label: "target_volume",
        kind: FieldKind::Float { min: 0.0, max: 1.0 },
        help: "Volume the ducked voices fade to while the primary voice plays: 0.0 silences \
               them, 0.2 leaves them quietly audible, 1.0 does nothing.",
    },
    SubFieldSpec {
        key: "fade_duration_ms",
        label: "fade_duration_ms",
        kind: FieldKind::UInt {
            min: 0,
            max: u32::MAX as u64,
        },
        help: "How long the fade down (and back up) takes, in milliseconds. 500 is a gentle \
               dip; 0 is instant.",
    },
];

/// One-line summary of a ducking rule for the list view.
fn summarize_ducking_rule(value: &Value) -> String {
    let Some(obj) = value.as_object() else {
        return "(invalid)".to_string();
    };
    let primary = obj
        .get("primary_voice")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if primary.is_empty() {
        return "(unconfigured — set primary_voice)".to_string();
    }
    let ducked: Vec<&str> = obj
        .get("ducked_voices")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    let ducked_text = if ducked.is_empty() {
        "(no voices)".to_string()
    } else {
        ducked.join(", ")
    };
    let target = obj
        .get("target_volume")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5);
    let fade = obj
        .get("fade_duration_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(500);
    format!(
        "\"{}\" ducks {} → {:.0}% over {}ms",
        primary,
        ducked_text,
        target * 100.0,
        fade
    )
}

fn ducking_rule_skeleton() -> Value {
    serde_json::json!({
        "primary_voice": "",
        "ducked_voices": [],
        "target_volume": 0.5,
        "fade_duration_ms": 500
    })
}

fn no_item_defaults() -> Value {
    Value::Null
}

/// Ducking-rule list metadata.
pub static DUCKING_RULE_META: StructListMeta = StructListMeta {
    specs: &DUCKING_RULE_FIELDS,
    item_noun: "rule",
    summarize: summarize_ducking_rule,
    skeleton: ducking_rule_skeleton,
    item_defaults: no_item_defaults,
};

/// Fields of one input route (input channel -> output channel).
pub static INPUT_ROUTE_FIELDS: [SubFieldSpec; 2] = [
    SubFieldSpec {
        key: "source_channel",
        label: "source_channel",
        kind: FieldKind::ChannelRef,
        help: "Which channel of the input device to take audio from: a 0-indexed number \
               or an alias from audio.channel_aliases.",
    },
    SubFieldSpec {
        key: "dest_channel",
        label: "dest_channel",
        kind: FieldKind::ChannelRef,
        help: "Which output channel to mix that audio into: a 0-indexed number or an \
               alias from audio.channel_aliases.",
    },
];

/// One-line summary of an input route for the list view.
fn summarize_route(value: &Value) -> String {
    let Some(obj) = value.as_object() else {
        return "(invalid)".to_string();
    };
    let chan = |v: Option<&Value>| match v {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => "?".to_string(),
    };
    format!(
        "in {} → out {}",
        chan(obj.get("source_channel")),
        chan(obj.get("dest_channel"))
    )
}

fn route_skeleton() -> Value {
    serde_json::json!({ "source_channel": 0, "dest_channel": 0 })
}

/// Input-route list metadata.
pub static INPUT_ROUTE_META: StructListMeta = StructListMeta {
    specs: &INPUT_ROUTE_FIELDS,
    item_noun: "route",
    summarize: summarize_route,
    skeleton: route_skeleton,
    item_defaults: no_item_defaults,
};

/// Fields of one microphone input.
pub static INPUT_FIELDS: [SubFieldSpec; 5] = [
    SubFieldSpec {
        key: "device",
        label: "device",
        kind: FieldKind::InputDevice,
        help: "Input device to capture from. Unset uses the system default input. Enter \
               opens the device picker with a live level meter.",
    },
    SubFieldSpec {
        key: "volume",
        label: "volume",
        kind: FieldKind::Float { min: 0.0, max: 1.0 },
        help: "Input volume (0.0 - 1.0). Default 1.0.",
    },
    SubFieldSpec {
        key: "voice_id",
        label: "voice_id",
        kind: FieldKind::VoiceRef,
        help: "The voice name this input plays under, so ducking rules can react to it \
               (e.g. a rule with primary_voice \"mic\" ducks music while you talk). \
               Default \"mic\".",
    },
    SubFieldSpec {
        key: "routes",
        label: "routes",
        kind: FieldKind::StructList(&INPUT_ROUTE_META),
        help: "Which input channels feed which output channels. At least one route is \
               required for the input to be audible.",
    },
    SubFieldSpec {
        key: "latency_ms",
        label: "latency_ms",
        kind: FieldKind::UInt { min: 5, max: 500 },
        help: "Buffer latency in milliseconds (5 - 500). Lower is more immediate but \
               risks dropouts. Default 20.",
    },
];

/// One-line summary of a live input for the list view.
fn summarize_input(value: &Value) -> String {
    let Some(obj) = value.as_object() else {
        return "(invalid)".to_string();
    };
    let device = obj
        .get("device")
        .and_then(|v| v.as_str())
        .map(|d| format!("\"{}\"", d))
        .unwrap_or_else(|| "(default device)".to_string());
    let voice = obj
        .get("voice_id")
        .and_then(|v| v.as_str())
        .unwrap_or("mic");
    let routes = obj
        .get("routes")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let routes_text = match routes {
        0 => "no routes!".to_string(),
        1 => "1 route".to_string(),
        n => format!("{} routes", n),
    };
    format!("{} → voice \"{}\", {}", device, voice, routes_text)
}

fn input_skeleton() -> Value {
    Value::Object(serde_json::Map::new())
}

fn input_item_defaults() -> Value {
    serde_json::to_value(crate::config::InputConfig::default()).unwrap_or(Value::Null)
}

/// Live-input list metadata.
pub static INPUT_META: StructListMeta = StructListMeta {
    specs: &INPUT_FIELDS,
    item_noun: "input",
    summarize: summarize_input,
    skeleton: input_skeleton,
    item_defaults: input_item_defaults,
};

/// Command parameters offered when adding a field to a macro. Keys and kinds
/// mirror the Play command's parameters (src/mqtt/commands.rs, PlayMessage);
/// macros may also carry parameters of any other command, added as raw JSON.
pub static MACRO_PARAM_FIELDS: [SubFieldSpec; 14] = [
    SubFieldSpec {
        key: "volume",
        label: "volume",
        kind: FieldKind::Float { min: 0.0, max: 1.0 },
        help: "Playback volume (0.0 - 1.0).",
    },
    SubFieldSpec {
        key: "voice",
        label: "voice",
        kind: FieldKind::VoiceRef,
        help: "Voice name the sound plays under, for grouped control and ducking.",
    },
    SubFieldSpec {
        key: "fade_in",
        label: "fade_in",
        kind: FieldKind::UInt {
            min: 0,
            max: u32::MAX as u64,
        },
        help: "Fade-in duration in milliseconds.",
    },
    SubFieldSpec {
        key: "loop",
        label: "loop",
        kind: FieldKind::Bool,
        help: "Loop playback continuously.",
    },
    SubFieldSpec {
        key: "crossfade_ms",
        label: "crossfade_ms",
        kind: FieldKind::UInt {
            min: 0,
            max: u32::MAX as u64,
        },
        help: "Crossfade duration at loop boundaries in milliseconds (0 = none).",
    },
    SubFieldSpec {
        key: "start_position_ms",
        label: "start_position_ms",
        kind: FieldKind::UInt {
            min: 0,
            max: u64::MAX,
        },
        help: "Start playback at this offset in milliseconds.",
    },
    SubFieldSpec {
        key: "channel_map",
        label: "channel_map",
        kind: FieldKind::MapToJson,
        help: "Channel routing as JSON, e.g. [{\"src\": 0, \"dest\": 2, \"gain\": 0.5}].",
    },
    SubFieldSpec {
        key: "mode",
        label: "mode",
        kind: FieldKind::Enum(&["auto", "full", "stream"]),
        help: "Load strategy override: auto, full (in-memory), or stream.",
    },
    SubFieldSpec {
        key: "freshness",
        label: "freshness",
        kind: FieldKind::Enum(&["trusting", "dev", "pinned"]),
        help: "Cache freshness override: trusting, dev, or pinned.",
    },
    SubFieldSpec {
        key: "window_ms",
        label: "window_ms",
        kind: FieldKind::UInt {
            min: 100,
            max: 60000,
        },
        help: "Streamed-source ring depth override in milliseconds.",
    },
    SubFieldSpec {
        key: "prebuffer_ms",
        label: "prebuffer_ms",
        kind: FieldKind::UInt { min: 0, max: 60000 },
        help: "Streamed-source prebuffer override in milliseconds.",
    },
    SubFieldSpec {
        key: "cacheable",
        label: "cacheable",
        kind: FieldKind::Bool,
        help: "Whether streamed HTTP plays persist to the disk cache.",
    },
    SubFieldSpec {
        key: "file",
        label: "file",
        kind: FieldKind::Text,
        help: "Audio file path or URL to play.",
    },
    SubFieldSpec {
        key: "id",
        label: "id",
        kind: FieldKind::Text,
        help: "Sample identifier for targeting later commands at this sound.",
    },
];

/// The macro-parameter spec for `key`, when it is a known command parameter.
pub fn macro_param_spec(key: &str) -> Option<&'static SubFieldSpec> {
    MACRO_PARAM_FIELDS.iter().find(|s| s.key == key)
}

/// Every config field the editor can set, in display order. The coverage test
/// in tests/config_editor_test.rs asserts this stays complete as the config
/// schema grows.
pub static REGISTRY: [FieldSpec; 54] = [
    // --- MQTT ---
    FieldSpec {
        section: Section::Mqtt,
        label: "server",
        path: &["mqtt", "server"],
        kind: FieldKind::Text,
        help: "MQTT broker hostname or IP address.",
    },
    FieldSpec {
        section: Section::Mqtt,
        label: "port",
        path: &["mqtt", "port"],
        kind: FieldKind::UInt { min: 1, max: 65535 },
        help: "MQTT broker TCP port (plain default 1883, TLS commonly 8883).",
    },
    FieldSpec {
        section: Section::Mqtt,
        label: "topic",
        path: &["mqtt", "topic"],
        kind: FieldKind::OptionalText,
        help: "Topic to subscribe to for commands (wildcards # and + work). Required unless the HTTP server is enabled.",
    },
    FieldSpec {
        section: Section::Mqtt,
        label: "client_id",
        path: &["mqtt", "client_id"],
        kind: FieldKind::OptionalText,
        help: "MQTT client id. Unset auto-generates mqttaudio_<pid>.",
    },
    FieldSpec {
        section: Section::Mqtt,
        label: "reconnect_delay_seconds",
        path: &["mqtt", "reconnect_delay_seconds"],
        kind: FieldKind::UInt {
            min: 1,
            max: 86400,
        },
        help: "Seconds to wait before reconnecting after losing the broker.",
    },
    FieldSpec {
        section: Section::Mqtt,
        label: "username",
        path: &["mqtt", "username"],
        kind: FieldKind::OptionalText,
        help: "Broker username for authentication. Unset = anonymous.",
    },
    FieldSpec {
        section: Section::Mqtt,
        label: "password",
        path: &["mqtt", "password"],
        kind: FieldKind::Secret,
        help: "Broker password. Sent in cleartext unless TLS is enabled.",
    },
    FieldSpec {
        section: Section::Mqtt,
        label: "tls",
        path: &["mqtt", "tls"],
        kind: FieldKind::Flag,
        help: "Enable TLS to the broker. Off = plain TCP (even on port 8883).",
    },
    FieldSpec {
        section: Section::Mqtt,
        label: "tls.ca_path",
        path: &["mqtt", "tls", "ca_path"],
        kind: FieldKind::OptionalText,
        help: "PEM CA certificate to trust (self-signed/private brokers). Unset uses the system root store. Setting this enables TLS.",
    },
    // --- Audio ---
    FieldSpec {
        section: Section::Audio,
        label: "device",
        path: &["audio", "device"],
        kind: FieldKind::OutputDevice,
        help: "Audio output device. Unset uses the system default. Enter opens the device picker with live testing.",
    },
    FieldSpec {
        section: Section::Audio,
        label: "sample_rate",
        path: &["audio", "sample_rate"],
        kind: FieldKind::UInt {
            min: 8000,
            max: 192000,
        },
        help: "Output sample rate in Hz (8000 - 192000). Default 48000.",
    },
    FieldSpec {
        section: Section::Audio,
        label: "channels",
        path: &["audio", "channels"],
        kind: FieldKind::OptionalUInt { min: 1, max: 65535 },
        help: "Number of output channels. Unset auto-detects from the device.",
    },
    FieldSpec {
        section: Section::Audio,
        label: "buffer_size",
        path: &["audio", "buffer_size"],
        kind: FieldKind::UInt { min: 64, max: 8192 },
        help: "Buffer size in frames (64 - 8192). Smaller = lower latency, higher CPU. Default 512.",
    },
    FieldSpec {
        section: Section::Audio,
        label: "channel_aliases",
        path: &["audio", "channel_aliases"],
        kind: FieldKind::MapToUInt,
        help: "Friendly names for output channel numbers (e.g. front_left = 0, lfe = 3). Once defined, every channel field — bass management, input routes, channel volumes — can use the name instead of the number, and the channel pickers offer them.",
    },
    FieldSpec {
        section: Section::Audio,
        label: "channel_volumes",
        path: &["audio", "channel_volumes"],
        kind: FieldKind::MapToFloat { min: 0.0, max: 1.0 },
        help: "Per-channel calibration gain (key: channel number or alias, value 0.0 - 1.0). Unlisted channels stay at 1.0.",
    },
    FieldSpec {
        section: Section::Audio,
        label: "output_ceiling_db",
        path: &["audio", "output_ceiling_db"],
        kind: FieldKind::Float {
            min: -60.0,
            max: 0.0,
        },
        help: "Limiter ceiling in dBFS; the output peak is held at or below it. Default -1.0.",
    },
    FieldSpec {
        section: Section::Audio,
        label: "master_gain",
        path: &["audio", "master_gain"],
        kind: FieldKind::Float { min: 0.0, max: 8.0 },
        help: "Linear gain applied to the summed bus before limiting. Default 1.0 (unity).",
    },
    // --- Cache ---
    FieldSpec {
        section: Section::Cache,
        label: "enabled",
        path: &["cache", "enabled"],
        kind: FieldKind::Bool,
        help: "Cache decoded audio on disk for fast replays. Default on.",
    },
    FieldSpec {
        section: Section::Cache,
        label: "directory",
        path: &["cache", "directory"],
        kind: FieldKind::Text,
        help: "Cache directory (~ expands to home). Default ~/.mqttaudio/cache.",
    },
    FieldSpec {
        section: Section::Cache,
        label: "revalidate_after_seconds",
        path: &["cache", "revalidate_after_seconds"],
        kind: FieldKind::UInt {
            min: 0,
            max: u32::MAX as u64,
        },
        help: "Revalidation window for remote (HTTP) cache entries in seconds. 0 = always re-check. Default 300.",
    },
    FieldSpec {
        section: Section::Cache,
        label: "precache",
        path: &["cache", "precache"],
        kind: FieldKind::StringList,
        help: "Files, directories (non-recursive), or HTTP URLs to cache at startup.",
    },
    FieldSpec {
        section: Section::Cache,
        label: "precache_blocking",
        path: &["cache", "precache_blocking"],
        kind: FieldKind::Bool,
        help: "Block startup until precache finishes (on, default) or load in the background (off).",
    },
    FieldSpec {
        section: Section::Cache,
        label: "max_memory_mb",
        path: &["cache", "max_memory_mb"],
        kind: FieldKind::UInt {
            min: 0,
            max: u32::MAX as u64,
        },
        help: "Memory cache cap in MiB. 0 (default) auto-detects a bounded cap. Overridden by memory_budget.",
    },
    FieldSpec {
        section: Section::Cache,
        label: "memory_budget",
        path: &["cache", "memory_budget"],
        kind: FieldKind::MemoryBudget,
        help: "Advanced memory cap: auto (fraction of RAM, clamped), explicit (fixed MiB), or unlimited. Overrides max_memory_mb.",
    },
    FieldSpec {
        section: Section::Cache,
        label: "load_mode",
        path: &["cache", "load_mode"],
        kind: FieldKind::Enum(&["auto", "full", "stream"]),
        help: "Default load strategy: auto (size/duration decides), full (in-memory, all features), stream (bounded memory, forward-only).",
    },
    FieldSpec {
        section: Section::Cache,
        label: "full_load_max_bytes",
        path: &["cache", "full_load_max_bytes"],
        kind: FieldKind::UInt {
            min: 0,
            max: u64::MAX,
        },
        help: "Auto threshold: assets with a larger estimated decoded size are streamed. Default 33554432 (32 MiB).",
    },
    FieldSpec {
        section: Section::Cache,
        label: "full_load_max_seconds",
        path: &["cache", "full_load_max_seconds"],
        kind: FieldKind::UInt {
            min: 0,
            max: u32::MAX as u64,
        },
        help: "Auto threshold: assets longer than this many seconds are streamed. Default 60.",
    },
    FieldSpec {
        section: Section::Cache,
        label: "stream_window_ms",
        path: &["cache", "stream_window_ms"],
        kind: FieldKind::UInt {
            min: 100,
            max: 60000,
        },
        help: "Streamed-source ring depth in ms; bounds per-stream memory (100 - 60000). Default 1500.",
    },
    FieldSpec {
        section: Section::Cache,
        label: "stream_prebuffer_ms",
        path: &["cache", "stream_prebuffer_ms"],
        kind: FieldKind::UInt { min: 0, max: 60000 },
        help: "Audio prebuffered before a streamed source starts (ms). Must not exceed stream_window_ms. Default 150.",
    },
    FieldSpec {
        section: Section::Cache,
        label: "stream_prebuffer_deadline_ms",
        path: &["cache", "stream_prebuffer_deadline_ms"],
        kind: FieldKind::UInt {
            min: 0,
            max: 600000,
        },
        help: "Max wait for the prebuffer before starting anyway (ms). Must be >= stream_prebuffer_ms. Default 300.",
    },
    FieldSpec {
        section: Section::Cache,
        label: "freshness",
        path: &["cache", "freshness"],
        kind: FieldKind::Enum(&["trusting", "dev", "pinned"]),
        help: "Cache freshness: trusting (serve cache, refresh in background), dev (re-check every load), pinned (never auto-check).",
    },
    // --- Security ---
    FieldSpec {
        section: Section::Security,
        label: "allowed_directories",
        path: &["security", "allowed_directories"],
        kind: FieldKind::StringList,
        help: "Whitelist of directories for local file playback (~ expands). Empty = any path allowed.",
    },
    // --- Logging ---
    FieldSpec {
        section: Section::Logging,
        label: "level",
        path: &["logging", "level"],
        kind: FieldKind::Enum(&["error", "warn", "info", "debug", "trace"]),
        help: "Log level. Default info.",
    },
    FieldSpec {
        section: Section::Logging,
        label: "verbose",
        path: &["logging", "verbose"],
        kind: FieldKind::Bool,
        help: "Verbose mode; equivalent to level = debug.",
    },
    FieldSpec {
        section: Section::Logging,
        label: "format",
        path: &["logging", "format"],
        kind: FieldKind::Enum(&["text", "json"]),
        help: "Console log format: text (human-readable) or json (line-delimited, for log aggregation).",
    },
    FieldSpec {
        section: Section::Logging,
        label: "mqtt_topic",
        path: &["logging", "mqtt_topic"],
        kind: FieldKind::OptionalText,
        help: "MQTT topic to publish log records to as JSON. Unset = off.",
    },
    // --- HTTP ---
    FieldSpec {
        section: Section::Http,
        label: "enabled",
        path: &["http", "enabled"],
        kind: FieldKind::Bool,
        help: "Enable the HTTP REST/WebSocket server. Default off.",
    },
    FieldSpec {
        section: Section::Http,
        label: "port",
        path: &["http", "port"],
        kind: FieldKind::UInt { min: 0, max: 65535 },
        help: "Port to listen on. 0 = auto-select an available port.",
    },
    FieldSpec {
        section: Section::Http,
        label: "bind_address",
        path: &["http", "bind_address"],
        kind: FieldKind::Text,
        help: "Bind address. Default 127.0.0.1 (loopback only) for security.",
    },
    FieldSpec {
        section: Section::Http,
        label: "auth_token",
        path: &["http", "auth_token"],
        kind: FieldKind::Secret,
        help: "Bearer token protecting command routes (min 8 characters). Unset = open.",
    },
    FieldSpec {
        section: Section::Http,
        label: "websocket_enabled",
        path: &["http", "websocket_enabled"],
        kind: FieldKind::Bool,
        help: "Enable the /ws WebSocket endpoint for live log streaming. Default on.",
    },
    FieldSpec {
        section: Section::Http,
        label: "cors_permissive",
        path: &["http", "cors_permissive"],
        kind: FieldKind::Bool,
        help: "Allow CORS from any origin (for web admin panels). Default off.",
    },
    FieldSpec {
        section: Section::Http,
        label: "require_auth",
        path: &["http", "require_auth"],
        kind: FieldKind::Bool,
        help: "Require the token on ALL routes including status/health/ws. Default off.",
    },
    // --- Ducking ---
    FieldSpec {
        section: Section::Ducking,
        label: "ducking_rules",
        path: &["ducking_rules"],
        kind: FieldKind::StructList(&DUCKING_RULE_META),
        help: "Automatic volume reduction: while a primary voice has sound playing, the listed voices fade to a target volume, then fade back when it stops. Example: duck \"music\" to 20% while \"announcements\" plays.",
    },
    // --- Bass management ---
    FieldSpec {
        section: Section::BassManagement,
        label: "enabled",
        path: &["bass_management", "enabled"],
        kind: FieldKind::Bool,
        help: "Extract bass from source channels into a subwoofer (LFE) channel. Default off.",
    },
    FieldSpec {
        section: Section::BassManagement,
        label: "lfe_channel",
        path: &["bass_management", "lfe_channel"],
        kind: FieldKind::ChannelRef,
        help: "Output channel for the subwoofer (number or alias). Default 3 (standard 5.1 LFE position).",
    },
    FieldSpec {
        section: Section::BassManagement,
        label: "crossover_frequency_hz",
        path: &["bass_management", "crossover_frequency_hz"],
        kind: FieldKind::Float {
            min: 10.0,
            max: 200.0,
        },
        help: "Crossover frequency in Hz (10 - 200). Default 80.",
    },
    FieldSpec {
        section: Section::BassManagement,
        label: "source_channels",
        path: &["bass_management", "source_channels"],
        kind: FieldKind::ChannelRefList,
        help: "Channels to extract bass from (numbers or aliases). Must not include the LFE channel.",
    },
    FieldSpec {
        section: Section::BassManagement,
        label: "remove_bass_from_sources",
        path: &["bass_management", "remove_bass_from_sources"],
        kind: FieldKind::Bool,
        help: "High-pass the source channels after extraction (on, default) or leave them full-range (additive LFE).",
    },
    FieldSpec {
        section: Section::BassManagement,
        label: "lfe_gain",
        path: &["bass_management", "lfe_gain"],
        kind: FieldKind::Float { min: 0.0, max: 8.0 },
        help: "Linear trim applied to the summed LFE output. Default 1.0.",
    },
    // --- Inputs ---
    FieldSpec {
        section: Section::Inputs,
        label: "inputs",
        path: &["inputs"],
        kind: FieldKind::StructList(&INPUT_META),
        help: "Live inputs (microphone, line-in) mixed into the output in real time. Each input picks a capture device, routes its channels to output channels, and plays under a voice name that ducking rules can react to.",
    },
    // --- Advanced ---
    FieldSpec {
        section: Section::Advanced,
        label: "resampler_quality",
        path: &["advanced", "resampler_quality"],
        kind: FieldKind::Enum(&["fast", "medium", "high", "maximum"]),
        help: "Sample-rate conversion quality. fast (default, ~60ms/min stereo) to maximum (~230ms/min; precache-everything setups only).",
    },
    FieldSpec {
        section: Section::Advanced,
        label: "schema_version",
        path: &["schema_version"],
        kind: FieldKind::OptionalUInt {
            min: 0,
            max: u32::MAX as u64,
        },
        help: "Config schema version marker. Unset = current. Rarely needs setting by hand.",
    },
    // --- Macros ---
    FieldSpec {
        section: Section::Macros,
        label: "macros",
        path: &["macros"],
        kind: FieldKind::MapToJson,
        help: "Named bundles of command parameters. A command that includes \"macro\": \"quiet\" is merged with the bundle named \"quiet\"; the command's own parameters win on conflict. Enter opens the list of macros.",
    },
];

/// The registry entries belonging to `section`, in declaration order.
pub fn section_fields(section: Section) -> Vec<&'static FieldSpec> {
    REGISTRY.iter().filter(|f| f.section == section).collect()
}

/// `Config::default()` serialized to JSON, used to display compiled-in defaults
/// for unset fields.
pub fn default_config_value() -> Value {
    serde_json::to_value(Config::default()).unwrap_or(Value::Null)
}

/// Parse user-typed text into a JSON value for a scalar field kind. Returns a
/// human-readable error when the text does not fit the kind or its bounds.
/// Empty input on optional kinds means "unset" and returns Ok(None).
pub fn parse_field_value(kind: &FieldKind, input: &str) -> Result<Option<Value>, String> {
    let trimmed = input.trim();
    match kind {
        FieldKind::Text | FieldKind::VoiceRef => {
            if trimmed.is_empty() {
                Err("a value is required".to_string())
            } else {
                Ok(Some(Value::String(trimmed.to_string())))
            }
        }
        FieldKind::OptionalText
        | FieldKind::Secret
        | FieldKind::OutputDevice
        | FieldKind::InputDevice => {
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(Value::String(trimmed.to_string())))
            }
        }
        FieldKind::UInt { min, max } => {
            let n: u64 = trimmed
                .parse()
                .map_err(|_| format!("'{}' is not a whole number", trimmed))?;
            if n < *min || n > *max {
                return Err(format!("must be between {} and {}", min, max));
            }
            Ok(Some(Value::Number(n.into())))
        }
        FieldKind::OptionalUInt { min, max } => {
            if trimmed.is_empty() {
                return Ok(None);
            }
            let n: u64 = trimmed
                .parse()
                .map_err(|_| format!("'{}' is not a whole number", trimmed))?;
            if n < *min || n > *max {
                return Err(format!("must be between {} and {}", min, max));
            }
            Ok(Some(Value::Number(n.into())))
        }
        FieldKind::Float { min, max } => {
            let f: f64 = trimmed
                .parse()
                .map_err(|_| format!("'{}' is not a number", trimmed))?;
            if !f.is_finite() {
                return Err("must be a finite number".to_string());
            }
            if f < *min || f > *max {
                return Err(format!("must be between {} and {}", min, max));
            }
            Ok(Some(
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .ok_or_else(|| "not a representable number".to_string())?,
            ))
        }
        FieldKind::Enum(options) => {
            let lower = trimmed.to_lowercase();
            if options.contains(&lower.as_str()) {
                Ok(Some(Value::String(lower)))
            } else {
                Err(format!("must be one of: {}", options.join(", ")))
            }
        }
        FieldKind::ChannelRef => {
            if trimmed.is_empty() {
                return Err("a channel number or alias is required".to_string());
            }
            match trimmed.parse::<u64>() {
                Ok(idx) => Ok(Some(Value::Number(idx.into()))),
                Err(_) => Ok(Some(Value::String(trimmed.to_string()))),
            }
        }
        FieldKind::MapToJson => {
            let v: Value =
                serde_json::from_str(trimmed).map_err(|e| format!("invalid JSON: {}", e))?;
            Ok(Some(v))
        }
        FieldKind::Bool => match trimmed.to_lowercase().as_str() {
            "true" => Ok(Some(Value::Bool(true))),
            "false" => Ok(Some(Value::Bool(false))),
            _ => Err("must be true or false".to_string()),
        },
        FieldKind::Flag
        | FieldKind::StringList
        | FieldKind::MapToUInt
        | FieldKind::MapToFloat { .. }
        | FieldKind::ChannelRefList
        | FieldKind::VoiceRefList
        | FieldKind::MemoryBudget
        | FieldKind::StructList(_) => Err("not a text-editable field".to_string()),
    }
}

/// Render a field's value for display. `value` is the explicitly-set value if
/// any; the caller dims and annotates defaults itself.
pub fn display_value(kind: &FieldKind, value: &Value) -> String {
    match kind {
        FieldKind::Secret => {
            if value.is_null() {
                "(unset)".to_string()
            } else {
                "********".to_string()
            }
        }
        FieldKind::Flag => {
            if value.is_null() {
                "off".to_string()
            } else {
                "on".to_string()
            }
        }
        FieldKind::StringList | FieldKind::ChannelRefList | FieldKind::VoiceRefList => {
            match value {
                Value::Array(arr) => format!(
                    "[{} item{}]",
                    arr.len(),
                    if arr.len() == 1 { "" } else { "s" }
                ),
                _ => "[0 items]".to_string(),
            }
        }
        FieldKind::MapToUInt | FieldKind::MapToFloat { .. } | FieldKind::MapToJson => match value {
            Value::Object(map) => format!(
                "{{{} entr{}}}",
                map.len(),
                if map.len() == 1 { "y" } else { "ies" }
            ),
            _ => "{0 entries}".to_string(),
        },
        FieldKind::StructList(_) => match value {
            Value::Array(arr) => format!(
                "[{} item{}]",
                arr.len(),
                if arr.len() == 1 { "" } else { "s" }
            ),
            _ => "[0 items]".to_string(),
        },
        FieldKind::MemoryBudget => match value {
            Value::Object(map) => match map.get("mode").and_then(|m| m.as_str()) {
                Some("auto") => {
                    let frac = map.get("fraction").and_then(|v| v.as_f64()).unwrap_or(0.4);
                    let floor = map.get("floor_mb").and_then(|v| v.as_u64()).unwrap_or(128);
                    let ceiling = map
                        .get("ceiling_mb")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(1024);
                    format!(
                        "auto ({:.0}% of RAM, {} - {} MiB)",
                        frac * 100.0,
                        floor,
                        ceiling
                    )
                }
                Some("explicit") => {
                    let mb = map.get("mb").and_then(|v| v.as_u64()).unwrap_or(0);
                    format!("explicit ({} MiB)", mb)
                }
                Some("unlimited") => "unlimited".to_string(),
                _ => "(invalid)".to_string(),
            },
            _ => "(not set)".to_string(),
        },
        _ => match value {
            Value::Null => "(unset)".to_string(),
            Value::String(s) => s.clone(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            other => other.to_string(),
        },
    }
}
