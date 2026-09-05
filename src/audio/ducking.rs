// ABOUTME: Audio ducking engine for automatic volume reduction of background voices.
// ABOUTME: Manages ducking rules, voice activity tracking, and smooth fade calculations.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A resolved change to one voice's ducking target, computed by the control
/// thread and sent to the audio thread to apply. Carries the new target volume
/// (>= 1.0 means restore) and the fade length in frames.
#[derive(Debug, Clone, PartialEq)]
pub struct DuckTargetChange {
    /// Voice whose ducking target is changing.
    pub voice: String,
    /// Target multiplier for the voice (>= 1.0 restores to full volume).
    pub target_volume: f32,
    /// Fade duration in frames for the transition.
    pub fade_frames: usize,
}

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

    /// Longest fade (in frames) this voice has been ducked with since it was last
    /// at full volume, so a restore reuses it instead of a fixed default (D25). Reset
    /// to 0 once a restore completes the return to full volume.
    duck_fade_frames: usize,

    /// Multiplier at the START of the current buffer, snapshotted by `advance_buffer`
    /// before the buffer's single advance. Together with `buffer_end_multiplier` it
    /// lets the mix loop interpolate the duck gain per frame across the buffer (D2)
    /// while the fade itself advances only once per buffer (D1).
    buffer_start_multiplier: f32,

    /// Multiplier at the END of the current buffer (== `current_multiplier` after the
    /// buffer's single advance). The per-frame value is lerped from
    /// `buffer_start_multiplier` toward this across the buffer's frames.
    buffer_end_multiplier: f32,
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
            duck_fade_frames: 0,
            buffer_start_multiplier: 1.0,
            buffer_end_multiplier: 1.0,
        }
    }

    /// Begin ducking to a new target volume
    fn begin_duck(&mut self, target_volume: f32, fade_duration_frames: usize) {
        self.start_multiplier = self.current_multiplier;
        self.target_volume = target_volume;
        self.fade_duration_frames = fade_duration_frames;
        self.fade_elapsed_frames = 0;
        self.is_ducking = true;
        // Remember the longest fade this voice has been ducked with, so the eventual
        // restore reuses it rather than a fixed default (D25).
        self.duck_fade_frames = self.duck_fade_frames.max(fade_duration_frames);
    }

    /// Begin restoring to full volume
    fn begin_restore(&mut self, fade_duration_frames: usize) {
        self.start_multiplier = self.current_multiplier;
        self.target_volume = 1.0;
        self.fade_duration_frames = fade_duration_frames;
        self.fade_elapsed_frames = 0;
        self.is_ducking = false;
        // The voice is heading back to full volume; the next duck cycle tracks its
        // own fade from scratch (D25).
        self.duck_fade_frames = 0;
    }

    /// Advance fade state by given number of frames and return current multiplier
    fn advance_and_get_multiplier(&mut self, frames: usize) -> f32 {
        // Advance fade state
        self.fade_elapsed_frames =
            (self.fade_elapsed_frames + frames).min(self.fade_duration_frames);

        // Calculate progress (0.0 to 1.0)
        let progress = if self.fade_duration_frames == 0 {
            1.0
        } else {
            self.fade_elapsed_frames as f32 / self.fade_duration_frames as f32
        };

        // Linear interpolation from start_multiplier to target_volume
        self.current_multiplier =
            self.start_multiplier + (self.target_volume - self.start_multiplier) * progress;

        self.current_multiplier
    }

    /// Snapshot the buffer-start multiplier and advance the fade by exactly one
    /// buffer's worth of frames, recording the resulting buffer-end multiplier (D1).
    /// The mix loop then reads a per-frame value interpolated between the two
    /// endpoints (D2) without advancing the fade again.
    fn advance_buffer(&mut self, frames: usize) {
        self.buffer_start_multiplier = self.current_multiplier;
        self.buffer_end_multiplier = self.advance_and_get_multiplier(frames);
    }

    /// The duck multiplier at frame `frame_idx` of a buffer `frames` long, linearly
    /// interpolated between this buffer's start and end multipliers (D2). With a
    /// single-frame buffer (or `frame_idx == 0`) this is the buffer-start value. The
    /// mix path reads the endpoints directly and lerps in `mixer::duck_frame_gain`;
    /// this mirrors that for the applier-level unit test.
    #[cfg(test)]
    fn frame_multiplier(&self, frame_idx: usize, frames: usize) -> f32 {
        if frames <= 1 {
            return self.buffer_start_multiplier;
        }
        let t = frame_idx as f32 / frames as f32;
        self.buffer_start_multiplier
            + (self.buffer_end_multiplier - self.buffer_start_multiplier) * t
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

    /// Duck states per voice. Retained for the reference ducking path the unit
    /// tests validate against; the audio thread keeps its own state in
    /// `DuckingApplier`, so the binary never reaches this field.
    #[allow(dead_code)]
    duck_states: HashMap<String, DuckState>,

    /// Last (target volume, fade frames) emitted per voice, so `compute_changes`
    /// only emits a change when the resolved target OR the fade length moves. Tracking
    /// the fade too means a same-target/faster-fade rule activating mid-fade still
    /// re-arms the applier with the shorter fade.
    last_changes: HashMap<String, (f32, usize)>,

    /// Longest fade (in frames) each voice has been ducked with since it was last
    /// at rest, so a restore can reuse that fade instead of a fixed default (D25).
    /// A voice ducked by several rules keeps the max, so it does not snap back over
    /// a fade shorter than the one that ducked it. Cleared when the voice restores.
    duck_fades: HashMap<String, usize>,
}

impl DuckingEngine {
    /// Create a new ducking engine with given rules and sample rate
    pub fn new(rules: Vec<DuckingRule>, sample_rate: u32) -> Self {
        tracing::info!(
            "Initializing ducking engine with {} rules at {} Hz",
            rules.len(),
            sample_rate
        );
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
            last_changes: HashMap::new(),
            duck_fades: HashMap::new(),
        }
    }

    /// Notify engine that a voice's activity state has changed, recomputing the
    /// internal duck states. This is the reference path the unit tests validate
    /// the control-side `compute_changes` + `DuckingApplier` against; the running
    /// system drives ducking through `compute_changes` instead.
    #[allow(dead_code)]
    pub fn notify_voice_active(&mut self, voice_id: &str, is_active: bool) {
        tracing::debug!("Voice '{}' activity changed: {}", voice_id, is_active);

        // Update active voices map. Inactive voices are removed rather than
        // stored as false, so one-shot auto voices do not accumulate forever.
        if is_active {
            self.active_voices.insert(voice_id.to_string(), true);
        } else {
            self.active_voices.remove(voice_id);
        }

        // Recalculate all duck states
        self.update_duck_states();
    }

    /// Get the ducking multiplier for a voice and advance its fade state, using
    /// the engine's own duck states. Retained as the reference path for tests;
    /// the audio thread uses `DuckingApplier::get_multiplier` at runtime.
    #[allow(dead_code)]

    /// Get the current ducking multiplier for a voice.
    /// This is called from the mixer callback for each sample.
    pub fn get_multiplier(&mut self, voice_id: &str, frames: usize) -> f32 {
        self.duck_states
            .get_mut(voice_id)
            .map(|s| s.advance_and_get_multiplier(frames))
            .unwrap_or(1.0)
    }

    pub fn advance(&mut self, frames: usize) {
        for state in self.duck_states.values_mut() {
            state.advance_and_get_multiplier(frames);
        }
    }

    /// Get ducking multiplier without advancing (for testing)
    #[cfg(test)]
    pub fn peek_multiplier(&self, voice_id: &str) -> f32 {
        self.duck_states
            .get(voice_id)
            .map(|s| s.get_multiplier())
            .unwrap_or(1.0)
    }

    /// Record a voice activity change and compute the resulting target changes,
    /// without owning any duck state. The control thread calls this and forwards
    /// each [`DuckTargetChange`] to the audio thread; only voices whose resolved
    /// target actually moved are emitted.
    pub fn compute_changes(&mut self, voice_id: &str, is_active: bool) -> Vec<DuckTargetChange> {
        self.active_voices.insert(voice_id.to_string(), is_active);

        // The default restore fade for a voice that was never ducked (D25 only
        // changes restore once a voice has actually been ducked).
        let default_restore = self.ms_to_frames(2000);
        // A voice never ducked is at rest: full volume with the default restore
        // fade. Comparing against that resting state (not a bare 1.0) means a
        // non-ducked voice does not spuriously emit just because its resolved
        // restore fade differs from a zero sentinel.
        let resting = (1.0, default_restore);

        let mut changes = Vec::new();
        for voice in self.potential_voices() {
            let (target, mut fade_frames) = self.resolve_target(&voice);
            if target < 1.0 {
                // Ducking: remember the longest fade this voice has been ducked
                // with, so its eventual restore can reuse it (D25).
                let longest = self
                    .duck_fades
                    .get(&voice)
                    .copied()
                    .map_or(fade_frames, |f| f.max(fade_frames));
                self.duck_fades.insert(voice.clone(), longest);
            } else {
                // Restoring: reuse the longest fade the voice was ducked with
                // instead of a fixed default, then forget it (the voice is at rest).
                fade_frames = self.duck_fades.remove(&voice).unwrap_or(default_restore);
            }
            let (last_target, last_fade) =
                self.last_changes.get(&voice).copied().unwrap_or(resting);
            if (target - last_target).abs() > f32::EPSILON || fade_frames != last_fade {
                self.last_changes
                    .insert(voice.clone(), (target, fade_frames));
                changes.push(DuckTargetChange {
                    voice,
                    target_volume: target,
                    fade_frames,
                });
            }
        }
        changes
    }

    /// Every voice that could be ducked or could trigger ducking across all rules.
    fn potential_voices(&self) -> HashSet<String> {
        let mut voices = HashSet::new();
        for rule in &self.rules {
            voices.insert(rule.primary_voice.clone());
            for ducked_voice in &rule.ducked_voices {
                voices.insert(ducked_voice.clone());
            }
        }
        voices
    }

    /// Resolve the target multiplier and fade length (in frames) for a voice
    /// given the current active set. No applicable rule means restore to full
    /// volume over the default restore time.
    fn resolve_target(&self, voice_id: &str) -> (f32, usize) {
        let applicable_rules = self.find_applicable_rules(voice_id);
        if applicable_rules.is_empty() {
            (1.0, self.ms_to_frames(2000))
        } else {
            let target = applicable_rules
                .iter()
                .map(|r| r.target_volume)
                .min_by(|a, b| a.total_cmp(b))
                .unwrap();
            let fade_duration_ms = applicable_rules
                .iter()
                .map(|r| r.fade_duration_ms)
                .min()
                .unwrap();
            (target, self.ms_to_frames(fade_duration_ms))
        }
    }

    /// Update all duck states based on current voice activity. Part of the
    /// reference ducking path exercised by the unit tests (see `notify_voice_active`).
    #[allow(dead_code)]
    fn update_duck_states(&mut self) {
        // Get all voices that might need ducking (any voice in any rule)
        let mut all_potential_voices: std::collections::HashSet<String> =
            std::collections::HashSet::new();

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
                let should_restore = self
                    .duck_states
                    .get(&voice_id)
                    .map(|s| s.target_volume < 1.0 || s.is_ducking)
                    .unwrap_or(false);

                if should_restore {
                    // Reuse the longest fade this voice was ducked with, falling back
                    // to the default restore time if it was never ducked (D25).
                    let default_restore = self.ms_to_frames(2000);
                    let state = self
                        .duck_states
                        .entry(voice_id.clone())
                        .or_insert_with(DuckState::new);
                    let fade_frames = if state.duck_fade_frames > 0 {
                        state.duck_fade_frames
                    } else {
                        default_restore
                    };
                    tracing::debug!(
                        "Voice '{}' restoring to full volume over {} frames",
                        voice_id,
                        fade_frames
                    );
                    state.begin_restore(fade_frames);
                }
            } else {
                // Multiple rules: use lowest target volume and fastest fade
                let target_volume = applicable_rules
                    .iter()
                    .map(|r| r.target_volume)
                    .min_by(|a, b| a.total_cmp(b))
                    .unwrap();

                let fade_duration_ms = applicable_rules
                    .iter()
                    .map(|r| r.fade_duration_ms)
                    .min()
                    .unwrap();

                let fade_frames = self.ms_to_frames(fade_duration_ms);
                let current_multiplier = self
                    .duck_states
                    .get(&voice_id)
                    .map(|s| s.current_multiplier)
                    .unwrap_or(1.0);

                // Whether this voice's ducking parameters actually moved. Gating the
                // re-arm on this means an unrelated voice toggling (which re-runs this
                // for every ducked voice) does not reset a settled/in-flight fade by
                // calling begin_duck with the same target and fade (D5).
                let params_changed = self
                    .duck_states
                    .get(&voice_id)
                    .map(|s| {
                        (s.target_volume - target_volume).abs() > 0.001
                            || s.fade_duration_frames != fade_frames
                            || !s.is_ducking
                    })
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

                let state = self
                    .duck_states
                    .entry(voice_id.clone())
                    .or_insert_with(DuckState::new);
                // Only (re-)arm the fade when the parameters changed; an unchanged
                // re-evaluation leaves the in-flight or settled fade untouched.
                if params_changed {
                    state.begin_duck(target_volume, fade_frames);
                }
            }
        }
    }

    /// Find all rules that apply to a given voice (should it be ducked?)
    fn find_applicable_rules(&self, voice_id: &str) -> Vec<&DuckingRule> {
        self.rules
            .iter()
            .filter(|rule| {
                // Rule applies if:
                // 1. This voice is in the ducked_voices list
                // 2. The primary voice has active samples
                rule.ducked_voices.iter().any(|v| v == voice_id)
                    && self
                        .active_voices
                        .get(&rule.primary_voice)
                        .copied()
                        .unwrap_or(false)
            })
            .collect()
    }

    /// Convert milliseconds to frames based on sample rate
    fn ms_to_frames(&self, ms: u32) -> usize {
        ((ms as f32 / 1000.0) * self.sample_rate as f32) as usize
    }
}

/// Applies pre-resolved ducking target changes and advances the per-voice fades.
/// This is the audio-thread half of ducking: it owns only the duck states and is
/// driven entirely by [`DuckTargetChange`]s computed on the control thread, so the
/// rule evaluation and active-voice bookkeeping stay off the real-time path.
#[derive(Default)]
pub struct DuckingApplier {
    /// Duck states per voice.
    duck_states: HashMap<String, DuckState>,
}

impl DuckingApplier {
    /// Create an applier with no ducked voices. Targets for voices it does not
    /// know are ignored (see [`apply_target`](Self::apply_target)); the running
    /// daemon uses [`with_ducked_voices`](Self::with_ducked_voices), so this is
    /// exercised only by the lib's unit tests and is dead in the bin target.
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an applier pre-populated with every voice the configured ducking
    /// rules can target (the union of the rules' `ducked_voices`). Pre-populating
    /// off-RT means [`apply_target`](Self::apply_target) never inserts — and so
    /// never allocates — on the audio thread, even for the FIRST duck of a voice.
    pub fn with_ducked_voices<I, S>(voices: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let duck_states: HashMap<String, DuckState> = voices
            .into_iter()
            .map(|v| (v.into(), DuckState::new()))
            .collect();
        Self { duck_states }
    }

    /// Apply a resolved target change, starting a duck or restore fade for the
    /// voice. Runs on the audio thread: `get_mut` only — the voice set was
    /// pre-populated at construction, so this never inserts or allocates. A voice
    /// outside that set cannot arrive here (the control-side engine only emits
    /// voices named by the configured rules); if one does, it is ignored.
    pub fn apply_target(&mut self, change: &DuckTargetChange) {
        let Some(state) = self.duck_states.get_mut(&change.voice) else {
            return;
        };
        if change.target_volume >= 1.0 {
            state.begin_restore(change.fade_frames);
        } else {
            state.begin_duck(change.target_volume, change.fade_frames);
        }
    }

    /// Advance every ducked voice's fade by exactly one buffer of `frames`, once
    /// (D1). Each voice's fade is advanced a single time regardless of how many
    /// samples or inputs reference it this buffer, snapshotting its buffer-start and
    /// buffer-end multipliers for the per-frame read below. Iterating the existing
    /// duck-state map allocates nothing, so this is callback-safe; the distinct-voice
    /// set is exactly the set of states already present.
    pub fn advance_buffer(&mut self, frames: usize) {
        for state in self.duck_states.values_mut() {
            state.advance_buffer(frames);
        }
    }

    /// The duck multiplier for `voice_id` at frame `frame_idx` of a buffer `frames`
    /// long, interpolated between the buffer endpoints snapshotted by
    /// [`advance_buffer`] (D2). Does not advance any fade, so it is safe to call for
    /// every sample/input/frame of the voice. An unducked voice reads 1.0.
    #[cfg(test)]
    pub fn frame_multiplier(&self, voice_id: &str, frame_idx: usize, frames: usize) -> f32 {
        self.duck_states
            .get(voice_id)
            .map(|s| s.frame_multiplier(frame_idx, frames))
            .unwrap_or(1.0)
    }

    /// The (buffer-start, buffer-end) duck multipliers for `voice_id`, snapshotted by
    /// the buffer's single [`advance_buffer`] (D1). The mix loop lerps between them
    /// per frame (D2). An unducked voice reads `(1.0, 1.0)`. Borrows immutably, so it
    /// can be read while a sample on the same state is mutably borrowed elsewhere.
    pub fn buffer_endpoints(&self, voice_id: &str) -> (f32, f32) {
        self.duck_states
            .get(voice_id)
            .map(|s| (s.buffer_start_multiplier, s.buffer_end_multiplier))
            .unwrap_or((1.0, 1.0))
    }

    /// Get the ducking multiplier for a voice and advance its fade state.
    /// Called by the reference/test paths; the mix path uses
    /// [`advance_buffer`] + [`buffer_endpoints`] so a shared voice advances once.
    #[allow(dead_code)]
    pub fn get_multiplier(&mut self, voice_id: &str, frames: usize) -> f32 {
        if let Some(state) = self.duck_states.get_mut(voice_id) {
            state.advance_and_get_multiplier(frames)
        } else {
            1.0
        }
    }

    /// Get the ducking multiplier without advancing (for testing).
    #[cfg(test)]
    fn peek_multiplier(&self, voice_id: &str) -> f32 {
        self.duck_states
            .get(voice_id)
            .map(|s| s.get_multiplier())
            .unwrap_or(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_rule(
        primary: &str,
        ducked: Vec<&str>,
        target: f32,
        fade_ms: u32,
    ) -> DuckingRule {
        DuckingRule {
            primary_voice: primary.to_string(),
            ducked_voices: ducked.iter().map(|s| s.to_string()).collect(),
            target_volume: target,
            fade_duration_ms: fade_ms,
        }
    }

    #[test]
    fn nan_target_volume_does_not_panic() {
        // Two active primaries duck the same voice, forcing the min-by comparison
        // across a NaN target. A NaN-unsafe comparator panics here.
        let rules = vec![
            create_test_rule("narration", vec!["music"], f32::NAN, 1000),
            create_test_rule("dialog", vec!["music"], 0.5, 1000),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);
        engine.notify_voice_active("narration", true);
        engine.notify_voice_active("dialog", true);
        let _ = engine.get_multiplier("music", 1);
    }

    #[test]
    fn test_ducking_engine_creation() {
        let rules = vec![create_test_rule("narration", vec!["music"], 0.1, 2000)];
        let engine = DuckingEngine::new(rules, 48000);
        assert_eq!(engine.rules.len(), 1);
        assert_eq!(engine.sample_rate, 48000);
    }

    #[test]
    fn test_single_rule_ducking() {
        let rules = vec![create_test_rule("narration", vec!["music"], 0.1, 1000)];
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
        assert!(
            multiplier > 0.1 && multiplier < 1.0,
            "Expected multiplier between 0.1 and 1.0, got {}",
            multiplier
        );

        // Stop narration - music should restore
        engine.notify_voice_active("narration", false);

        // Advance through restore
        engine.advance(half_fade_frames);

        // Should be restoring toward 1.0
        let restored = engine.peek_multiplier("music");
        assert!(
            restored > multiplier,
            "Expected restoration, got {}",
            restored
        );
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
        assert_eq!(
            engine
                .duck_states
                .get("music")
                .unwrap()
                .fade_duration_frames,
            expected_frames
        );
    }

    #[test]
    fn test_no_ducking_when_primary_inactive() {
        let rules = vec![create_test_rule("narration", vec!["music"], 0.1, 1000)];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Start music (not narration)
        engine.notify_voice_active("music", true);

        // Music should not be ducked
        assert_eq!(engine.peek_multiplier("music"), 1.0);
    }

    #[test]
    fn test_ducking_multiple_voices() {
        let rules = vec![create_test_rule(
            "narration",
            vec!["music", "ambience", "effects"],
            0.1,
            1000,
        )];
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
        let rules = vec![create_test_rule("narration", vec!["music"], 0.1, 1000)];
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
        let rules = vec![create_test_rule("narration", vec!["music"], 0.1, 0)];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Start narration
        engine.notify_voice_active("narration", true);

        // With 0ms fade, should immediately be at target
        let multiplier = engine.get_multiplier("music", 1);
        assert!(
            (multiplier - 0.1).abs() < 0.01,
            "Expected ~0.1, got {}",
            multiplier
        );
    }

    #[test]
    fn test_fade_progress() {
        let rules = vec![create_test_rule("narration", vec!["music"], 0.2, 1000)];
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
    fn unrelated_voice_toggle_does_not_reset_a_settled_ducked_voice() {
        // D5: toggling a voice that does not change "music"'s ducking parameters
        // must not re-arm "music"'s fade. Previously update_duck_states called
        // begin_duck on every still-ducked voice on any toggle, resetting
        // fade_elapsed_frames and start_multiplier and re-stretching a settled fade.
        let rules = vec![create_test_rule("narration", vec!["music"], 0.1, 1000)];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Narration ducks music; advance the full fade so it settles at the target.
        engine.notify_voice_active("narration", true);
        engine.get_multiplier("music", 48000);
        let settled = engine.peek_multiplier("music");
        assert!((settled - 0.1).abs() < 1e-3, "music should settle at 0.1");
        let elapsed_before = engine.duck_states.get("music").unwrap().fade_elapsed_frames;
        assert_eq!(elapsed_before, 48000, "the fade should be fully elapsed");

        // Toggle an UNRELATED voice (not in any rule). This must not touch music's
        // fade: same target, same fade -> begin_duck must be gated out.
        engine.notify_voice_active("unrelated", true);

        let state = engine.duck_states.get("music").unwrap();
        assert_eq!(
            state.fade_elapsed_frames, elapsed_before,
            "an unrelated toggle must not reset music's elapsed fade"
        );
        assert!(
            (state.start_multiplier - settled).abs() < 1e-6
                || state.fade_elapsed_frames == elapsed_before,
            "an unrelated toggle must not re-arm music's fade"
        );
        assert!(
            (engine.peek_multiplier("music") - settled).abs() < 1e-6,
            "music's multiplier must be unchanged by the unrelated toggle"
        );
    }

    #[test]
    fn test_primary_voice_not_ducked() {
        let rules = vec![create_test_rule("narration", vec!["music"], 0.1, 1000)];
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

    #[test]
    fn advance_buffer_advances_each_voice_once_regardless_of_reads() {
        // D1: a voice shared by several samples must have its fade advanced exactly
        // once per buffer. advance_buffer advances once; frame_multiplier only reads,
        // so two "samples" reading the same voice see the SAME per-frame multiplier
        // and the fade does not race ahead.
        let mut applier = DuckingApplier::with_ducked_voices(["music"]);
        applier.apply_target(&DuckTargetChange {
            voice: "music".to_string(),
            target_volume: 0.0,
            fade_frames: 1000,
        });

        // One buffer of 100 frames: the fade advances 100/1000 of the way.
        applier.advance_buffer(100);
        let state = applier.duck_states.get("music").unwrap();
        assert_eq!(
            state.fade_elapsed_frames, 100,
            "the fade must advance by exactly one buffer, not once per sample"
        );

        // Two samples reading frame 50 of this buffer see the identical multiplier.
        let m_sample1 = applier.frame_multiplier("music", 50, 100);
        let m_sample2 = applier.frame_multiplier("music", 50, 100);
        assert_eq!(
            m_sample1, m_sample2,
            "every sample on the voice must see the same multiplier this buffer"
        );

        // Reading does not advance the fade.
        assert_eq!(
            applier
                .duck_states
                .get("music")
                .unwrap()
                .fade_elapsed_frames,
            100,
            "frame_multiplier must not advance the fade"
        );

        // The per-frame value interpolates start->end across the buffer: frame 0 is
        // the buffer start (1.0 here) and the last frame approaches the buffer end.
        let start = applier.frame_multiplier("music", 0, 100);
        let near_end = applier.frame_multiplier("music", 99, 100);
        assert!(
            (start - 1.0).abs() < 1e-6,
            "frame 0 should be the buffer-start multiplier (1.0), got {start}"
        );
        assert!(
            near_end < start,
            "the multiplier should fall across the buffer ({near_end} !< {start})"
        );

        // A second buffer advances exactly one more buffer (total 200/1000).
        applier.advance_buffer(100);
        assert_eq!(
            applier
                .duck_states
                .get("music")
                .unwrap()
                .fade_elapsed_frames,
            200
        );
    }

    #[test]
    fn frame_multiplier_is_one_for_an_unducked_voice() {
        let applier = DuckingApplier::new();
        assert_eq!(applier.frame_multiplier("anything", 0, 512), 1.0);
        assert_eq!(applier.frame_multiplier("anything", 100, 512), 1.0);
    }

    #[test]
    fn applier_ducks_then_restores() {
        let mut applier = DuckingApplier::with_ducked_voices(["music"]);

        // Duck "music" to 0.2 over 1000ms (48000 frames).
        applier.apply_target(&DuckTargetChange {
            voice: "music".to_string(),
            target_volume: 0.2,
            fade_frames: 48000,
        });
        applier.get_multiplier("music", 48000);
        assert!(
            (applier.peek_multiplier("music") - 0.2).abs() < 0.01,
            "expected ducked to ~0.2, got {}",
            applier.peek_multiplier("music")
        );

        // Restore over 2000ms (96000 frames).
        applier.apply_target(&DuckTargetChange {
            voice: "music".to_string(),
            target_volume: 1.0,
            fade_frames: 96000,
        });
        applier.get_multiplier("music", 96000);
        assert!(
            (applier.peek_multiplier("music") - 1.0).abs() < 0.01,
            "expected restored to ~1.0, got {}",
            applier.peek_multiplier("music")
        );
    }

    #[test]
    fn compute_changes_only_emits_on_target_move() {
        let rules = vec![create_test_rule("narration", vec!["music"], 0.1, 1000)];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Primary becomes active: "music" moves from 1.0 to 0.1, so one change.
        let changes = engine.compute_changes("narration", true);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].voice, "music");
        assert!((changes[0].target_volume - 0.1).abs() < 1e-6);

        // Re-asserting the same activity does not move any target: no changes.
        let again = engine.compute_changes("narration", true);
        assert!(
            again.is_empty(),
            "expected no changes when targets are unchanged, got {:?}",
            again
        );

        // Primary goes inactive: "music" restores to 1.0, so one change again.
        let restore = engine.compute_changes("narration", false);
        assert_eq!(restore.len(), 1);
        assert_eq!(restore[0].voice, "music");
        assert!((restore[0].target_volume - 1.0).abs() < 1e-6);
    }

    #[test]
    fn restore_uses_the_triggering_rules_fade_not_a_fixed_default() {
        // D25/D3: a 200ms rule must restore over ~200ms, not the old hardcoded
        // 2000ms. The restore fade is the same length the rule used to duck.
        let rules = vec![create_test_rule("narration", vec!["music"], 0.1, 200)];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Duck: the emitted change carries the rule's 200ms fade (9600 frames).
        let duck = engine.compute_changes("narration", true);
        assert_eq!(duck.len(), 1);
        assert_eq!(duck[0].fade_frames, engine.ms_to_frames(200));

        // Release: the restore must reuse the rule's 200ms fade, NOT 2000ms.
        let restore = engine.compute_changes("narration", false);
        assert_eq!(restore.len(), 1);
        assert_eq!(restore[0].voice, "music");
        assert!((restore[0].target_volume - 1.0).abs() < 1e-6);
        assert_eq!(
            restore[0].fade_frames,
            engine.ms_to_frames(200),
            "restore must honor the rule's fade_duration_ms (200ms), not 2000ms"
        );
    }

    #[test]
    fn restore_uses_the_max_fade_across_rules_that_ducked_the_voice() {
        // When several rules duck a voice, the restore uses the longest of their
        // fades (the slowest), so a voice that was ducked over a long fade does not
        // snap back over a short one. Ducking still uses the fastest fade.
        let rules = vec![
            create_test_rule("narration", vec!["music"], 0.2, 2000),
            create_test_rule("dialog", vec!["music"], 0.2, 500),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Narration ducks music over its 2000ms fade.
        let first = engine.compute_changes("narration", true);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].fade_frames, engine.ms_to_frames(2000));

        // Dialog activates: same target, but the fastest applicable fade is now
        // 500ms, so a change is emitted that accelerates the duck.
        let second = engine.compute_changes("dialog", true);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].fade_frames, engine.ms_to_frames(500));

        // Both go inactive; the restore uses the longest fade the voice was ducked
        // with (2000ms), not the last (fastest) duck fade or a fixed default.
        engine.compute_changes("narration", false);
        let restore = engine.compute_changes("dialog", false);
        assert_eq!(restore.len(), 1);
        assert_eq!(restore[0].voice, "music");
        assert_eq!(
            restore[0].fade_frames,
            engine.ms_to_frames(2000),
            "restore must use the longest fade the voice was ducked with"
        );
    }

    #[test]
    fn compute_changes_emits_on_faster_fade_same_target() {
        // Two rules duck "music" to the SAME target but with different fade times.
        // When the faster-fading primary activates mid-fade, the resolved target is
        // unchanged but the fade accelerates; compute_changes must emit so the
        // applier re-arms with the shorter fade (the old engine always re-armed).
        let rules = vec![
            create_test_rule("narration", vec!["music"], 0.2, 2000),
            create_test_rule("dialog", vec!["music"], 0.2, 500),
        ];
        let mut engine = DuckingEngine::new(rules, 48000);

        // Narration starts: music ducks to 0.2 over 2000ms (96000 frames).
        let first = engine.compute_changes("narration", true);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].voice, "music");
        assert!((first[0].target_volume - 0.2).abs() < 1e-6);
        assert_eq!(first[0].fade_frames, 96000);

        // Dialog starts: same 0.2 target, but the fastest applicable fade is now
        // 500ms (24000 frames). The target did not move, yet a change MUST be
        // emitted to accelerate the fade.
        let second = engine.compute_changes("dialog", true);
        assert_eq!(
            second.len(),
            1,
            "a faster fade to the same target must emit a change, got {:?}",
            second
        );
        assert_eq!(second[0].voice, "music");
        assert!((second[0].target_volume - 0.2).abs() < 1e-6);
        assert_eq!(
            second[0].fade_frames, 24000,
            "the emitted change must carry the faster fade"
        );

        // Re-asserting dialog's activity changes nothing (same target, same fade).
        let again = engine.compute_changes("dialog", true);
        assert!(
            again.is_empty(),
            "no change expected when target and fade are unchanged, got {:?}",
            again
        );
    }

    #[test]
    fn applier_matches_legacy_engine() {
        // Drive the control-side compute_changes + applier and assert the
        // multiplier tracks a legacy engine.notify_voice_active + get_multiplier.
        let rule = create_test_rule("narration", vec!["music"], 0.3, 1000);

        let mut legacy = DuckingEngine::new(vec![rule.clone()], 48000);
        let mut control = DuckingEngine::new(vec![rule], 48000);
        let mut applier = DuckingApplier::with_ducked_voices(["music"]);

        // Activate the primary on both paths.
        legacy.notify_voice_active("narration", true);
        for change in control.compute_changes("narration", true) {
            applier.apply_target(&change);
        }

        // Advance both in identical steps and compare the running multiplier.
        for _ in 0..5 {
            let l = legacy.get_multiplier("music", 9600);
            let a = applier.get_multiplier("music", 9600);
            assert!(
                (l - a).abs() < 1e-4,
                "applier {a} diverged from legacy engine {l}"
            );
        }

        // Deactivate and compare through the restore.
        legacy.notify_voice_active("narration", false);
        for change in control.compute_changes("narration", false) {
            applier.apply_target(&change);
        }
        for _ in 0..5 {
            let l = legacy.get_multiplier("music", 9600);
            let a = applier.get_multiplier("music", 9600);
            assert!(
                (l - a).abs() < 1e-4,
                "applier {a} diverged from legacy engine {l} during restore"
            );
        }
    }
}
