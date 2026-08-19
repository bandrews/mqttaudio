// ABOUTME: Configuration loading and management.
// ABOUTME: Parses JSON config files and merges with CLI arguments.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;
use crate::audio::ducking::DuckingRule;

/// Highest gain any volume control accepts.
///
/// Unity is 1.0; 4.0 is +12 dB, enough to lift a quiet microphone or an
/// underpowered subwoofer without letting a mistyped value destroy a speaker.
/// The mixer saturates its output regardless, so boosted material clips rather
/// than wrapping.
pub const MAX_GAIN: f32 = 4.0;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct MqttConfig {
    pub server: String,
    pub port: u16,
    pub topic: Option<String>,
    pub client_id: Option<String>,
    pub reconnect_delay_seconds: u64,
    /// MQTT broker username for authentication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// MQTT broker password for authentication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            server: "localhost".to_string(),
            port: 1883,
            topic: None,
            client_id: None,
            reconnect_delay_seconds: 10,
            username: None,
            password: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AudioConfig {
    pub device: Option<String>,
    pub sample_rate: u32,
    pub channels: Option<usize>,
    pub buffer_size: u32,
    #[serde(default)]
    pub channel_names: HashMap<String, String>,
    #[serde(default)]
    pub channel_volumes: HashMap<String, f32>,
    /// Maps alias names to channel numbers (e.g., "front_left" -> 0)
    #[serde(default)]
    pub channel_aliases: HashMap<String, usize>,
}

/// A channel reference that can be either a numeric index or a string alias.
/// Used in config fields that reference channels.
#[derive(Debug, Clone, PartialEq)]
pub enum ChannelRef {
    Index(usize),
    Alias(String),
}

impl ChannelRef {
    /// Resolve this channel reference to a numeric index using the alias map.
    /// Returns an error if the alias is not found.
    pub fn resolve(&self, aliases: &HashMap<String, usize>) -> Result<usize, String> {
        match self {
            ChannelRef::Index(idx) => Ok(*idx),
            ChannelRef::Alias(name) => {
                aliases.get(name)
                    .copied()
                    .ok_or_else(|| format!(
                        "Unknown channel '{}': define it in audio.channel_aliases as \"{}\": <channel number>",
                        name, name
                    ))
            }
        }
    }
}

impl<'de> Deserialize<'de> for ChannelRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, Visitor};

        struct ChannelRefVisitor;

        impl<'de> Visitor<'de> for ChannelRefVisitor {
            type Value = ChannelRef;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a channel number or alias string")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(ChannelRef::Index(value as usize))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value < 0 {
                    Err(de::Error::custom("channel index cannot be negative"))
                } else {
                    Ok(ChannelRef::Index(value as usize))
                }
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                // First try to parse as a number for backwards compatibility
                if let Ok(idx) = value.parse::<usize>() {
                    Ok(ChannelRef::Index(idx))
                } else {
                    Ok(ChannelRef::Alias(value.to_string()))
                }
            }
        }

        deserializer.deserialize_any(ChannelRefVisitor)
    }
}

impl Serialize for ChannelRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            ChannelRef::Index(idx) => serializer.serialize_u64(*idx as u64),
            ChannelRef::Alias(name) => serializer.serialize_str(name),
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            device: None,
            sample_rate: 48000,
            channels: None,
            buffer_size: 512,
            channel_names: HashMap::new(),
            channel_volumes: HashMap::new(),
            channel_aliases: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CacheConfig {
    pub enabled: bool,
    pub directory: String,
    pub revalidate_after_seconds: u64,
    /// List of files to precache on startup
    #[serde(default)]
    pub precache: Vec<String>,
    /// Maximum memory cache size in megabytes.
    /// When exceeded, least-recently-used entries are evicted.
    /// Set to 0 for unlimited (default: 512 MB).
    pub max_memory_mb: u32,
    /// Block startup until all precache files are loaded.
    /// When true (default): App waits for all files to load before accepting commands.
    /// When false: App starts immediately, files load in background (lazy-load).
    pub precache_blocking: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: "~/.mqttaudio/cache".to_string(),
            revalidate_after_seconds: 300,
            precache: Vec::new(),
            max_memory_mb: 512,
            precache_blocking: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SecurityConfig {
    #[serde(default)]
    pub allowed_directories: Vec<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            allowed_directories: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub verbose: bool,
    /// MQTT topic to publish log messages to (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mqtt_topic: Option<String>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            verbose: false,
            mqtt_topic: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct BassManagementConfig {
    pub enabled: bool,
    pub lfe_channel: ChannelRef,
    pub crossover_frequency_hz: f32,
    #[serde(default)]
    pub source_channels: Vec<ChannelRef>,
    pub remove_bass_from_sources: bool,
}

impl Default for BassManagementConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            lfe_channel: ChannelRef::Index(3), // Standard 5.1 LFE position
            crossover_frequency_hz: 80.0,
            source_channels: Vec::new(),
            remove_bass_from_sources: false,
        }
    }
}

/// Resolved bass management configuration with numeric channel indices
#[derive(Debug, Clone)]
pub struct ResolvedBassManagement {
    pub enabled: bool,
    pub lfe_channel: usize,
    pub crossover_frequency_hz: f32,
    pub source_channels: Vec<usize>,
    pub remove_bass_from_sources: bool,
}

/// Configuration for a single input-to-output route
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InputRouteConfig {
    /// Source channel on the input device (0-indexed, or alias)
    pub source_channel: ChannelRef,
    /// Destination channel on the output device (0-indexed, or alias)
    pub dest_channel: ChannelRef,
}

/// Configuration for a single audio input (microphone)
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct InputConfig {
    /// Device name (None = default input device)
    pub device: Option<String>,
    /// Volume (0.0 - 1.0)
    pub volume: f32,
    /// Voice ID for ducking integration
    pub voice_id: String,
    /// Channel routing from input to output
    pub routes: Vec<InputRouteConfig>,
    /// Buffer latency in milliseconds
    pub latency_ms: u32,
    /// Capture channel count to open (None = smallest count the routes need)
    pub channels: Option<usize>,
    /// Capture sample rate to request (None = match the output sample rate)
    pub sample_rate: Option<u32>,
    /// Peak capture level (0.0-1.0) above which this input counts as
    /// actively speaking for ducking rules with its voice_id as
    /// primary_voice. None disables activity detection.
    pub activity_threshold: Option<f32>,
    /// How long activity persists after the level drops below the threshold,
    /// so ducking does not flutter between words
    pub activity_hold_ms: u32,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            device: None,
            volume: 1.0,
            voice_id: "mic".to_string(),
            routes: Vec::new(),
            latency_ms: 20,
            channels: None,
            sample_rate: None,
            activity_threshold: None,
            activity_hold_ms: 750,
        }
    }
}

/// Resampler quality preset
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResamplerQuality {
    /// Fastest resampling, acceptable quality for most content.
    /// sinc_len=64, oversample=64. ~60ms for 1 min stereo.
    Fast,
    /// Balanced quality and speed.
    /// sinc_len=128, oversample=128. ~95ms for 1 min stereo.
    Medium,
    /// High quality, slower processing.
    /// sinc_len=256, oversample=128. ~190ms for 1 min stereo.
    High,
    /// Maximum quality, slowest processing.
    /// sinc_len=256, oversample=256. ~230ms for 1 min stereo.
    /// Only recommended when precaching everything on startup.
    Maximum,
}

impl Default for ResamplerQuality {
    fn default() -> Self {
        ResamplerQuality::Fast
    }
}

impl ResamplerQuality {
    /// Get the sinc filter length for this quality setting
    pub fn sinc_len(&self) -> usize {
        match self {
            ResamplerQuality::Fast => 64,
            ResamplerQuality::Medium => 128,
            ResamplerQuality::High => 256,
            ResamplerQuality::Maximum => 256,
        }
    }

    /// Get the oversampling factor for this quality setting
    pub fn oversampling_factor(&self) -> usize {
        match self {
            ResamplerQuality::Fast => 64,
            ResamplerQuality::Medium => 128,
            ResamplerQuality::High => 128,
            ResamplerQuality::Maximum => 256,
        }
    }
}

/// Advanced configuration settings.
/// Most users will never need to change these.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AdvancedConfig {
    /// Quality level for sample rate conversion.
    ///
    /// When audio files have a different sample rate than the output device,
    /// they must be resampled. Higher quality settings produce better audio
    /// but take longer to process.
    ///
    /// Options:
    /// - "fast": Best for real-time playback. ~60ms to resample 1 minute of audio.
    ///   Sounds great for most content (games, sound effects, music).
    ///
    /// - "medium": Balanced option. ~95ms per minute.
    ///   Slightly better quality, still good for interactive use.
    ///
    /// - "high": ~190ms per minute.
    ///   Noticeable quality improvement for critical listening.
    ///
    /// - "maximum": ~230ms per minute.
    ///   Best quality, but significantly slower. Only recommended when:
    ///   - All audio is precached on startup
    ///   - You're running on a powerful system
    ///   - Audio quality is more important than latency
    ///
    /// Default: "fast" (optimized for real-time immersive applications)
    pub resampler_quality: ResamplerQuality,
}

impl Default for AdvancedConfig {
    fn default() -> Self {
        Self {
            resampler_quality: ResamplerQuality::Fast,
        }
    }
}

/// Configuration for the optional HTTP server
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct HttpConfig {
    /// Enable the HTTP server
    pub enabled: bool,
    /// Port to listen on (0 = auto-select available port)
    pub port: u16,
    /// Bind address (default: 127.0.0.1 for security)
    pub bind_address: String,
    /// Optional Bearer token for authentication (if set, all requests require it)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
    /// Enable WebSocket endpoint for log streaming
    pub websocket_enabled: bool,
    /// Allow CORS from any origin (useful for web admin panels)
    pub cors_permissive: bool,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 0, // Auto-select available port
            bind_address: "127.0.0.1".to_string(),
            auth_token: None,
            websocket_enabled: true,
            cors_permissive: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub mqtt: MqttConfig,
    pub audio: AudioConfig,
    pub cache: CacheConfig,
    pub security: SecurityConfig,
    pub logging: LoggingConfig,
    #[serde(default)]
    pub http: HttpConfig,
    #[serde(default)]
    pub ducking_rules: Vec<DuckingRule>,
    #[serde(default)]
    pub bass_management: BassManagementConfig,
    #[serde(default)]
    pub inputs: Vec<InputConfig>,
    #[serde(default)]
    pub advanced: AdvancedConfig,
    /// Command macros for parameter presets.
    /// Each macro name maps to an object of default parameters that will be merged
    /// into commands that reference the macro.
    #[serde(default)]
    pub macros: HashMap<String, serde_json::Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mqtt: MqttConfig::default(),
            audio: AudioConfig::default(),
            cache: CacheConfig::default(),
            security: SecurityConfig::default(),
            logging: LoggingConfig::default(),
            http: HttpConfig::default(),
            ducking_rules: Vec::new(),
            bass_management: BassManagementConfig::default(),
            inputs: Vec::new(),
            advanced: AdvancedConfig::default(),
            macros: HashMap::new(),
        }
    }
}

impl Config {
    /// Load configuration from a JSON file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path.as_ref())
            .map_err(|e| ConfigError::IoError(e.to_string()))?;

        let config: Config = serde_json::from_str(&contents)
            .map_err(|e| ConfigError::ParseError(e.to_string()))?;

        Ok(config)
    }

    /// Search for config file in default locations
    pub fn find_config_file() -> Option<PathBuf> {
        let search_paths = vec![
            PathBuf::from("./mqttaudio.json"),
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("mqttaudio")
                .join("config.json"),
            PathBuf::from("/etc/mqttaudio/config.json"),
        ];

        for path in search_paths {
            if path.exists() {
                return Some(path);
            }
        }

        None
    }

    /// Load config from default search paths or create default config
    pub fn load() -> Result<Self, ConfigError> {
        if let Some(path) = Self::find_config_file() {
            tracing::info!("Loading config from: {}", path.display());
            Self::from_file(path)
        } else {
            tracing::info!("No config file found, using defaults");
            Ok(Self::default())
        }
    }

    /// Load config from a specific path or search default paths
    pub fn load_from_path_or_default<P: AsRef<Path>>(path: Option<P>) -> Result<Self, ConfigError> {
        if let Some(p) = path {
            tracing::info!("Loading config from: {}", p.as_ref().display());
            Self::from_file(p)
        } else {
            Self::load()
        }
    }

    /// Expand tilde (~) in directory paths
    pub fn expand_tilde(path: &str) -> String {
        if path.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                return path.replacen("~", &home.to_string_lossy(), 1);
            }
        }
        path.to_string()
    }

    /// Get the expanded cache directory path
    pub fn cache_directory(&self) -> PathBuf {
        PathBuf::from(Self::expand_tilde(&self.cache.directory))
    }

    /// Get expanded allowed directories
    #[cfg(test)]
    pub fn allowed_directories(&self) -> Vec<PathBuf> {
        self.security.allowed_directories
            .iter()
            .map(|d| PathBuf::from(Self::expand_tilde(d)))
            .collect()
    }

    /// Check whether a file path may be played or precached.
    ///
    /// HTTP/HTTPS URLs are always allowed. When `security.allowed_directories`
    /// is non-empty, a local path must resolve (symlinks followed) inside one
    /// of the allowed directories, which also rejects `../` traversal. An
    /// empty list leaves local playback unrestricted.
    pub fn is_local_path_allowed(&self, path: &str) -> Result<(), String> {
        if path.starts_with("http://") || path.starts_with("https://") {
            return Ok(());
        }
        if self.security.allowed_directories.is_empty() {
            return Ok(());
        }

        let expanded = Self::expand_tilde(path);
        let canonical = fs::canonicalize(&expanded)
            .map_err(|e| format!("Cannot resolve path '{}': {}", path, e))?;

        for dir in &self.security.allowed_directories {
            let dir_expanded = Self::expand_tilde(dir);
            match fs::canonicalize(&dir_expanded) {
                Ok(allowed) => {
                    if canonical.starts_with(&allowed) {
                        return Ok(());
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "security.allowed_directories entry '{}' cannot be resolved: {}",
                        dir, e
                    );
                }
            }
        }

        Err(format!(
            "'{}' is outside security.allowed_directories; add its directory there to permit it",
            path
        ))
    }

    /// Merge CLI arguments into this config (CLI args override config file)
    #[allow(clippy::too_many_arguments)]
    pub fn merge_cli_args(
        &mut self,
        server: Option<String>,
        port: Option<u16>,
        topic: Option<String>,
        device: Option<String>,
        sample_rate: Option<u32>,
        channels: Option<usize>,
        verbose: bool,
        lfe_channel: Option<usize>,
        crossover_frequency: Option<f32>,
        log_topic: Option<String>,
        mqtt_username: Option<String>,
        mqtt_password: Option<String>,
        http_port: Option<u16>,
        max_cache_mb: Option<u32>,
    ) {
        // Override MQTT settings
        if let Some(s) = server {
            self.mqtt.server = s;
        }
        if let Some(p) = port {
            self.mqtt.port = p;
        }
        if let Some(t) = topic {
            self.mqtt.topic = Some(t);
        }
        if let Some(u) = mqtt_username {
            self.mqtt.username = Some(u);
        }
        if let Some(p) = mqtt_password {
            self.mqtt.password = Some(p);
        }

        // Override audio settings
        if let Some(d) = device {
            self.audio.device = Some(d);
        }
        if let Some(sr) = sample_rate {
            self.audio.sample_rate = sr;
        }
        if let Some(ch) = channels {
            self.audio.channels = Some(ch);
        }

        // Override logging settings
        if verbose {
            self.logging.verbose = true;
            self.logging.level = "debug".to_string();
        }
        if let Some(lt) = log_topic {
            self.logging.mqtt_topic = Some(lt);
        }

        // Override bass management settings
        if let Some(ch) = lfe_channel {
            self.bass_management.lfe_channel = ChannelRef::Index(ch);
        }
        if let Some(freq) = crossover_frequency {
            self.bass_management.crossover_frequency_hz = freq;
        }

        // Override HTTP settings
        if let Some(hp) = http_port {
            self.http.enabled = true;
            self.http.port = hp;
        }

        // Override cache settings
        if let Some(max_mb) = max_cache_mb {
            self.cache.max_memory_mb = max_mb;
        }
    }

    /// Resolve a channel reference to a numeric index using this config's aliases
    ///
    /// `audio.channel_names` labels channels for display and `audio.channel_aliases`
    /// is what routing resolves against. Naming a channel in the first and using it
    /// in a route is the easiest mistake to make with this config, so a name found
    /// only in `channel_names` is reported with the entry needed to fix it.
    pub fn resolve_channel(&self, channel: &ChannelRef) -> Result<usize, String> {
        channel.resolve(&self.audio.channel_aliases).map_err(|e| {
            let ChannelRef::Alias(name) = channel else {
                return e;
            };

            match self.audio.channel_names.iter().find(|(_, label)| *label == name) {
                Some((index, _)) => format!(
                    "Unknown channel '{}': audio.channel_names labels channel {} as '{}', \
                     but routing resolves against audio.channel_aliases - add \"{}\": {} there",
                    name, index, name, name, index
                ),
                None => e,
            }
        })
    }

    /// Resolve bass management channel references to numeric indices
    pub fn resolve_bass_management(&self) -> Result<ResolvedBassManagement, String> {
        let lfe = self.resolve_channel(&self.bass_management.lfe_channel)?;
        let sources: Result<Vec<usize>, String> = self.bass_management.source_channels
            .iter()
            .map(|ch| self.resolve_channel(ch))
            .collect();
        Ok(ResolvedBassManagement {
            enabled: self.bass_management.enabled,
            lfe_channel: lfe,
            crossover_frequency_hz: self.bass_management.crossover_frequency_hz,
            source_channels: sources?,
            remove_bass_from_sources: self.bass_management.remove_bass_from_sources,
        })
    }

    /// Resolve a channel_volumes key to a channel index.
    ///
    /// Keys may be a channel number, an alias from `audio.channel_aliases`, or
    /// a name from `audio.channel_names`, so levelling works whichever way the
    /// channels were labelled.
    fn resolve_channel_volume_key(&self, key: &str) -> Result<usize, String> {
        if let Ok(index) = key.parse::<usize>() {
            return Ok(index);
        }

        if let Some(index) = self.audio.channel_aliases.get(key) {
            return Ok(*index);
        }

        for (index, name) in &self.audio.channel_names {
            if name == key {
                return index.parse::<usize>().map_err(|_| {
                    format!("channel_names key '{}' is not a channel number", index)
                });
            }
        }

        Err(format!(
            "'{}' is not a channel number, an audio.channel_aliases entry, or an audio.channel_names value",
            key
        ))
    }

    /// Per-channel output gains, indexed by channel number.
    ///
    /// Channels with no entry in `audio.channel_volumes` are left at unity.
    /// Entries beyond the output channel count are ignored, so a config shared
    /// between a wide and a narrow device still loads.
    pub fn resolve_channel_gains(&self, output_channels: usize) -> Result<Vec<f32>, String> {
        let mut gains = vec![1.0; output_channels];

        for (key, gain) in &self.audio.channel_volumes {
            let index = self.resolve_channel_volume_key(key)?;
            if index < output_channels {
                gains[index] = *gain;
            }
        }

        Ok(gains)
    }

    /// Resolve input route channel references to numeric indices
    pub fn resolve_input_routes(&self, routes: &[InputRouteConfig]) -> Result<Vec<(usize, usize)>, String> {
        routes.iter()
            .map(|r| {
                let src = self.resolve_channel(&r.source_channel)?;
                let dest = self.resolve_channel(&r.dest_channel)?;
                Ok((src, dest))
            })
            .collect()
    }

    /// Supported audio file extensions for precaching
    const AUDIO_EXTENSIONS: &'static [&'static str] = &["wav", "mp3", "ogg", "flac"];

    /// Expand precache entries, converting directories to lists of audio files
    pub fn expand_precache_entries(&self) -> Vec<String> {
        let mut result = Vec::new();

        for entry in &self.cache.precache {
            let expanded = Self::expand_tilde(entry);
            let path = Path::new(&expanded);

            if path.is_dir() {
                // Scan directory for audio files
                match fs::read_dir(path) {
                    Ok(entries) => {
                        let mut files: Vec<String> = entries
                            .filter_map(|e| e.ok())
                            .filter(|e| {
                                if let Some(ext) = e.path().extension() {
                                    let ext_lower = ext.to_string_lossy().to_lowercase();
                                    Self::AUDIO_EXTENSIONS.contains(&ext_lower.as_str())
                                } else {
                                    false
                                }
                            })
                            .map(|e| e.path().to_string_lossy().to_string())
                            .collect();
                        files.sort(); // Sort for deterministic order
                        result.extend(files);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to read precache directory '{}': {}", expanded, e);
                    }
                }
            } else {
                // It's a file (or doesn't exist yet - let precache handle the error)
                result.push(expanded);
            }
        }

        result
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        // Either MQTT topic or HTTP must be enabled
        if self.mqtt.topic.is_none() && !self.http.enabled {
            errors.push("Either mqtt.topic or http.enabled is required".to_string());
        }

        // Sample rate must be reasonable
        if self.audio.sample_rate < 8000 || self.audio.sample_rate > 192000 {
            errors.push("audio.sample_rate must be between 8000 and 192000".to_string());
        }

        // Buffer size must be reasonable
        if self.audio.buffer_size < 64 || self.audio.buffer_size > 8192 {
            errors.push("audio.buffer_size must be between 64 and 8192".to_string());
        }

        // Channel volumes are gains: unity is 1.0, above that boosts
        for (ch, vol) in &self.audio.channel_volumes {
            if *vol < 0.0 || *vol > MAX_GAIN {
                errors.push(format!(
                    "audio.channel_volumes.{} must be between 0.0 and {}",
                    ch, MAX_GAIN
                ));
            }
            if let Err(e) = self.resolve_channel_volume_key(ch) {
                errors.push(format!("audio.channel_volumes.{}: {}", ch, e));
            }
        }

        // Logging level must be valid
        let valid_levels = vec!["error", "warn", "info", "debug", "trace"];
        if !valid_levels.contains(&self.logging.level.as_str()) {
            errors.push(format!("logging.level must be one of: {}", valid_levels.join(", ")));
        }

        // Bass management validation
        if self.bass_management.enabled {
            if self.bass_management.crossover_frequency_hz < 10.0
                || self.bass_management.crossover_frequency_hz > 200.0 {
                errors.push("bass_management.crossover_frequency_hz must be between 10 and 200".to_string());
            }
            if self.bass_management.source_channels.is_empty() {
                errors.push("bass_management.source_channels must not be empty when enabled".to_string());
            }
            // Validate channel aliases resolve, and reject duplicates: the
            // same source listed twice would run its crossover filter twice
            // per frame, corrupting the filter state
            if let Err(e) = self.resolve_channel(&self.bass_management.lfe_channel) {
                errors.push(format!("bass_management.lfe_channel: {}", e));
            }
            let mut seen_sources = std::collections::HashSet::new();
            for (i, ch) in self.bass_management.source_channels.iter().enumerate() {
                match self.resolve_channel(ch) {
                    Ok(index) => {
                        if !seen_sources.insert(index) {
                            errors.push(format!(
                                "bass_management.source_channels lists channel {} twice",
                                index
                            ));
                        }
                    }
                    Err(e) => {
                        errors.push(format!("bass_management.source_channels[{}]: {}", i, e));
                    }
                }
            }
        }

        // Input validation
        for (i, input) in self.inputs.iter().enumerate() {
            if input.volume < 0.0 || input.volume > MAX_GAIN {
                errors.push(format!(
                    "inputs[{}].volume must be between 0.0 and {}",
                    i, MAX_GAIN
                ));
            }
            if input.routes.is_empty() {
                errors.push(format!("inputs[{}].routes must not be empty", i));
            }
            if input.latency_ms < 5 || input.latency_ms > 500 {
                errors.push(format!("inputs[{}].latency_ms must be between 5 and 500", i));
            }
            if let Some(channels) = input.channels {
                if channels == 0 || channels > 64 {
                    errors.push(format!("inputs[{}].channels must be between 1 and 64", i));
                }
            }
            if let Some(rate) = input.sample_rate {
                if !(8000..=192000).contains(&rate) {
                    errors.push(format!("inputs[{}].sample_rate must be between 8000 and 192000", i));
                }
            }
            if let Some(threshold) = input.activity_threshold {
                if threshold <= 0.0 || threshold > 1.0 {
                    errors.push(format!(
                        "inputs[{}].activity_threshold must be above 0.0 and at most 1.0",
                        i
                    ));
                }
            }
            if input.activity_hold_ms > 10000 {
                errors.push(format!("inputs[{}].activity_hold_ms must be at most 10000", i));
            }
            // Validate channel aliases in routes
            for (j, route) in input.routes.iter().enumerate() {
                if let Err(e) = self.resolve_channel(&route.source_channel) {
                    errors.push(format!("inputs[{}].routes[{}].source_channel: {}", i, j, e));
                }
                if let Err(e) = self.resolve_channel(&route.dest_channel) {
                    errors.push(format!("inputs[{}].routes[{}].dest_channel: {}", i, j, e));
                }
            }
        }

        // Ducking rule validation
        for (i, rule) in self.ducking_rules.iter().enumerate() {
            if rule.target_volume < 0.0 || rule.target_volume > 1.0 {
                errors.push(format!(
                    "ducking_rules[{}].target_volume must be between 0.0 and 1.0",
                    i
                ));
            }
            if rule.fade_duration_ms > 60000 {
                errors.push(format!(
                    "ducking_rules[{}].fade_duration_ms must be at most 60000",
                    i
                ));
            }
            if rule.ducked_voices.is_empty() {
                errors.push(format!("ducking_rules[{}].ducked_voices must not be empty", i));
            }
        }

        // HTTP validation
        if self.http.enabled {
            // Validate bind address is not empty
            if self.http.bind_address.is_empty() {
                errors.push("http.bind_address must not be empty".to_string());
            }
            // Auth token should be reasonably long if set
            if let Some(ref token) = self.http.auth_token {
                if token.len() < 8 {
                    errors.push("http.auth_token should be at least 8 characters for security".to_string());
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    IoError(String),
    ParseError(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::IoError(e) => write!(f, "IO error: {}", e),
            ConfigError::ParseError(e) => write!(f, "Parse error: {}", e),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    #[test]
    fn test_default_config() {
        let config = Config::default();

        assert_eq!(config.mqtt.server, "localhost");
        assert_eq!(config.mqtt.port, 1883);
        assert_eq!(config.mqtt.topic, None);
        assert_eq!(config.mqtt.username, None);
        assert_eq!(config.mqtt.password, None);
        assert_eq!(config.audio.sample_rate, 48000);
        assert_eq!(config.audio.buffer_size, 512);
        assert_eq!(config.cache.enabled, true);
        assert_eq!(config.cache.revalidate_after_seconds, 300);
        assert_eq!(config.logging.level, "info");
    }

    #[test]
    fn test_parse_mqtt_credentials() {
        let json = r#"{
            "mqtt": {
                "topic": "audio/test",
                "username": "myuser",
                "password": "mypassword"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.mqtt.username, Some("myuser".to_string()));
        assert_eq!(config.mqtt.password, Some("mypassword".to_string()));
    }

    #[test]
    fn test_mqtt_credentials_not_serialized_when_none() {
        let config = MqttConfig::default();
        let json = serde_json::to_string(&config).unwrap();

        // username and password should not appear in JSON when None
        assert!(!json.contains("username"));
        assert!(!json.contains("password"));
    }

    #[test]
    fn test_merge_cli_args_mqtt_credentials() {
        let mut config = Config::default();
        assert!(config.mqtt.username.is_none());
        assert!(config.mqtt.password.is_none());

        config.merge_cli_args(
            None, None, None, None, None, None, false, None, None, None,
            Some("cli_user".to_string()),
            Some("cli_pass".to_string()),
            None,
            None,
        );

        assert_eq!(config.mqtt.username, Some("cli_user".to_string()));
        assert_eq!(config.mqtt.password, Some("cli_pass".to_string()));
    }

    #[test]
    fn test_cli_args_override_config_credentials() {
        let json = r#"{
            "mqtt": {
                "topic": "audio/test",
                "username": "config_user",
                "password": "config_pass"
            }
        }"#;

        let mut config: Config = serde_json::from_str(json).unwrap();

        config.merge_cli_args(
            None, None, None, None, None, None, false, None, None, None,
            Some("cli_user".to_string()),
            Some("cli_pass".to_string()),
            None,
            None,
        );

        // CLI should override config
        assert_eq!(config.mqtt.username, Some("cli_user".to_string()));
        assert_eq!(config.mqtt.password, Some("cli_pass".to_string()));
    }

    #[test]
    fn test_parse_minimal_config() {
        let json = r#"{
            "mqtt": {
                "topic": "audio/test"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.mqtt.topic, Some("audio/test".to_string()));
        assert_eq!(config.mqtt.server, "localhost");
        assert_eq!(config.mqtt.port, 1883);
    }

    #[test]
    fn test_parse_complete_config() {
        let json = r#"{
            "mqtt": {
                "server": "mqtt.example.com",
                "port": 8883,
                "topic": "audio/commands",
                "client_id": "custom-id",
                "reconnect_delay_seconds": 5
            },
            "audio": {
                "device": "USB Audio",
                "sample_rate": 96000,
                "buffer_size": 1024,
                "channel_names": {
                    "0": "front_left",
                    "1": "front_right"
                },
                "channel_volumes": {
                    "front_left": 0.9,
                    "1": 0.8
                }
            },
            "cache": {
                "enabled": false,
                "directory": "/tmp/cache",
                "revalidate_after_seconds": 60
            },
            "security": {
                "allowed_directories": [
                    "/opt/sounds",
                    "~/audio"
                ]
            },
            "logging": {
                "level": "debug",
                "verbose": true
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.mqtt.server, "mqtt.example.com");
        assert_eq!(config.mqtt.port, 8883);
        assert_eq!(config.mqtt.topic, Some("audio/commands".to_string()));
        assert_eq!(config.mqtt.client_id, Some("custom-id".to_string()));
        assert_eq!(config.mqtt.reconnect_delay_seconds, 5);

        assert_eq!(config.audio.device, Some("USB Audio".to_string()));
        assert_eq!(config.audio.sample_rate, 96000);
        assert_eq!(config.audio.buffer_size, 1024);
        assert_eq!(config.audio.channel_names.get("0"), Some(&"front_left".to_string()));
        assert_eq!(config.audio.channel_volumes.get("front_left"), Some(&0.9));

        assert_eq!(config.cache.enabled, false);
        assert_eq!(config.cache.directory, "/tmp/cache");
        assert_eq!(config.cache.revalidate_after_seconds, 60);

        assert_eq!(config.security.allowed_directories.len(), 2);
        assert_eq!(config.security.allowed_directories[0], "/opt/sounds");

        assert_eq!(config.logging.level, "debug");
        assert_eq!(config.logging.verbose, true);
    }

    #[test]
    fn test_load_from_file() {
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, r#"{{
            "mqtt": {{
                "server": "testserver",
                "topic": "test/topic"
            }}
        }}"#).unwrap();

        let config = Config::from_file(temp_file.path()).unwrap();

        assert_eq!(config.mqtt.server, "testserver");
        assert_eq!(config.mqtt.topic, Some("test/topic".to_string()));
    }

    #[test]
    fn test_load_from_invalid_file() {
        let result = Config::from_file("/nonexistent/path.json");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_from_invalid_json() {
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "{{ invalid json }}").unwrap();

        let result = Config::from_file(temp_file.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_expand_tilde() {
        let expanded = Config::expand_tilde("~/test/path");
        assert!(!expanded.starts_with("~"));
        assert!(expanded.contains("test/path"));

        let no_tilde = Config::expand_tilde("/absolute/path");
        assert_eq!(no_tilde, "/absolute/path");
    }

    #[test]
    fn test_cache_directory_expansion() {
        let mut config = Config::default();
        config.cache.directory = "~/test/cache".to_string();

        let expanded = config.cache_directory();
        assert!(!expanded.to_string_lossy().starts_with("~"));
    }

    #[test]
    fn test_allowed_directories_expansion() {
        let mut config = Config::default();
        config.security.allowed_directories = vec![
            "~/sounds".to_string(),
            "/opt/audio".to_string(),
        ];

        let expanded = config.allowed_directories();
        assert_eq!(expanded.len(), 2);
        assert!(!expanded[0].to_string_lossy().starts_with("~"));
        assert_eq!(expanded[1], PathBuf::from("/opt/audio"));
    }

    #[test]
    fn test_validate_missing_topic_and_http() {
        let config = Config::default();
        let result = config.validate();

        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("mqtt.topic") || e.contains("http.enabled")));
    }

    #[test]
    fn test_validate_http_only_mode() {
        let mut config = Config::default();
        config.http.enabled = true;
        config.http.port = 8080;

        // HTTP-only mode should be valid (no MQTT topic needed)
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_mqtt_only_mode() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test/topic".to_string());

        // MQTT-only mode should be valid
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_invalid_sample_rate() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.audio.sample_rate = 1000;

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("sample_rate")));
    }

    #[test]
    fn test_validate_invalid_buffer_size() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.audio.buffer_size = 32;

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("buffer_size")));
    }

    #[test]
    fn test_validate_invalid_channel_volume() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.audio.channel_volumes.insert("0".to_string(), -0.5);

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("channel_volumes")));
    }

    #[test]
    fn test_validate_invalid_log_level() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.logging.level = "invalid".to_string();

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("logging.level")));
    }

    #[test]
    fn test_validate_valid_config() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_merge_cli_args_server() {
        let mut config = Config::default();
        config.mqtt.server = "original".to_string();

        config.merge_cli_args(
            Some("overridden".to_string()),
            None,
            None,
            None,
            None,
            None, // channels
            false,
            None,
            None,
            None, // log_topic
            None, // mqtt_username
            None, // mqtt_password
            None, // http_port
            None, // max_cache_mb
        );

        assert_eq!(config.mqtt.server, "overridden");
    }

    #[test]
    fn test_merge_cli_args_all_fields() {
        let mut config = Config::default();

        config.merge_cli_args(
            Some("newserver".to_string()),
            Some(8883),
            Some("newtopic".to_string()),
            Some("newdevice".to_string()),
            Some(96000),
            Some(8), // channels
            true,
            Some(5),
            Some(120.0),
            Some("audio/logs".to_string()), // log_topic
            Some("testuser".to_string()),   // mqtt_username
            Some("testpass".to_string()),   // mqtt_password
            None, // http_port
            None, // max_cache_mb
        );

        assert_eq!(config.mqtt.server, "newserver");
        assert_eq!(config.mqtt.port, 8883);
        assert_eq!(config.mqtt.topic, Some("newtopic".to_string()));
        assert_eq!(config.mqtt.username, Some("testuser".to_string()));
        assert_eq!(config.mqtt.password, Some("testpass".to_string()));
        assert_eq!(config.audio.device, Some("newdevice".to_string()));
        assert_eq!(config.audio.sample_rate, 96000);
        assert_eq!(config.audio.channels, Some(8));
        assert_eq!(config.logging.verbose, true);
        assert_eq!(config.logging.level, "debug");
        assert_eq!(config.logging.mqtt_topic, Some("audio/logs".to_string()));
        assert_eq!(config.bass_management.lfe_channel, ChannelRef::Index(5));
        assert_eq!(config.bass_management.crossover_frequency_hz, 120.0);
    }

    #[test]
    fn test_merge_cli_args_partial_override() {
        let mut config = Config::default();
        config.mqtt.server = "original".to_string();
        config.mqtt.port = 1883;
        config.mqtt.topic = Some("original/topic".to_string());

        config.merge_cli_args(
            None,  // Don't override server
            Some(8883),  // Override port
            None,  // Don't override topic
            None,
            None,
            None, // channels
            false,
            None,
            None,
            None, // log_topic
            None, // mqtt_username
            None, // mqtt_password
            None, // http_port
            None, // max_cache_mb
        );

        assert_eq!(config.mqtt.server, "original");  // Unchanged
        assert_eq!(config.mqtt.port, 8883);  // Changed
        assert_eq!(config.mqtt.topic, Some("original/topic".to_string()));  // Unchanged
    }

    #[test]
    fn test_merge_cli_args_verbose_sets_debug() {
        let mut config = Config::default();
        assert_eq!(config.logging.level, "info");
        assert_eq!(config.logging.verbose, false);

        config.merge_cli_args(None, None, None, None, None, None, true, None, None, None, None, None, None, None);

        assert_eq!(config.logging.verbose, true);
        assert_eq!(config.logging.level, "debug");
    }

    #[test]
    fn test_bass_management_config_default() {
        let config = Config::default();

        assert_eq!(config.bass_management.enabled, false);
        assert_eq!(config.bass_management.lfe_channel, ChannelRef::Index(3));
        assert_eq!(config.bass_management.crossover_frequency_hz, 80.0);
        assert!(config.bass_management.source_channels.is_empty());
        assert_eq!(config.bass_management.remove_bass_from_sources, false);
    }

    #[test]
    fn test_bass_management_validation_invalid_crossover() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.bass_management.enabled = true;
        config.bass_management.source_channels = vec![ChannelRef::Index(0), ChannelRef::Index(1)];
        config.bass_management.crossover_frequency_hz = 5.0; // Too low

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("crossover_frequency_hz")));
    }

    #[test]
    fn test_bass_management_validation_empty_sources() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.bass_management.enabled = true;
        // source_channels is empty

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("source_channels")));
    }

    #[test]
    fn test_bass_management_validation_valid() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.bass_management.enabled = true;
        config.bass_management.source_channels = vec![ChannelRef::Index(0), ChannelRef::Index(1)];
        config.bass_management.crossover_frequency_hz = 80.0;

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_input_config_default() {
        let input = InputConfig::default();

        assert!(input.device.is_none());
        assert_eq!(input.volume, 1.0);
        assert_eq!(input.voice_id, "mic");
        assert!(input.routes.is_empty());
        assert_eq!(input.latency_ms, 20);
    }

    #[test]
    fn test_input_config_parse() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "inputs": [
                {
                    "device": "USB Microphone",
                    "volume": 0.8,
                    "voice_id": "gamemaster_mic",
                    "routes": [
                        {"source_channel": 0, "dest_channel": 4},
                        {"source_channel": 0, "dest_channel": 5}
                    ],
                    "latency_ms": 30
                }
            ]
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.inputs.len(), 1);
        let input = &config.inputs[0];
        assert_eq!(input.device, Some("USB Microphone".to_string()));
        assert_eq!(input.volume, 0.8);
        assert_eq!(input.voice_id, "gamemaster_mic");
        assert_eq!(input.routes.len(), 2);
        assert_eq!(input.routes[0].source_channel, ChannelRef::Index(0));
        assert_eq!(input.routes[0].dest_channel, ChannelRef::Index(4));
        assert_eq!(input.routes[1].dest_channel, ChannelRef::Index(5));
        assert_eq!(input.latency_ms, 30);
    }

    #[test]
    fn test_input_config_multiple_inputs() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "inputs": [
                {
                    "voice_id": "mic1",
                    "routes": [{"source_channel": 0, "dest_channel": 0}]
                },
                {
                    "device": "Second Mic",
                    "voice_id": "mic2",
                    "routes": [{"source_channel": 0, "dest_channel": 1}]
                }
            ]
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.inputs.len(), 2);
        assert_eq!(config.inputs[0].voice_id, "mic1");
        assert_eq!(config.inputs[1].voice_id, "mic2");
    }

    #[test]
    fn test_unknown_alias_error_names_the_alias_map() {
        let config = Config::default();
        let err = config.resolve_channel(&ChannelRef::Alias("booth".to_string())).unwrap_err();

        assert!(err.contains("booth"));
        assert!(
            err.contains("channel_aliases"),
            "the error should say which map to add the name to, got: {}",
            err
        );
    }

    #[test]
    fn test_unknown_alias_error_points_at_channel_names() {
        // channel_names is index -> name for display; routing resolves against
        // channel_aliases. Naming a channel in the wrong map is the easiest
        // mistake to make, so the error has to say so.
        let mut config = Config::default();
        config.audio.channel_names.insert("6".to_string(), "booth".to_string());

        let err = config.resolve_channel(&ChannelRef::Alias("booth".to_string())).unwrap_err();

        assert!(err.contains("channel_names"), "got: {}", err);
        assert!(err.contains("channel_aliases"), "got: {}", err);
        assert!(err.contains("6"), "the error should name the channel it found: {}", err);
    }

    #[test]
    fn test_input_route_error_points_at_channel_names() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.audio.channel_names.insert("4".to_string(), "house".to_string());
        config.inputs.push(InputConfig {
            routes: vec![InputRouteConfig {
                source_channel: ChannelRef::Index(0),
                dest_channel: ChannelRef::Alias("house".to_string()),
            }],
            ..Default::default()
        });

        let errors = config.validate().unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("channel_names") && e.contains("channel_aliases")),
            "validation should explain the two maps, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_channel_volumes_allow_boost() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.audio.channel_volumes.insert("3".to_string(), 1.5);

        assert!(config.validate().is_ok(), "boosting a channel above unity is allowed");
    }

    #[test]
    fn test_channel_volumes_reject_excessive_gain() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.audio.channel_volumes.insert("3".to_string(), MAX_GAIN + 1.0);

        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("channel_volumes")));
    }

    #[test]
    fn test_input_volume_allows_boost() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.inputs.push(InputConfig {
            volume: 2.0,
            routes: vec![InputRouteConfig { source_channel: ChannelRef::Index(0), dest_channel: ChannelRef::Index(0) }],
            ..Default::default()
        });

        assert!(config.validate().is_ok(), "a quiet microphone can be boosted");
    }

    #[test]
    fn test_input_volume_rejects_excessive_gain() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.inputs.push(InputConfig {
            volume: MAX_GAIN + 1.0,
            routes: vec![InputRouteConfig { source_channel: ChannelRef::Index(0), dest_channel: ChannelRef::Index(0) }],
            ..Default::default()
        });

        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("volume")));
    }

    #[test]
    fn test_resolve_channel_gains_by_index() {
        let mut config = Config::default();
        config.audio.channel_volumes.insert("0".to_string(), 0.5);
        config.audio.channel_volumes.insert("3".to_string(), 1.5);

        let gains = config.resolve_channel_gains(4).unwrap();
        assert_eq!(gains, vec![0.5, 1.0, 1.0, 1.5]);
    }

    #[test]
    fn test_resolve_channel_gains_by_alias() {
        let mut config = Config::default();
        config.audio.channel_aliases.insert("sub".to_string(), 3);
        config.audio.channel_volumes.insert("sub".to_string(), 1.8);

        let gains = config.resolve_channel_gains(4).unwrap();
        assert_eq!(gains[3], 1.8);
    }

    #[test]
    fn test_resolve_channel_gains_by_channel_name() {
        // channel_names is index -> name, and the example config levels
        // channels using those names
        let mut config = Config::default();
        config.audio.channel_names.insert("3".to_string(), "lfe".to_string());
        config.audio.channel_volumes.insert("lfe".to_string(), 1.2);

        let gains = config.resolve_channel_gains(4).unwrap();
        assert_eq!(gains[3], 1.2);
    }

    #[test]
    fn test_resolve_channel_gains_ignores_channels_beyond_output() {
        let mut config = Config::default();
        config.audio.channel_volumes.insert("9".to_string(), 0.5);

        let gains = config.resolve_channel_gains(2).unwrap();
        assert_eq!(gains, vec![1.0, 1.0]);
    }

    #[test]
    fn test_channel_volumes_unknown_name_is_an_error() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.audio.channel_volumes.insert("nowhere".to_string(), 0.5);

        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("nowhere")));
    }

    #[test]
    fn test_input_validation_invalid_volume() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.inputs.push(InputConfig {
            volume: -0.5, // Invalid
            routes: vec![InputRouteConfig { source_channel: ChannelRef::Index(0), dest_channel: ChannelRef::Index(0) }],
            ..Default::default()
        });

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("volume")));
    }

    #[test]
    fn test_input_validation_empty_routes() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.inputs.push(InputConfig {
            routes: vec![], // Invalid - empty
            ..Default::default()
        });

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("routes")));
    }

    #[test]
    fn test_input_validation_invalid_latency() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.inputs.push(InputConfig {
            latency_ms: 1000, // Invalid - too high
            routes: vec![InputRouteConfig { source_channel: ChannelRef::Index(0), dest_channel: ChannelRef::Index(0) }],
            ..Default::default()
        });

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("latency_ms")));
    }

    #[test]
    fn test_input_validation_valid() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.inputs.push(InputConfig {
            device: Some("Test Mic".to_string()),
            volume: 0.8,
            voice_id: "mic".to_string(),
            routes: vec![
                InputRouteConfig { source_channel: ChannelRef::Index(0), dest_channel: ChannelRef::Index(0) },
                InputRouteConfig { source_channel: ChannelRef::Index(0), dest_channel: ChannelRef::Index(1) },
            ],
            latency_ms: 25,
            channels: None,
            sample_rate: None,
            activity_threshold: None,
            activity_hold_ms: 750,
        });

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_input_activity_threshold_validation() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.inputs.push(InputConfig {
            activity_threshold: Some(1.5), // Invalid - above 1.0
            routes: vec![InputRouteConfig { source_channel: ChannelRef::Index(0), dest_channel: ChannelRef::Index(0) }],
            ..Default::default()
        });

        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("activity_threshold")));

        config.inputs[0].activity_threshold = Some(0.05);
        assert!(config.validate().is_ok(), "a sensible threshold validates");
    }

    #[test]
    fn test_bass_management_rejects_duplicate_sources() {
        // The same source channel listed twice runs its crossover filter
        // twice per frame, corrupting the filter state
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.bass_management.enabled = true;
        config.bass_management.source_channels =
            vec![ChannelRef::Index(0), ChannelRef::Index(1), ChannelRef::Index(0)];

        let errors = config.validate().unwrap_err();
        assert!(
            errors.iter().any(|e| e.contains("source_channels") && e.contains("twice")),
            "got: {:?}",
            errors
        );
    }

    #[test]
    fn test_ducking_rule_validation() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.ducking_rules.push(DuckingRule {
            primary_voice: "narration".to_string(),
            ducked_voices: vec![],
            target_volume: 1.5,
            fade_duration_ms: 120000,
        });

        let errors = config.validate().unwrap_err();
        assert!(errors.iter().any(|e| e.contains("target_volume")));
        assert!(errors.iter().any(|e| e.contains("fade_duration_ms")));
        assert!(errors.iter().any(|e| e.contains("ducked_voices")));

        config.ducking_rules[0] = DuckingRule {
            primary_voice: "narration".to_string(),
            ducked_voices: vec!["music".to_string()],
            target_volume: 0.2,
            fade_duration_ms: 1000,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_cache_precache_default() {
        let config = Config::default();
        assert!(config.cache.precache.is_empty());
    }

    #[test]
    fn test_cache_precache_parse() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "cache": {
                "precache": [
                    "/sounds/startup.wav",
                    "https://example.com/welcome.mp3"
                ]
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.cache.precache.len(), 2);
        assert_eq!(config.cache.precache[0], "/sounds/startup.wav");
        assert_eq!(config.cache.precache[1], "https://example.com/welcome.mp3");
    }

    #[test]
    fn test_local_path_allowed_when_no_directories_configured() {
        let config = Config::default();
        // An empty list leaves local playback unrestricted
        assert!(config.is_local_path_allowed("/etc/hostname").is_ok());
    }

    #[test]
    fn test_local_path_allowed_inside_configured_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sound.wav");
        std::fs::write(&file, b"x").unwrap();

        let mut config = Config::default();
        config.security.allowed_directories = vec![dir.path().to_string_lossy().to_string()];

        assert!(config.is_local_path_allowed(file.to_str().unwrap()).is_ok());
    }

    #[test]
    fn test_local_path_denied_outside_configured_directory() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("secret.wav");
        std::fs::write(&file, b"x").unwrap();

        let mut config = Config::default();
        config.security.allowed_directories = vec![allowed.path().to_string_lossy().to_string()];

        let err = config.is_local_path_allowed(file.to_str().unwrap()).unwrap_err();
        assert!(err.contains("allowed_directories"), "error should name the setting: {}", err);
    }

    #[test]
    fn test_local_path_denied_via_traversal() {
        let parent = tempfile::tempdir().unwrap();
        let allowed = parent.path().join("sounds");
        std::fs::create_dir(&allowed).unwrap();
        let secret = parent.path().join("secret.wav");
        std::fs::write(&secret, b"x").unwrap();

        let mut config = Config::default();
        config.security.allowed_directories = vec![allowed.to_string_lossy().to_string()];

        let sneaky = format!("{}/../secret.wav", allowed.to_string_lossy());
        assert!(config.is_local_path_allowed(&sneaky).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_local_path_denied_via_symlink() {
        let allowed = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("secret.wav");
        std::fs::write(&secret, b"x").unwrap();
        let link = allowed.path().join("link.wav");
        std::os::unix::fs::symlink(&secret, &link).unwrap();

        let mut config = Config::default();
        config.security.allowed_directories = vec![allowed.path().to_string_lossy().to_string()];

        assert!(config.is_local_path_allowed(link.to_str().unwrap()).is_err());
    }

    #[test]
    fn test_local_path_check_ignores_urls() {
        let mut config = Config::default();
        config.security.allowed_directories = vec!["/opt/sounds".to_string()];

        assert!(config.is_local_path_allowed("https://example.com/a.mp3").is_ok());
        assert!(config.is_local_path_allowed("http://example.com/a.mp3").is_ok());
    }

    #[test]
    fn test_cache_precache_blocking_default() {
        let config = Config::default();
        // Default is blocking (true) - wait for all files before accepting commands
        assert!(config.cache.precache_blocking);
    }

    #[test]
    fn test_cache_precache_blocking_false() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "cache": {
                "precache_blocking": false
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(!config.cache.precache_blocking);
    }

    #[test]
    fn test_cache_precache_blocking_true() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "cache": {
                "precache_blocking": true
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.cache.precache_blocking);
    }

    #[test]
    fn test_cache_max_memory_mb_default() {
        let config = Config::default();
        assert_eq!(config.cache.max_memory_mb, 512);
    }

    #[test]
    fn test_cache_max_memory_mb_parse() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "cache": {
                "max_memory_mb": 1024
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.cache.max_memory_mb, 1024);
    }

    #[test]
    fn test_cache_max_memory_mb_unlimited() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "cache": {
                "max_memory_mb": 0
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.cache.max_memory_mb, 0);
    }

    #[test]
    fn test_logging_mqtt_topic_default() {
        let config = Config::default();
        assert!(config.logging.mqtt_topic.is_none());
    }

    #[test]
    fn test_logging_mqtt_topic_parse() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "logging": {
                "level": "info",
                "mqtt_topic": "audio/logs"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.logging.mqtt_topic, Some("audio/logs".to_string()));
    }

    #[test]
    fn test_merge_cli_args_log_topic() {
        let mut config = Config::default();
        assert!(config.logging.mqtt_topic.is_none());

        config.merge_cli_args(
            None, None, None, None, None, None, false, None, None,
            Some("audio/logs".to_string()),
            None, None, None, None,
        );

        assert_eq!(config.logging.mqtt_topic, Some("audio/logs".to_string()));
    }

    #[test]
    fn test_logging_config_serialization() {
        let config = LoggingConfig {
            level: "debug".to_string(),
            verbose: true,
            mqtt_topic: Some("test/logs".to_string()),
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"mqtt_topic\":\"test/logs\""));

        // With no mqtt_topic, it should be omitted
        let config_no_topic = LoggingConfig {
            level: "info".to_string(),
            verbose: false,
            mqtt_topic: None,
        };
        let json_no_topic = serde_json::to_string(&config_no_topic).unwrap();
        assert!(!json_no_topic.contains("mqtt_topic"));
    }

    #[test]
    fn test_channel_ref_resolve_index() {
        let aliases = HashMap::new();
        let channel = ChannelRef::Index(5);
        assert_eq!(channel.resolve(&aliases), Ok(5));
    }

    #[test]
    fn test_channel_ref_resolve_alias() {
        let mut aliases = HashMap::new();
        aliases.insert("front_left".to_string(), 0);
        aliases.insert("front_right".to_string(), 1);

        let channel = ChannelRef::Alias("front_left".to_string());
        assert_eq!(channel.resolve(&aliases), Ok(0));

        let channel2 = ChannelRef::Alias("front_right".to_string());
        assert_eq!(channel2.resolve(&aliases), Ok(1));
    }

    #[test]
    fn test_channel_ref_resolve_unknown_alias() {
        let aliases = HashMap::new();
        let channel = ChannelRef::Alias("unknown".to_string());
        assert!(channel.resolve(&aliases).is_err());
    }

    #[test]
    fn test_channel_aliases_parse() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "audio": {
                "channel_aliases": {
                    "front_left": 0,
                    "front_right": 1,
                    "lfe": 3
                }
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.audio.channel_aliases.len(), 3);
        assert_eq!(config.audio.channel_aliases.get("front_left"), Some(&0));
        assert_eq!(config.audio.channel_aliases.get("front_right"), Some(&1));
        assert_eq!(config.audio.channel_aliases.get("lfe"), Some(&3));
    }

    #[test]
    fn test_channel_ref_deserialize_from_number() {
        let json = r#"{"source_channel": 5, "dest_channel": 3}"#;
        let route: InputRouteConfig = serde_json::from_str(json).unwrap();

        assert_eq!(route.source_channel, ChannelRef::Index(5));
        assert_eq!(route.dest_channel, ChannelRef::Index(3));
    }

    #[test]
    fn test_channel_ref_deserialize_from_string_alias() {
        let json = r#"{"source_channel": "mic_left", "dest_channel": "front_left"}"#;
        let route: InputRouteConfig = serde_json::from_str(json).unwrap();

        assert_eq!(route.source_channel, ChannelRef::Alias("mic_left".to_string()));
        assert_eq!(route.dest_channel, ChannelRef::Alias("front_left".to_string()));
    }

    #[test]
    fn test_channel_ref_deserialize_string_number_as_index() {
        // A string containing a number should parse as an index
        let json = r#"{"source_channel": "0", "dest_channel": "5"}"#;
        let route: InputRouteConfig = serde_json::from_str(json).unwrap();

        assert_eq!(route.source_channel, ChannelRef::Index(0));
        assert_eq!(route.dest_channel, ChannelRef::Index(5));
    }

    #[test]
    fn test_bass_management_with_aliases() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "audio": {
                "channel_aliases": {
                    "front_left": 0,
                    "front_right": 1,
                    "lfe": 3
                }
            },
            "bass_management": {
                "enabled": true,
                "lfe_channel": "lfe",
                "source_channels": ["front_left", "front_right"],
                "crossover_frequency_hz": 80.0
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        // Check that aliases are stored as ChannelRef::Alias
        assert_eq!(config.bass_management.lfe_channel, ChannelRef::Alias("lfe".to_string()));

        // Resolve and check values
        let resolved = config.resolve_bass_management().unwrap();
        assert_eq!(resolved.lfe_channel, 3);
        assert_eq!(resolved.source_channels, vec![0, 1]);

        // Validation should pass
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_input_routes_with_aliases() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "audio": {
                "channel_aliases": {
                    "front_left": 0,
                    "front_right": 1
                }
            },
            "inputs": [{
                "voice_id": "mic",
                "routes": [
                    {"source_channel": 0, "dest_channel": "front_left"},
                    {"source_channel": 0, "dest_channel": "front_right"}
                ]
            }]
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        let routes = config.resolve_input_routes(&config.inputs[0].routes).unwrap();
        assert_eq!(routes, vec![(0, 0), (0, 1)]);
    }

    #[test]
    fn test_validation_fails_on_unknown_alias() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "audio": {
                "channel_aliases": {}
            },
            "bass_management": {
                "enabled": true,
                "lfe_channel": "unknown_alias",
                "source_channels": [0, 1],
                "crossover_frequency_hz": 80.0
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        let result = config.validate();

        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("unknown_alias")));
    }

    #[test]
    fn test_validation_fails_on_unknown_route_alias() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "audio": {
                "channel_aliases": {}
            },
            "inputs": [{
                "voice_id": "mic",
                "routes": [
                    {"source_channel": 0, "dest_channel": "unknown"}
                ]
            }]
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        let result = config.validate();

        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("unknown")));
    }

    #[test]
    fn test_channel_ref_serialize() {
        let route = InputRouteConfig {
            source_channel: ChannelRef::Index(0),
            dest_channel: ChannelRef::Alias("front_left".to_string()),
        };

        let json = serde_json::to_string(&route).unwrap();
        assert!(json.contains("\"source_channel\":0"));
        assert!(json.contains("\"dest_channel\":\"front_left\""));
    }

    #[test]
    fn test_expand_precache_entries_file() {
        let mut config = Config::default();
        config.cache.precache = vec!["/some/file.wav".to_string()];

        let expanded = config.expand_precache_entries();
        assert_eq!(expanded, vec!["/some/file.wav"]);
    }

    #[test]
    fn test_expand_precache_entries_nonexistent_file() {
        let mut config = Config::default();
        config.cache.precache = vec!["/nonexistent/path/to/file.wav".to_string()];

        // Non-existent files are passed through (precache will handle the error)
        let expanded = config.expand_precache_entries();
        assert_eq!(expanded, vec!["/nonexistent/path/to/file.wav"]);
    }

    #[test]
    fn test_expand_precache_entries_directory() {
        // Create a temporary directory with some audio files
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();

        // Create some audio files
        std::fs::write(temp_path.join("sound1.wav"), b"fake wav").unwrap();
        std::fs::write(temp_path.join("sound2.mp3"), b"fake mp3").unwrap();
        std::fs::write(temp_path.join("sound3.ogg"), b"fake ogg").unwrap();
        std::fs::write(temp_path.join("sound4.flac"), b"fake flac").unwrap();
        std::fs::write(temp_path.join("readme.txt"), b"not audio").unwrap();
        std::fs::write(temp_path.join("data.json"), b"not audio").unwrap();

        let mut config = Config::default();
        config.cache.precache = vec![temp_path.to_string_lossy().to_string()];

        let expanded = config.expand_precache_entries();

        // Should only include audio files, sorted alphabetically
        assert_eq!(expanded.len(), 4);
        assert!(expanded[0].ends_with("sound1.wav"));
        assert!(expanded[1].ends_with("sound2.mp3"));
        assert!(expanded[2].ends_with("sound3.ogg"));
        assert!(expanded[3].ends_with("sound4.flac"));
    }

    #[test]
    fn test_expand_precache_entries_mixed() {
        // Create a temporary directory with some audio files
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();

        std::fs::write(temp_path.join("ambient.wav"), b"fake wav").unwrap();
        std::fs::write(temp_path.join("music.mp3"), b"fake mp3").unwrap();

        let mut config = Config::default();
        config.cache.precache = vec![
            "/specific/file.wav".to_string(),
            temp_path.to_string_lossy().to_string(),
            "http://example.com/sound.mp3".to_string(),
        ];

        let expanded = config.expand_precache_entries();

        // Should have the specific file, the expanded directory, and the URL
        assert_eq!(expanded.len(), 4);
        assert_eq!(expanded[0], "/specific/file.wav");
        assert!(expanded[1].ends_with("ambient.wav"));
        assert!(expanded[2].ends_with("music.mp3"));
        assert_eq!(expanded[3], "http://example.com/sound.mp3");
    }

    #[test]
    fn test_expand_precache_entries_empty_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();

        // Empty directory - no files
        let mut config = Config::default();
        config.cache.precache = vec![temp_path.to_string_lossy().to_string()];

        let expanded = config.expand_precache_entries();
        assert!(expanded.is_empty());
    }

    #[test]
    fn test_expand_precache_entries_case_insensitive_extensions() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();

        // Create files with various case extensions
        std::fs::write(temp_path.join("sound1.WAV"), b"fake wav").unwrap();
        std::fs::write(temp_path.join("sound2.Mp3"), b"fake mp3").unwrap();
        std::fs::write(temp_path.join("sound3.OGG"), b"fake ogg").unwrap();

        let mut config = Config::default();
        config.cache.precache = vec![temp_path.to_string_lossy().to_string()];

        let expanded = config.expand_precache_entries();

        // Should find all files regardless of extension case
        assert_eq!(expanded.len(), 3);
    }

    #[test]
    fn test_audio_extensions_constant() {
        assert!(Config::AUDIO_EXTENSIONS.contains(&"wav"));
        assert!(Config::AUDIO_EXTENSIONS.contains(&"mp3"));
        assert!(Config::AUDIO_EXTENSIONS.contains(&"ogg"));
        assert!(Config::AUDIO_EXTENSIONS.contains(&"flac"));
        assert!(!Config::AUDIO_EXTENSIONS.contains(&"txt"));
    }

    #[test]
    fn test_http_config_default() {
        let config = HttpConfig::default();

        assert_eq!(config.enabled, false);
        assert_eq!(config.port, 0);
        assert_eq!(config.bind_address, "127.0.0.1");
        assert!(config.auth_token.is_none());
        assert_eq!(config.websocket_enabled, true);
        assert_eq!(config.cors_permissive, false);
    }

    #[test]
    fn test_http_config_parse() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "http": {
                "enabled": true,
                "port": 8080,
                "bind_address": "0.0.0.0",
                "auth_token": "secrettoken123",
                "websocket_enabled": true,
                "cors_permissive": true
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.http.enabled, true);
        assert_eq!(config.http.port, 8080);
        assert_eq!(config.http.bind_address, "0.0.0.0");
        assert_eq!(config.http.auth_token, Some("secrettoken123".to_string()));
        assert_eq!(config.http.websocket_enabled, true);
        assert_eq!(config.http.cors_permissive, true);
    }

    #[test]
    fn test_http_config_auth_token_not_serialized_when_none() {
        let config = HttpConfig::default();
        let json = serde_json::to_string(&config).unwrap();

        // auth_token should not appear in JSON when None
        assert!(!json.contains("auth_token"));
    }

    #[test]
    fn test_http_validation_empty_bind_address() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.http.enabled = true;
        config.http.bind_address = "".to_string();

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("bind_address")));
    }

    #[test]
    fn test_http_validation_short_auth_token() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.http.enabled = true;
        config.http.auth_token = Some("short".to_string());

        let result = config.validate();
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("auth_token")));
    }

    #[test]
    fn test_http_validation_valid_config() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.http.enabled = true;
        config.http.port = 8080;
        config.http.auth_token = Some("longenoughtoken".to_string());

        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_merge_cli_args_http_port() {
        let mut config = Config::default();
        assert!(!config.http.enabled);
        assert_eq!(config.http.port, 0);

        config.merge_cli_args(
            None, None, None, None, None, None, false, None, None, None, None, None,
            Some(9000),
            None,
        );

        // Setting http_port should enable HTTP and set the port
        assert!(config.http.enabled);
        assert_eq!(config.http.port, 9000);
    }

    #[test]
    fn test_merge_cli_args_max_cache_mb() {
        let mut config = Config::default();
        assert_eq!(config.cache.max_memory_mb, 512); // default

        config.merge_cli_args(
            None, None, None, None, None, None, false, None, None, None, None, None,
            None,
            Some(1024),
        );

        assert_eq!(config.cache.max_memory_mb, 1024);
    }

    #[test]
    fn test_resampler_quality_default() {
        let config = Config::default();
        assert_eq!(config.advanced.resampler_quality, ResamplerQuality::Fast);
    }

    #[test]
    fn test_resampler_quality_parse() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "advanced": {
                "resampler_quality": "medium"
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.advanced.resampler_quality, ResamplerQuality::Medium);
    }

    #[test]
    fn test_resampler_quality_all_values() {
        for (json_value, expected) in [
            ("fast", ResamplerQuality::Fast),
            ("medium", ResamplerQuality::Medium),
            ("high", ResamplerQuality::High),
            ("maximum", ResamplerQuality::Maximum),
        ] {
            let json = format!(r#"{{"mqtt": {{"topic": "test"}}, "advanced": {{"resampler_quality": "{}"}}}}"#, json_value);
            let config: Config = serde_json::from_str(&json).unwrap();
            assert_eq!(config.advanced.resampler_quality, expected, "Failed for {}", json_value);
        }
    }

    #[test]
    fn test_resampler_quality_methods() {
        // Fast
        assert_eq!(ResamplerQuality::Fast.sinc_len(), 64);
        assert_eq!(ResamplerQuality::Fast.oversampling_factor(), 64);

        // Medium
        assert_eq!(ResamplerQuality::Medium.sinc_len(), 128);
        assert_eq!(ResamplerQuality::Medium.oversampling_factor(), 128);

        // High
        assert_eq!(ResamplerQuality::High.sinc_len(), 256);
        assert_eq!(ResamplerQuality::High.oversampling_factor(), 128);

        // Maximum
        assert_eq!(ResamplerQuality::Maximum.sinc_len(), 256);
        assert_eq!(ResamplerQuality::Maximum.oversampling_factor(), 256);
    }

    #[test]
    fn test_advanced_config_default() {
        let config = AdvancedConfig::default();
        assert_eq!(config.resampler_quality, ResamplerQuality::Fast);
    }

    #[test]
    fn test_macros_default_empty() {
        let config = Config::default();
        assert!(config.macros.is_empty());
    }

    #[test]
    fn test_macros_parse() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "macros": {
                "wholeroom": {
                    "channel_map": [{"src": 0, "dest": "left"}, {"src": 1, "dest": "right"}],
                    "volume": 0.2
                },
                "quiet": {
                    "volume": 0.1
                }
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        assert_eq!(config.macros.len(), 2);
        assert!(config.macros.contains_key("wholeroom"));
        assert!(config.macros.contains_key("quiet"));

        let wholeroom = &config.macros["wholeroom"];
        assert_eq!(wholeroom["volume"], 0.2);
        assert!(wholeroom["channel_map"].is_array());

        let quiet = &config.macros["quiet"];
        assert_eq!(quiet["volume"], 0.1);
    }

    #[test]
    fn test_macros_with_complex_values() {
        let json = r#"{
            "mqtt": {"topic": "test"},
            "macros": {
                "surround": {
                    "channel_map": [
                        {"src": 0, "dest": 0},
                        {"src": 1, "dest": 1},
                        {"src": 0, "dest": 4},
                        {"src": 1, "dest": 5}
                    ],
                    "volume": 0.5,
                    "voice": "surround_effects",
                    "fade_in": 1000
                }
            }
        }"#;

        let config: Config = serde_json::from_str(json).unwrap();

        let surround = &config.macros["surround"];
        assert_eq!(surround["volume"], 0.5);
        assert_eq!(surround["voice"], "surround_effects");
        assert_eq!(surround["fade_in"], 1000);
        assert_eq!(surround["channel_map"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn test_macros_serialization() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.macros.insert(
            "test_macro".to_string(),
            serde_json::json!({"volume": 0.5}),
        );

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("macros"));
        assert!(json.contains("test_macro"));
        assert!(json.contains("0.5"));
    }
}
