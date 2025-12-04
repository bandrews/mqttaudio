// ABOUTME: Configuration loading and management.
// ABOUTME: Parses JSON config files and merges with CLI arguments.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;
use crate::audio::ducking::DuckingRule;

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
                    .ok_or_else(|| format!("Unknown channel alias: '{}'", name))
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
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: "~/.mqttaudio/cache".to_string(),
            revalidate_after_seconds: 300,
            precache: Vec::new(),
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
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            device: None,
            volume: 1.0,
            voice_id: "mic".to_string(),
            routes: Vec::new(),
            latency_ms: 20,
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
    }

    /// Resolve a channel reference to a numeric index using this config's aliases
    pub fn resolve_channel(&self, channel: &ChannelRef) -> Result<usize, String> {
        channel.resolve(&self.audio.channel_aliases)
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

        // Channel volumes must be 0.0 to 1.0
        for (ch, vol) in &self.audio.channel_volumes {
            if *vol < 0.0 || *vol > 1.0 {
                errors.push(format!("audio.channel_volumes.{} must be between 0.0 and 1.0", ch));
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
            // Validate channel aliases resolve
            if let Err(e) = self.resolve_channel(&self.bass_management.lfe_channel) {
                errors.push(format!("bass_management.lfe_channel: {}", e));
            }
            for (i, ch) in self.bass_management.source_channels.iter().enumerate() {
                if let Err(e) = self.resolve_channel(ch) {
                    errors.push(format!("bass_management.source_channels[{}]: {}", i, e));
                }
            }
        }

        // Input validation
        for (i, input) in self.inputs.iter().enumerate() {
            if input.volume < 0.0 || input.volume > 1.0 {
                errors.push(format!("inputs[{}].volume must be between 0.0 and 1.0", i));
            }
            if input.routes.is_empty() {
                errors.push(format!("inputs[{}].routes must not be empty", i));
            }
            if input.latency_ms < 5 || input.latency_ms > 500 {
                errors.push(format!("inputs[{}].latency_ms must be between 5 and 500", i));
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
        config.audio.channel_volumes.insert("0".to_string(), 1.5);

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

        config.merge_cli_args(None, None, None, None, None, None, true, None, None, None, None, None, None);

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
    fn test_input_validation_invalid_volume() {
        let mut config = Config::default();
        config.mqtt.topic = Some("test".to_string());
        config.inputs.push(InputConfig {
            volume: 1.5, // Invalid
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
        });

        let result = config.validate();
        assert!(result.is_ok());
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
            None, None, None,
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
        );

        // Setting http_port should enable HTTP and set the port
        assert!(config.http.enabled);
        assert_eq!(config.http.port, 9000);
    }
}
