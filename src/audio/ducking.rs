// ABOUTME: Audio ducking engine for automatic volume reduction of background voices.
// ABOUTME: Manages ducking rules, voice activity tracking, and smooth fade calculations.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Ducking rule from configuration
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct DuckingRule {
    /// Voice that triggers ducking (when it has active samples)
    pub primary_voice: String,

    /// Voices to duck when primary is active
    pub ducked_voices: Vec<String>,

    /// Target volume for ducked voices (0.0 - 1.0)
    pub target_volume: f32,

    /// Fade duration in milliseconds
    pub fade_duration_ms: u32,
}

/// State of ducking for a single voice
#[derive(Debug, Clone)]
struct DuckState {
    /// Target volume to duck to (from applicable rules)
    target_volume: f32,

    /// Current volume multiplier (smooth fade in progress)
    current_multiplier: f32,

    /// Starting multiplier when fade began (for linear interpolation)
    start_multiplier: f32,

    /// Fade duration in frames
    fade_duration_frames: usize,

    /// Frames elapsed in current fade
    fade_elapsed_frames: usize,

    /// Is currently ducking (vs. restoring)
    is_ducking: bool,

    /// Fade duration of the rule that last ducked this voice, reused as the
    /// restore duration so recovery matches the configured fade
    last_duck_fade_frames: usize,
}

impl DuckState {
    /// Create a new duck state at full volume (not ducking)
    fn new() -> Self {
        Self {
            target_volume: 1.0,
            current_multiplier: 1.0,
            start_multiplier: 1.0,
            fade_duration_frames: 0,
            fade_elapsed_frames: 0,
            is_ducking: false,
            last_duck_fade_frames: 0,
        }
    }

    /// Begin ducking to a new target volume
    fn begin_duck(&mut self, target_volume: f32, fade_duration_frames: usize) {
        self.start_multiplier = self.current_multiplier;
        self.target_volume = target_volume;
        self.fade_duration_frames = fade_duration_frames;
        self.fade_elapsed_frames = 0;
        self.is_ducking = true;
        self.last_duck_fade_frames = fade_duration_frames;
    }

    /// Begin restoring to full volume
    fn begin_restore(&mut self, fade_duration_frames: usize) {
        self.start_multiplier = self.current_multiplier;
        self.target_volume = 1.0;
        self.fade_duration_frames = fade_duration_frames;
        self.fade_elapsed_frames = 0;
        self.is_ducking = false;
    }

    /// Advance fade state by given number of frames and return current multiplier
    fn advance_and_get_multiplier(&mut self, frames: usize) -> f32 {
        // Advance fade state
        self.fade_elapsed_frames = (self.fade_elapsed_frames + frames)
            .min(self.fade_duration_frames);

        // Calculate progress (0.0 to 1.0)
        let progress = if self.fade_duration_frames == 0 {
            1.0
        } else {
            self.fade_elapsed_frames as f32 / self.fade_duration_frames as f32
        };

        // Linear interpolation from start_multiplier to target_volume
        self.current_multiplier = self.start_multiplier + (self.target_volume - self.start_multiplier) * progress;

        self.current_multiplier
    }

    /// Get current multiplier without advancing
    #[cfg(test)]
    fn get_multiplier(&self) -> f32 {
        self.current_multiplier
    }
}

/// Ducking engine that manages all ducking state and rules
pub struct DuckingEngine {
    /// Configured ducking rules
    rules: Vec<DuckingRule>,

    /// Sample rate (for converting ms to frames)
    sample_rate: u32,

    /// Active voices (voice_id -> has_active_samples)
    active_voices: HashMap<String, bool>,

    /// Duck states per voice
    duck_states: HashMap<String, DuckState>,
}

impl DuckingEngine {
    /// Create a new ducking engine with given rules and sample rate
    pub fn new(rules: Vec<DuckingRule>, sample_rate: u32) -> Self {
        tracing::info!("Initializing ducking engine with {} rules at {} Hz", rules.len(), sample_rate);
        for rule in &rules {
            tracing::debug!(
                "  Rule: '{}' ducks {:?} to {:.1}% over {}ms",
                rule.primary_voice,
                rule.ducked_voices,
                rule.target_volume * 100.0,
                rule.fade_duration_ms
            );
        }

        Self {
            rules,
            sample_rate,
            active_voices: HashMap::new(),
            duck_states: HashMap::new(),
        }
    }

    /// Notify engine that a voice's activity state has changed
    pub fn notify_voice_active(&mut self, voice_id: &str, is_active: bool) {
        tracing::debug!("Voice '{}' activity changed: {}", voice_id, is_active);

        // Update active voices map
        if is_active {
            self.active_voices.insert(voice_id.to_string(), true);
        } else {
            self.active_voices.insert(voice_id.to_string(), false);
        }

        // Recalculate all duck states
        self.update_duck_states();
    }

    /// Advance every voice's fade state by one callback's worth of frames.
    /// Called exactly once per audio callback; get_multiplier then reads the
    /// resulting values. Advancing per-sample instead would make a fade run
    /// N times too fast for a voice with N samples.
    pub fn advance(&mut self, frames: usize) {
        for state in self.duck_states.values_mut() {
            state.advance_and_get_multiplier(frames);
        }
    }

    /// Get the current ducking multiplier for a voice.
    /// This is called from the mixer callback for each sample.
    pub fn get_multiplier(&self, voice_id: &str) -> f32 {
        self.duck_states
            .get(voice_id)
            .map(|s| s.current_multiplier)
            .unwrap_or(1.0)
    }

    /// Get ducking multiplier without advancing (for testing)
    #[cfg(test)]
    pub fn peek_multiplier(&self, voice_id: &str) -> f32 {
        self.duck_states
            .get(voice_id)
            .map(|s| s.get_multiplier())
            .unwrap_or(1.0)
    }

    /// Update all duck states based on current voice activity
    fn update_duck_states(&mut self) {
        // Get all voices that might need ducking (any voice in any rule)
        let mut all_potential_voices: std::collections::HashSet<String> = std::collections::HashSet::new();

        for rule in &self.rules {
            all_potential_voices.insert(rule.primary_voice.clone());
            for ducked_voice in &rule.ducked_voices {
                all_potential_voices.insert(ducked_voice.clone());
            }
        }

        // For each voice, determine if it should be ducked
        for voice_id in all_potential_voices {
            let applicable_rules = self.find_applicable_rules(&voice_id);

            if applicable_rules.is_empty() {
                // No ducking needed - restore to full volume
                // Only start restore if we're not already at full volume
                let should_restore = self.duck_states
                    .get(&voice_id)
                    .map(|s| s.target_volume < 1.0 || s.is_ducking)
                    .unwrap_or(false);

                if should_restore {
                    // Restore over the same duration the ducking rule faded in
                    // with, so recovery speed matches the configured fade
                    let fade_frames = self.duck_states
                        .get(&voice_id)
                        .map(|s| s.last_duck_fade_frames)
                        .filter(|f| *f > 0)
                        .unwrap_or_else(|| self.ms_to_frames(2000));
                    tracing::debug!("Voice '{}' restoring to full volume over {} frames", voice_id, fade_frames);

                    let state = self.duck_states
                        .entry(voice_id.clone())
                        .or_insert_with(DuckState::new);
                    state.begin_restore(fade_frames);
                }
            } else {
                // Multiple rules: use lowest target volume and fastest fade
                let target_volume = applicable_rules.iter()
                    .map(|r| r.target_volume)
                    .min_by(|a, b| a.partial_cmp(b).unwrap())
                    .unwrap();

                let fade_duration_ms = applicable_rules.iter()
                    .map(|r| r.fade_duration_ms)
                    .min()
                    .unwrap();

                let fade_frames = self.ms_to_frames(fade_duration_ms);
                let current_multiplier = self.duck_states
                    .get(&voice_id)
                    .map(|s| s.current_multiplier)
                    .unwrap_or(1.0);

                // Check if parameters changed to avoid logging on every update
                let params_changed = self.duck_states
                    .get(&voice_id)
                    .map(|s| (s.target_volume - target_volume).abs() > 0.001
                           || s.fade_duration_frames != fade_frames
                           || !s.is_ducking)
                    .unwrap_or(true);

                if params_changed {
                    tracing::debug!(
                        "Voice '{}' ducking to {:.1}% over {}ms (from {:.1}%)",
                        voice_id,
                        target_volume * 100.0,
                        fade_duration_ms,
                        current_multiplier * 100.0
                    );
                }

                // Only restart the fade when the target actually changed;
                // restarting on every unrelated activity notification would
                // turn a linear fade into an asymptotic crawl
                if params_changed {
                    let state = self.duck_states
                        .entry(voice_id.clone())
                        .or_insert_with(DuckState::new);
                    state.begin_duck(target_volume, fade_frames);
                }
            }
        }
    }

    /// Find all rules that apply to a given voice (should it be ducked?)
    fn find_applicable_rules(&self, voice_id: &str) -> Vec<&DuckingRule> {
        self.rules.iter()
            .filter(|rule| {
                // Rule applies if:
                // 1. This voice is in the ducked_voices list
                // 2. The primary voice has active samples
                rule.ducked_voices.iter().any(|v| v == voice_id) &&
                self.active_voices.get(&rule.primary_voice).copied().unwrap_or(false)
            })
            .collect()
    }

    /// Convert milliseconds to frames based on sample rate
    fn ms_to_frames(&self, ms: u32) -> usize {
        ((ms as f32 / 1000.0) * self.sample_rate as f32) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_rule(primary: &str, ducked: Vec<&str>, target: f32, fade_ms: u32) -> DuckingRule {
        DuckingRule {
            primary_voice: primary.to_string(),
            ducked_voices: ducked.iter().map(|s| s.to_string()).collect(),
            target_volume: target,
            fade_duration_ms: fade_ms,
        }
    }

    #[test]
    fn test_ducking_engine_creation() {
        let rules = vec![
            create_test_rule("narration", vec!["music"], 0.1, 2000),
        ];
        let engine = DuckingEngine::new(rules, 48000);
        assert_eq!(engine.rules.len(), 1);
        assert_eq!(engine.sample_rate, 48000);
    }

    #[test]
    fn test_single_rule_ducking() {
        let rules = vec![
            create_test_rule("narration", vec!["music"], 0.1, 1000),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Initially, no ducking
        assert_eq!(engine.peek_multiplier("music"), 1.0);

        // Start narration - music should start ducking
        engine.notify_voice_active("narration", true);

        // Duck state should be initiated but not advanced yet
        assert!(engine.peek_multiplier("music") > 0.09 && engine.peek_multiplier("music") <= 1.0);

        // Advance partway through fade
        let half_fade_frames = 24000; // 0.5s at 48kHz
        engine.advance(half_fade_frames);

        // Should be somewhere between 0.1 and 1.0
        let multiplier = engine.peek_multiplier("music");
        assert!(multiplier > 0.1 && multiplier < 1.0, "Expected multiplier between 0.1 and 1.0, got {}", multiplier);

        // Stop narration - music should restore
        engine.notify_voice_active("narration", false);

        // Advance through restore
        engine.advance(half_fade_frames);

        // Should be restoring toward 1.0
        let restored = engine.peek_multiplier("music");
        assert!(restored > multiplier, "Expected restoration, got {}", restored);
    }

    #[test]
    fn test_multiple_rules_lowest_volume() {
        let rules = vec![
            create_test_rule("narration", vec!["music"], 0.2, 1000),
            create_test_rule("dialog", vec!["music"], 0.1, 1000),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Start both narration and dialog
        engine.notify_voice_active("narration", true);
        engine.notify_voice_active("dialog", true);

        // Should use lowest target volume (0.1 from dialog rule)
        // Duck state should target 0.1
        assert!(engine.duck_states.contains_key("music"));
        assert_eq!(engine.duck_states.get("music").unwrap().target_volume, 0.1);
    }

    #[test]
    fn test_multiple_rules_fastest_fade() {
        let rules = vec![
            create_test_rule("narration", vec!["music"], 0.1, 2000),
            create_test_rule("dialog", vec!["music"], 0.1, 500),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Start both
        engine.notify_voice_active("narration", true);
        engine.notify_voice_active("dialog", true);

        // Should use fastest fade (500ms)
        let expected_frames = 24000; // 500ms at 48kHz
        assert_eq!(engine.duck_states.get("music").unwrap().fade_duration_frames, expected_frames);
    }

    #[test]
    fn test_no_ducking_when_primary_inactive() {
        let rules = vec![
            create_test_rule("narration", vec!["music"], 0.1, 1000),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Start music (not narration)
        engine.notify_voice_active("music", true);

        // Music should not be ducked
        assert_eq!(engine.peek_multiplier("music"), 1.0);
    }

    #[test]
    fn test_ducking_multiple_voices() {
        let rules = vec![
            create_test_rule("narration", vec!["music", "ambience", "effects"], 0.1, 1000),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Start narration
        engine.notify_voice_active("narration", true);

        // All ducked voices should have duck states
        assert!(engine.duck_states.contains_key("music"));
        assert!(engine.duck_states.contains_key("ambience"));
        assert!(engine.duck_states.contains_key("effects"));
    }

    #[test]
    fn test_voice_not_in_any_rule() {
        let rules = vec![
            create_test_rule("narration", vec!["music"], 0.1, 1000),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Start unrelated voice
        engine.notify_voice_active("unrelated", true);

        // Should have no effect on music
        assert_eq!(engine.peek_multiplier("music"), 1.0);
        assert_eq!(engine.peek_multiplier("unrelated"), 1.0);
    }

    #[test]
    fn test_transition_between_rules() {
        let rules = vec![
            create_test_rule("narration", vec!["music"], 0.2, 1000),
            create_test_rule("dialog", vec!["music"], 0.1, 500),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Start narration (music should duck to 0.2)
        engine.notify_voice_active("narration", true);
        assert_eq!(engine.duck_states.get("music").unwrap().target_volume, 0.2);

        // Add dialog (music should duck further to 0.1)
        engine.notify_voice_active("dialog", true);
        assert_eq!(engine.duck_states.get("music").unwrap().target_volume, 0.1);

        // Stop dialog (music should return to 0.2)
        engine.notify_voice_active("dialog", false);
        assert_eq!(engine.duck_states.get("music").unwrap().target_volume, 0.2);

        // Stop narration (music should restore to 1.0)
        engine.notify_voice_active("narration", false);
        assert_eq!(engine.duck_states.get("music").unwrap().target_volume, 1.0);
    }

    #[test]
    fn test_ms_to_frames_conversion() {
        let engine = DuckingEngine::new(vec![], 48000);

        assert_eq!(engine.ms_to_frames(1000), 48000);
        assert_eq!(engine.ms_to_frames(500), 24000);
        assert_eq!(engine.ms_to_frames(100), 4800);
    }

    #[test]
    fn test_zero_duration_fade() {
        let rules = vec![
            create_test_rule("narration", vec!["music"], 0.1, 0),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Start narration
        engine.notify_voice_active("narration", true);

        // With 0ms fade, should immediately be at target
        engine.advance(1);
        let multiplier = engine.get_multiplier("music");
        assert!((multiplier - 0.1).abs() < 0.01, "Expected ~0.1, got {}", multiplier);
    }

    #[test]
    fn test_fade_progress() {
        let rules = vec![
            create_test_rule("narration", vec!["music"], 0.2, 1000),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Start ducking
        engine.notify_voice_active("narration", true);

        // Advance in steps and verify gradual change
        let step_frames = 12000; // 0.25s at 48kHz

        engine.advance(step_frames);
        let m1 = engine.peek_multiplier("music");

        engine.advance(step_frames);
        let m2 = engine.peek_multiplier("music");

        engine.advance(step_frames);
        let m3 = engine.peek_multiplier("music");

        engine.advance(step_frames);
        let m4 = engine.peek_multiplier("music");

        // Should be gradually decreasing
        assert!(m1 > m2);
        assert!(m2 > m3);
        assert!(m3 > m4);

        // Final value should be close to target
        assert!((m4 - 0.2).abs() < 0.05, "Expected ~0.2, got {}", m4);
    }

    #[test]
    fn test_overlapping_primary_voices() {
        let rules = vec![
            create_test_rule("narration", vec!["music"], 0.3, 1000),
            create_test_rule("effects", vec!["music"], 0.5, 1000),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Start both primaries
        engine.notify_voice_active("narration", true);
        engine.notify_voice_active("effects", true);

        // Should use most aggressive (lowest) target
        assert_eq!(engine.duck_states.get("music").unwrap().target_volume, 0.3);
    }

    #[test]
    fn test_primary_voice_not_ducked() {
        let rules = vec![
            create_test_rule("narration", vec!["music"], 0.1, 1000),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Start narration
        engine.notify_voice_active("narration", true);

        // Narration itself should not be ducked
        assert_eq!(engine.peek_multiplier("narration"), 1.0);
    }

    #[test]
    fn test_empty_rules() {
        let mut engine = DuckingEngine::new(vec![], 48000);

        engine.notify_voice_active("any_voice", true);

        // No rules = no ducking
        assert_eq!(engine.peek_multiplier("any_voice"), 1.0);
        assert_eq!(engine.peek_multiplier("another_voice"), 1.0);
    }

    #[test]
    fn test_rule_with_no_ducked_voices() {
        let mut rule = create_test_rule("narration", vec![], 0.1, 1000);
        rule.ducked_voices.clear();

        let mut engine = DuckingEngine::new(vec![rule], 48000);

        engine.notify_voice_active("narration", true);
        engine.notify_voice_active("music", true);

        // Music should not be affected
        assert_eq!(engine.peek_multiplier("music"), 1.0);
    }

    #[test]
    fn test_smooth_mid_fade_transition() {
        // This test verifies that when ducking parameters change mid-fade,
        // the transition is smooth and linear (no glitches)
        let rules = vec![
            create_test_rule("narration", vec!["music"], 0.2, 1000),
            create_test_rule("dialog", vec!["music"], 0.1, 500),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Start with music at full volume
        assert_eq!(engine.peek_multiplier("music"), 1.0);

        // Start narration - music should begin ducking to 0.2 over 1000ms (48000 frames)
        engine.notify_voice_active("narration", true);

        // Advance halfway through the narration duck (500ms = 24000 frames)
        engine.advance(24000);
        let halfway_to_02 = engine.peek_multiplier("music");

        // Should be halfway between 1.0 and 0.2 = 0.6
        assert!(
            (halfway_to_02 - 0.6).abs() < 0.05,
            "Expected ~0.6, got {}",
            halfway_to_02
        );

        // Now dialog starts - music should immediately begin ducking to 0.1 over 500ms
        // It should start from wherever it currently is (0.6) and fade to 0.1
        engine.notify_voice_active("dialog", true);

        // Immediately after the state change, multiplier should still be at 0.6
        // (fade hasn't advanced yet)
        let pre_advance = engine.peek_multiplier("music");
        assert!(
            (pre_advance - halfway_to_02).abs() < 0.001,
            "Multiplier should not jump on state change, got {} -> {}",
            halfway_to_02,
            pre_advance
        );

        // Advance halfway through the new duck (250ms = 12000 frames)
        engine.advance(12000);
        let halfway_to_01 = engine.peek_multiplier("music");

        // Should be halfway between 0.6 and 0.1 = 0.35
        assert!(
            (halfway_to_01 - 0.35).abs() < 0.05,
            "Expected ~0.35, got {}",
            halfway_to_01
        );

        // Complete the fade to 0.1
        engine.advance(12000);
        let final_ducked = engine.peek_multiplier("music");

        assert!(
            (final_ducked - 0.1).abs() < 0.01,
            "Expected ~0.1, got {}",
            final_ducked
        );

        // Now dialog stops - music should restore from 0.1 toward 0.2 (narration still active)
        engine.notify_voice_active("dialog", false);

        // Advance partway through restore
        engine.advance(24000);
        let restoring = engine.peek_multiplier("music");

        // Should be between 0.1 and 0.2
        assert!(
            restoring > 0.1 && restoring < 0.2,
            "Expected between 0.1 and 0.2, got {}",
            restoring
        );
    }
}
