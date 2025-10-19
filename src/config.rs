// ABOUTME: Configuration loading and management.
// ABOUTME: Parses JSON config files and merges with CLI arguments.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::fs;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct MqttConfig {
    pub server: String,
    pub port: u16,
    pub topic: Option<String>,
    pub client_id: Option<String>,
    pub reconnect_delay_seconds: u64,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            server: "localhost".to_string(),
            port: 1883,
            topic: None,
            client_id: None,
            reconnect_delay_seconds: 10,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AudioConfig {
    pub device: Option<String>,
    pub sample_rate: u32,
    pub buffer_size: u32,
    #[serde(default)]
    pub channel_names: HashMap<String, String>,
    #[serde(default)]
    pub channel_volumes: HashMap<String, f32>,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            device: None,
            sample_rate: 48000,
            buffer_size: 512,
            channel_names: HashMap::new(),
            channel_volumes: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CacheConfig {
    pub enabled: bool,
    pub directory: String,
    pub revalidate_after_seconds: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: "~/.mqttaudio/cache".to_string(),
            revalidate_after_seconds: 300,
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
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            verbose: false,
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mqtt: MqttConfig::default(),
            audio: AudioConfig::default(),
            cache: CacheConfig::default(),
            security: SecurityConfig::default(),
            logging: LoggingConfig::default(),
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
    pub fn allowed_directories(&self) -> Vec<PathBuf> {
        self.security.allowed_directories
            .iter()
            .map(|d| PathBuf::from(Self::expand_tilde(d)))
            .collect()
    }

    /// Merge CLI arguments into this config (CLI args override config file)
    pub fn merge_cli_args(
        &mut self,
        server: Option<String>,
        port: Option<u16>,
        topic: Option<String>,
        device: Option<String>,
        sample_rate: Option<u32>,
        verbose: bool,
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

        // Override audio settings
        if let Some(d) = device {
            self.audio.device = Some(d);
        }
        if let Some(sr) = sample_rate {
            self.audio.sample_rate = sr;
        }

        // Override logging settings
        if verbose {
            self.logging.verbose = true;
            self.logging.level = "debug".to_string();
        }
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        // MQTT topic is required
        if self.mqtt.topic.is_none() {
            errors.push("mqtt.topic is required".to_string());
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
    ValidationError(Vec<String>),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::IoError(e) => write!(f, "IO error: {}", e),
            ConfigError::ParseError(e) => write!(f, "Parse error: {}", e),
            ConfigError::ValidationError(errors) => {
                write!(f, "Validation errors:\n")?;
                for error in errors {
                    write!(f, "  - {}\n", error)?;
                }
                Ok(())
            }
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
        assert_eq!(config.audio.sample_rate, 48000);
        assert_eq!(config.audio.buffer_size, 512);
        assert_eq!(config.cache.enabled, true);
        assert_eq!(config.cache.revalidate_after_seconds, 300);
        assert_eq!(config.logging.level, "info");
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
    fn test_validate_missing_topic() {
        let config = Config::default();
        let result = config.validate();

        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("mqtt.topic")));
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
            false,
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
            true,
        );

        assert_eq!(config.mqtt.server, "newserver");
        assert_eq!(config.mqtt.port, 8883);
        assert_eq!(config.mqtt.topic, Some("newtopic".to_string()));
        assert_eq!(config.audio.device, Some("newdevice".to_string()));
        assert_eq!(config.audio.sample_rate, 96000);
        assert_eq!(config.logging.verbose, true);
        assert_eq!(config.logging.level, "debug");
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
            false,
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

        config.merge_cli_args(None, None, None, None, None, true);

        assert_eq!(config.logging.verbose, true);
        assert_eq!(config.logging.level, "debug");
    }
}
