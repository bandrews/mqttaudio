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
}

impl Voice {
    /// Create a new voice with the given ID
    pub fn new(id: String) -> Self {
        Self {
            id,
            sample_ids: Vec::new(),
            volume: 1.0,
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

    /// Generate a unique sample ID and add it to a voice
    pub fn add_sample_to_voice(&mut self, voice_id: &str) -> u64 {
        let sample_id = self.next_sample_id;
        self.next_sample_id += 1;

        let voice = self.get_or_create_voice(voice_id);
        voice.add_sample(sample_id);

        sample_id
    }

    /// Remove a sample from its voice
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn remove_sample(&mut self, sample_id: u64) {
        // Find and remove from all voices
        for voice in self.voices.values_mut() {
            voice.remove_sample(sample_id);
        }

        // Clean up empty voices
        self.voices.retain(|_, voice| !voice.is_empty());
    }

    /// Remove all samples from a voice
    pub fn clear_voice(&mut self, voice_id: &str) -> Vec<u64> {
        if let Some(voice) = self.voices.get_mut(voice_id) {
            let sample_ids = voice.sample_ids.clone();
            voice.sample_ids.clear();
            self.voices.retain(|_, voice| !voice.is_empty());
            sample_ids
        } else {
            Vec::new()
        }
    }

    /// Get all sample IDs in a voice
    pub fn get_voice_sample_ids(&self, voice_id: &str) -> Vec<u64> {
        self.voices
            .get(voice_id)
            .map(|v| v.sample_ids.clone())
            .unwrap_or_default()
    }

    /// Set voice volume
    pub fn set_voice_volume(&mut self, voice_id: &str, volume: f32) -> bool {
        if let Some(voice) = self.voices.get_mut(voice_id) {
            voice.volume = volume.clamp(0.0, 1.0);
            true
        } else {
            false
        }
    }

    /// Get voice volume
    pub fn get_voice_volume(&self, voice_id: &str) -> Option<f32> {
        self.voices.get(voice_id).map(|v| v.volume)
    }

    /// Get total number of voices
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    /// Get total number of samples across all voices
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn total_sample_count(&self) -> usize {
        self.voices.values().map(|v| v.sample_count()).sum()
    }

    /// Clean up empty voices
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn cleanup_empty_voices(&mut self) {
        self.voices.retain(|_, voice| !voice.is_empty());
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
        // Voice "ambience" should be cleaned up
        assert_eq!(manager.voice_count(), 1);
        assert_eq!(manager.total_sample_count(), 1);
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
        assert!(manager.set_voice_volume("ambience", 0.5));
        assert_eq!(manager.get_voice_volume("ambience"), Some(0.5));

        // Volume should clamp
        manager.set_voice_volume("ambience", 2.0);
        assert_eq!(manager.get_voice_volume("ambience"), Some(1.0));

        manager.set_voice_volume("ambience", -0.5);
        assert_eq!(manager.get_voice_volume("ambience"), Some(0.0));

        // Non-existent voice should return None/false
        assert_eq!(manager.get_voice_volume("nonexistent"), None);
        assert!(!manager.set_voice_volume("nonexistent", 0.5));
    }

    #[test]
    fn test_voice_manager_cleanup() {
        let mut manager = VoiceManager::new();

        manager.add_sample_to_voice("ambience");
        manager.add_sample_to_voice("music");

        assert_eq!(manager.voice_count(), 2);

        // Manually clear samples from ambience
        if let Some(voice) = manager.get_voice_mut("ambience") {
            voice.sample_ids.clear();
        }

        assert_eq!(manager.voice_count(), 2); // Still there until cleanup

        manager.cleanup_empty_voices();
        assert_eq!(manager.voice_count(), 1); // ambience removed

        // Verify music is still there
        assert!(manager.get_voice("music").is_some());
    }
}
