// ABOUTME: Voice management for grouping and controlling audio samples.
// ABOUTME: Handles voice operations like stop, fade, and volume control.

use std::collections::HashMap;

/// A voice represents a named group of samples that can be controlled together
#[derive(Debug, Clone)]
pub struct Voice {
    /// Voice name/ID
    #[cfg_attr(not(test), allow(dead_code))]
    pub id: String,
    /// IDs of active samples in this voice
    pub sample_ids: Vec<u64>,
    /// Voice-level volume (0.0 - 1.0)
    pub volume: f32,
    /// Playback speed given to the voice's samples (1.0 = normal)
    pub speed: f32,
    /// Whether the voice's samples keep their pitch when their speed changes
    pub pitch_correction: bool,
    /// Exists only because a play named no voice; forgotten once its last
    /// sample finishes unless a setting is stored on it
    pub transient: bool,
    /// Fed by a configured live input, so it stays registered for the life
    /// of the process
    pub has_live_input: bool,
}

impl Voice {
    /// Create a new voice with the given ID
    pub fn new(id: String) -> Self {
        Self {
            id,
            sample_ids: Vec::new(),
            volume: 1.0,
            speed: 1.0,
            pitch_correction: false,
            transient: false,
            has_live_input: false,
        }
    }

    /// Add a sample ID to this voice
    pub fn add_sample(&mut self, sample_id: u64) {
        self.sample_ids.push(sample_id);
    }

    /// Remove a sample ID from this voice
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn remove_sample(&mut self, sample_id: u64) {
        self.sample_ids.retain(|&id| id != sample_id);
    }

    /// Check if this voice is empty (no active samples)
    pub fn is_empty(&self) -> bool {
        self.sample_ids.is_empty()
    }

    /// Check if this voice is producing sound (playing samples or a live input)
    pub fn is_active(&self) -> bool {
        !self.is_empty() || self.has_live_input
    }

    /// Get the number of active samples in this voice
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn sample_count(&self) -> usize {
        self.sample_ids.len()
    }
}

/// Manager for all voices
#[derive(Debug)]
pub struct VoiceManager {
    /// Map of voice ID to voice
    voices: HashMap<String, Voice>,
    /// Next sample ID to assign
    next_sample_id: u64,
}

impl VoiceManager {
    /// Create a new voice manager
    pub fn new() -> Self {
        Self {
            voices: HashMap::new(),
            next_sample_id: 1,
        }
    }

    /// Get or create a voice by ID
    pub fn get_or_create_voice(&mut self, voice_id: &str) -> &mut Voice {
        self.voices
            .entry(voice_id.to_string())
            .or_insert_with(|| Voice::new(voice_id.to_string()))
    }

    /// Get a voice by ID (read-only)
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get_voice(&self, voice_id: &str) -> Option<&Voice> {
        self.voices.get(voice_id)
    }

    /// Get a mutable reference to a voice by ID
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get_voice_mut(&mut self, voice_id: &str) -> Option<&mut Voice> {
        self.voices.get_mut(voice_id)
    }

    /// Get or create a voice that has a setting stored on it, which keeps
    /// it from being forgotten when its samples finish
    fn get_or_create_configured_voice(&mut self, voice_id: &str) -> &mut Voice {
        let voice = self.get_or_create_voice(voice_id);
        voice.transient = false;
        voice
    }

    /// Generate a unique sample ID and add it to a named voice
    pub fn add_sample_to_voice(&mut self, voice_id: &str) -> u64 {
        let sample_id = self.next_sample_id;
        self.next_sample_id += 1;

        let voice = self.get_or_create_voice(voice_id);
        voice.add_sample(sample_id);

        sample_id
    }

    /// Generate a unique sample ID and add it to a voice created for a play
    /// that named none. The voice is forgotten once its last sample finishes.
    pub fn add_sample_to_anonymous_voice(&mut self, voice_id: &str) -> u64 {
        let is_new = !self.voices.contains_key(voice_id);
        let sample_id = self.add_sample_to_voice(voice_id);
        if is_new {
            self.get_or_create_voice(voice_id).transient = true;
        }
        sample_id
    }

    /// Register the voice a configured live input feeds
    pub fn register_input_voice(&mut self, voice_id: &str) {
        self.get_or_create_voice(voice_id).has_live_input = true;
    }

    /// Remove a sample from its voice, forgetting the voice if it was
    /// transient and has no samples left
    pub fn remove_sample(&mut self, sample_id: u64) {
        // Find and remove from all voices
        for voice in self.voices.values_mut() {
            voice.remove_sample(sample_id);
        }

        self.voices.retain(|_, voice| !(voice.transient && voice.is_empty()));
    }

    /// Forget a voice and its settings, returning the IDs of its samples so
    /// they can be stopped. A voice fed by a live input stays registered
    /// with default settings.
    pub fn clear_voice(&mut self, voice_id: &str) -> Vec<u64> {
        match self.voices.remove(voice_id) {
            Some(voice) => {
                if voice.has_live_input {
                    self.register_input_voice(voice_id);
                }
                voice.sample_ids
            }
            None => Vec::new(),
        }
    }

    /// Forget every voice, as clear_voice does for one, returning the IDs of
    /// all their samples
    pub fn clear_all_voices(&mut self) -> Vec<u64> {
        let voice_ids: Vec<String> = self.voices.keys().cloned().collect();
        voice_ids
            .iter()
            .flat_map(|voice_id| self.clear_voice(voice_id))
            .collect()
    }

    /// Get all sample IDs in a voice
    pub fn get_voice_sample_ids(&self, voice_id: &str) -> Vec<u64> {
        self.voices
            .get(voice_id)
            .map(|v| v.sample_ids.clone())
            .unwrap_or_default()
    }

    /// Set voice volume, creating the voice if needed. Returns the stored
    /// (clamped) volume.
    pub fn set_voice_volume(&mut self, voice_id: &str, volume: f32) -> f32 {
        let voice = self.get_or_create_configured_voice(voice_id);
        voice.volume = volume.clamp(0.0, crate::config::MAX_GAIN);
        voice.volume
    }

    /// Set the speed and pitch mode given to the voice's samples, creating
    /// the voice if needed
    pub fn set_voice_speed(&mut self, voice_id: &str, speed: f32, pitch_correction: bool) {
        let voice = self.get_or_create_configured_voice(voice_id);
        voice.speed = speed;
        voice.pitch_correction = pitch_correction;
    }

    /// Get voice speed and pitch mode
    pub fn get_voice_speed(&self, voice_id: &str) -> Option<(f32, bool)> {
        self.voices.get(voice_id).map(|v| (v.speed, v.pitch_correction))
    }

    /// Get voice volume
    pub fn get_voice_volume(&self, voice_id: &str) -> Option<f32> {
        self.voices.get(voice_id).map(|v| v.volume)
    }

    /// Get total number of voices, active or not
    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    /// Get number of voices playing samples or fed by a live input
    pub fn active_voice_count(&self) -> usize {
        self.voices.values().filter(|v| v.is_active()).count()
    }

    /// List all voices with their details
    pub fn list_voices(&self) -> Vec<serde_json::Value> {
        self.voices
            .iter()
            .map(|(id, voice)| {
                serde_json::json!({
                    "id": id,
                    "sample_count": voice.sample_count(),
                    "volume": voice.volume,
                    "speed": voice.speed,
                    "pitch_correction": voice.pitch_correction,
                    "active": voice.is_active()
                })
            })
            .collect()
    }

    /// Get total number of samples across all voices
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn total_sample_count(&self) -> usize {
        self.voices.values().map(|v| v.sample_count()).sum()
    }
}

impl Default for VoiceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_voice_creation() {
        let voice = Voice::new("test".to_string());
        assert_eq!(voice.id, "test");
        assert_eq!(voice.volume, 1.0);
        assert!(voice.is_empty());
        assert_eq!(voice.sample_count(), 0);
    }

    #[test]
    fn test_voice_add_remove_sample() {
        let mut voice = Voice::new("test".to_string());

        voice.add_sample(1);
        assert_eq!(voice.sample_count(), 1);
        assert!(!voice.is_empty());

        voice.add_sample(2);
        assert_eq!(voice.sample_count(), 2);

        voice.remove_sample(1);
        assert_eq!(voice.sample_count(), 1);
        assert_eq!(voice.sample_ids, vec![2]);

        voice.remove_sample(2);
        assert!(voice.is_empty());
    }

    #[test]
    fn test_voice_manager_creation() {
        let manager = VoiceManager::new();
        assert_eq!(manager.voice_count(), 0);
        assert_eq!(manager.total_sample_count(), 0);
    }

    #[test]
    fn test_voice_manager_get_or_create() {
        let mut manager = VoiceManager::new();

        let voice = manager.get_or_create_voice("ambience");
        assert_eq!(voice.id, "ambience");
        assert_eq!(manager.voice_count(), 1);

        // Getting again should not create a new one
        let voice2 = manager.get_or_create_voice("ambience");
        assert_eq!(voice2.id, "ambience");
        assert_eq!(manager.voice_count(), 1);

        // Different voice should create new one
        manager.get_or_create_voice("music");
        assert_eq!(manager.voice_count(), 2);
    }

    #[test]
    fn test_voice_manager_add_sample() {
        let mut manager = VoiceManager::new();

        let sample_id1 = manager.add_sample_to_voice("ambience");
        assert_eq!(sample_id1, 1);
        assert_eq!(manager.voice_count(), 1);
        assert_eq!(manager.total_sample_count(), 1);

        let sample_id2 = manager.add_sample_to_voice("ambience");
        assert_eq!(sample_id2, 2);
        assert_eq!(manager.voice_count(), 1);
        assert_eq!(manager.total_sample_count(), 2);

        let sample_id3 = manager.add_sample_to_voice("music");
        assert_eq!(sample_id3, 3);
        assert_eq!(manager.voice_count(), 2);
        assert_eq!(manager.total_sample_count(), 3);
    }

    #[test]
    fn test_voice_manager_remove_sample() {
        let mut manager = VoiceManager::new();

        let id1 = manager.add_sample_to_voice("ambience");
        let id2 = manager.add_sample_to_voice("ambience");
        manager.add_sample_to_voice("music");

        assert_eq!(manager.voice_count(), 2);
        assert_eq!(manager.total_sample_count(), 3);

        manager.remove_sample(id1);
        assert_eq!(manager.total_sample_count(), 2);

        manager.remove_sample(id2);
        // A named voice outlives its samples
        assert_eq!(manager.voice_count(), 2);
        assert_eq!(manager.total_sample_count(), 1);
        assert!(manager.get_voice("ambience").unwrap().is_empty());
    }

    #[test]
    fn test_named_voice_keeps_settings_after_last_sample_finishes() {
        let mut manager = VoiceManager::new();

        let id = manager.add_sample_to_voice("music");
        manager.set_voice_volume("music", 0.3);
        manager.set_voice_speed("music", 1.5, true);
        manager.remove_sample(id);

        assert_eq!(manager.get_voice_volume("music"), Some(0.3));
        assert_eq!(manager.get_voice_speed("music"), Some((1.5, true)));
    }

    #[test]
    fn test_anonymous_voice_is_forgotten_after_last_sample_finishes() {
        let mut manager = VoiceManager::new();

        let id1 = manager.add_sample_to_anonymous_voice("_auto_1");
        let id2 = manager.add_sample_to_anonymous_voice("_auto_1");
        manager.remove_sample(id1);
        assert!(manager.get_voice("_auto_1").is_some());

        manager.remove_sample(id2);
        assert!(manager.get_voice("_auto_1").is_none());
        assert_eq!(manager.voice_count(), 0);
    }

    #[test]
    fn test_setting_on_anonymous_voice_keeps_it() {
        let mut manager = VoiceManager::new();

        let id = manager.add_sample_to_anonymous_voice("_auto_1");
        manager.set_voice_volume("_auto_1", 0.5);
        manager.remove_sample(id);

        assert_eq!(manager.get_voice_volume("_auto_1"), Some(0.5));
    }

    #[test]
    fn test_set_voice_volume_creates_voice() {
        let mut manager = VoiceManager::new();

        assert_eq!(manager.set_voice_volume("music", 0.4), 0.4);
        assert_eq!(manager.voice_count(), 1);
        assert_eq!(manager.get_voice_volume("music"), Some(0.4));
    }

    #[test]
    fn test_set_voice_speed_creates_voice() {
        let mut manager = VoiceManager::new();

        assert_eq!(manager.get_voice_speed("music"), None);
        manager.set_voice_speed("music", 0.75, false);
        assert_eq!(manager.voice_count(), 1);
        assert_eq!(manager.get_voice_speed("music"), Some((0.75, false)));
    }

    #[test]
    fn test_new_voice_has_default_speed() {
        let mut manager = VoiceManager::new();

        manager.add_sample_to_voice("music");
        assert_eq!(manager.get_voice_speed("music"), Some((1.0, false)));
    }

    #[test]
    fn test_input_voice_is_registered_with_defaults() {
        let mut manager = VoiceManager::new();

        manager.register_input_voice("gm_mic");
        assert_eq!(manager.voice_count(), 1);
        assert_eq!(manager.get_voice_volume("gm_mic"), Some(1.0));
    }

    #[test]
    fn test_clear_voice_forgets_named_voice() {
        let mut manager = VoiceManager::new();

        let id = manager.add_sample_to_voice("music");
        manager.set_voice_volume("music", 0.2);

        assert_eq!(manager.clear_voice("music"), vec![id]);
        assert!(manager.get_voice("music").is_none());
        assert_eq!(manager.voice_count(), 0);
    }

    #[test]
    fn test_clear_voice_resets_input_voice_but_keeps_it() {
        let mut manager = VoiceManager::new();

        manager.register_input_voice("gm_mic");
        let id = manager.add_sample_to_voice("gm_mic");
        manager.set_voice_volume("gm_mic", 0.2);
        manager.set_voice_speed("gm_mic", 2.0, false);

        assert_eq!(manager.clear_voice("gm_mic"), vec![id]);
        let voice = manager.get_voice("gm_mic").unwrap();
        assert!(voice.is_empty());
        assert_eq!(voice.volume, 1.0);
        assert_eq!(voice.speed, 1.0);
        assert!(voice.has_live_input);
    }

    #[test]
    fn test_clear_all_voices() {
        let mut manager = VoiceManager::new();

        manager.register_input_voice("gm_mic");
        manager.set_voice_volume("gm_mic", 0.5);
        let id1 = manager.add_sample_to_voice("music");
        let id2 = manager.add_sample_to_anonymous_voice("_auto_1");
        manager.set_voice_volume("effects", 0.5);

        let mut cleared = manager.clear_all_voices();
        cleared.sort();
        assert_eq!(cleared, vec![id1, id2]);
        assert_eq!(manager.voice_count(), 1);
        assert_eq!(manager.get_voice_volume("gm_mic"), Some(1.0));
    }

    #[test]
    fn test_active_voice_count() {
        let mut manager = VoiceManager::new();

        manager.register_input_voice("gm_mic");
        let id = manager.add_sample_to_voice("music");
        manager.set_voice_volume("effects", 0.5);

        assert_eq!(manager.voice_count(), 3);
        assert_eq!(manager.active_voice_count(), 2);

        manager.remove_sample(id);
        assert_eq!(manager.active_voice_count(), 1);
    }

    #[test]
    fn test_list_voices_reports_settings_and_activity() {
        let mut manager = VoiceManager::new();

        manager.set_voice_speed("music", 1.25, true);
        let listed = manager.list_voices();

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["id"], "music");
        assert_eq!(listed[0]["speed"], 1.25);
        assert_eq!(listed[0]["pitch_correction"], true);
        assert_eq!(listed[0]["active"], false);
        assert_eq!(listed[0]["sample_count"], 0);
    }

    #[test]
    fn test_voice_manager_clear_voice() {
        let mut manager = VoiceManager::new();

        manager.add_sample_to_voice("ambience");
        manager.add_sample_to_voice("ambience");
        manager.add_sample_to_voice("music");

        let cleared = manager.clear_voice("ambience");
        assert_eq!(cleared.len(), 2);
        assert_eq!(manager.voice_count(), 1);
        assert_eq!(manager.total_sample_count(), 1);

        // Clearing non-existent voice should return empty vec
        let cleared = manager.clear_voice("nonexistent");
        assert_eq!(cleared.len(), 0);
    }

    #[test]
    fn test_voice_manager_get_voice_sample_ids() {
        let mut manager = VoiceManager::new();

        let id1 = manager.add_sample_to_voice("ambience");
        let id2 = manager.add_sample_to_voice("ambience");

        let sample_ids = manager.get_voice_sample_ids("ambience");
        assert_eq!(sample_ids.len(), 2);
        assert!(sample_ids.contains(&id1));
        assert!(sample_ids.contains(&id2));

        // Non-existent voice should return empty vec
        let sample_ids = manager.get_voice_sample_ids("nonexistent");
        assert_eq!(sample_ids.len(), 0);
    }

    #[test]
    fn test_voice_manager_volume() {
        let mut manager = VoiceManager::new();

        manager.add_sample_to_voice("ambience");

        // Default volume should be 1.0
        assert_eq!(manager.get_voice_volume("ambience"), Some(1.0));

        // Set volume
        assert_eq!(manager.set_voice_volume("ambience", 0.5), 0.5);
        assert_eq!(manager.get_voice_volume("ambience"), Some(0.5));

        // Boosting above unity is allowed
        manager.set_voice_volume("ambience", 2.0);
        assert_eq!(manager.get_voice_volume("ambience"), Some(2.0));

        // Volume should clamp at both ends
        manager.set_voice_volume("ambience", 100.0);
        assert_eq!(manager.get_voice_volume("ambience"), Some(crate::config::MAX_GAIN));

        manager.set_voice_volume("ambience", -0.5);
        assert_eq!(manager.get_voice_volume("ambience"), Some(0.0));

        // Non-existent voice has no volume
        assert_eq!(manager.get_voice_volume("nonexistent"), None);
    }
}
