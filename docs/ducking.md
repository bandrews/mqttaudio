# Audio Ducking Design

## Overview

Audio ducking automatically reduces the volume of certain voices when other voices are playing. This is commonly used to make narration or dialog clearly audible over background music and sound effects.

## Requirements

### Core Features
1. **Voice-level ducking**: Apply ducking to all samples in a voice
2. **Configurable fade duration**: Smooth fade down/up over time
3. **Config-based rules**: Define ducking rules in configuration file
4. **Multiple rules support**: When multiple rules apply, use lowest volume and fastest fade
5. **Clean architecture**: Separate ducking engine from mixer/playback flows
6. **Observation-based**: Ducking engine observes playback without tight coupling

### Deferred Features (Future)
- MQTT commands to configure ducking at runtime (start with config file only)
- Optional delay of primary voice playback until duck completes (not needed for MVP)

## Architecture

### Components

```
┌──────────────────────────────────────────────────────────────┐
│ Configuration File                                           │
│ {                                                            │
│   "ducking_rules": [                                         │
│     {                                                        │
│       "primary_voice": "narration",                          │
│       "ducked_voices": ["music", "ambience"],                │
│       "target_volume": 0.1,                                  │
│       "fade_duration_ms": 2000                               │
│     }                                                        │
│   ]                                                          │
│ }                                                            │
└─────────────────────────┬────────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────────────┐
│ Audio Engine (owns DuckingEngine)                           │
│  - Notifies ducking engine of voice state changes            │
│  - Queries ducking multipliers for voices                    │
└─────────────────────────┬────────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────────────┐
│ DuckingEngine                                                │
│  - Stores ducking rules                                      │
│  - Tracks which voices are active                            │
│  - Calculates duck states per voice                          │
│  - Returns multiplier for each voice (0.0 - 1.0)            │
└─────────────────────────┬────────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────────────┐
│ Mixer (Audio Callback)                                       │
│  - Calls ducking_engine.get_multiplier(voice_id)             │
│  - Applies: sample_vol * voice_vol * ducking_multiplier      │
└──────────────────────────────────────────────────────────────┘
```

### Data Flow

1. **Configuration Load**: Parse ducking rules from config file
2. **Voice Activity Changes**:
   - When samples start/stop in a voice, notify ducking engine
   - Ducking engine updates internal state
3. **Mixer Query**:
   - For each sample, mixer asks: `ducking_engine.get_multiplier(voice_id)`
   - Returns current ducking multiplier (includes smooth fade state)
4. **Volume Application**:
   - Final volume = `sample.volume * voice.volume * ducking_multiplier`

## Data Structures

### Configuration

```rust
/// Ducking rule from configuration
#[derive(Debug, Clone, Deserialize)]
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
```

### Runtime State

```rust
/// State of ducking for a single voice
#[derive(Debug, Clone)]
pub struct DuckState {
    /// Target volume to duck to (from applicable rules)
    target_volume: f32,

    /// Current volume multiplier (smooth fade in progress)
    current_multiplier: f32,

    /// Fade duration in frames
    fade_duration_frames: usize,

    /// Frames elapsed in current fade
    fade_elapsed_frames: usize,

    /// Is currently ducking (vs. restoring)
    is_ducking: bool,
}

/// Ducking engine state
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
```

## Core Algorithms

### 1. Determining Duck State

When a voice's activity changes (samples start/stop):

```rust
fn update_duck_states(&mut self) {
    // For each voice, determine if it should be ducked
    for voice_id in all_known_voices {
        let applicable_rules = self.find_applicable_rules(voice_id);

        if applicable_rules.is_empty() {
            // No ducking needed - restore to full volume
            self.begin_restore(voice_id);
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

            self.begin_duck(voice_id, target_volume, fade_duration_ms);
        }
    }
}

fn find_applicable_rules(&self, voice_id: &str) -> Vec<&DuckingRule> {
    self.rules.iter()
        .filter(|rule| {
            // Rule applies if:
            // 1. This voice is in the ducked_voices list
            // 2. The primary voice has active samples
            rule.ducked_voices.contains(&voice_id.to_string()) &&
            self.active_voices.get(&rule.primary_voice).copied().unwrap_or(false)
        })
        .collect()
}
```

### 2. Smooth Fade Calculation

```rust
fn get_multiplier(&mut self, voice_id: &str, frames: usize) -> f32 {
    let state = self.duck_states.get_mut(voice_id)?;

    // Advance fade state
    state.fade_elapsed_frames = (state.fade_elapsed_frames + frames)
        .min(state.fade_duration_frames);

    let progress = if state.fade_duration_frames == 0 {
        1.0
    } else {
        state.fade_elapsed_frames as f32 / state.fade_duration_frames as f32
    };

    state.current_multiplier = if state.is_ducking {
        // Fade from 1.0 to target_volume
        1.0 - (progress * (1.0 - state.target_volume))
    } else {
        // Restore from target_volume to 1.0
        state.target_volume + (progress * (1.0 - state.target_volume))
    };

    state.current_multiplier
}
```

### 3. Integration Points

**Audio Engine Integration:**
```rust
impl AudioEngine {
    fn handle_play_command(&mut self, ...) {
        // ... existing play logic ...

        // Notify ducking engine
        self.ducking_engine.notify_voice_active(&voice_id, true);
    }

    fn cleanup_completed_samples(&mut self) {
        // ... existing cleanup ...

        // Update ducking for voices that now have no samples
        for voice in voices_now_empty {
            self.ducking_engine.notify_voice_active(&voice, false);
        }
    }
}
```

**Mixer Integration:**
```rust
fn mix_sample_into_output(...) {
    let ducking_multiplier = state.ducking_engine.get_multiplier(&sample.voice_id);
    let final_volume = sample.volume * sample.voice_volume * ducking_multiplier;
    // ... rest of mixing ...
}
```

## Configuration Example

```json
{
  "ducking_rules": [
    {
      "primary_voice": "narration",
      "ducked_voices": ["music", "ambience"],
      "target_volume": 0.1,
      "fade_duration_ms": 2000
    },
    {
      "primary_voice": "dialog",
      "ducked_voices": ["music", "ambience", "effects"],
      "target_volume": 0.15,
      "fade_duration_ms": 1500
    },
    {
      "primary_voice": "alert",
      "ducked_voices": ["music", "ambience", "effects", "narration"],
      "target_volume": 0.05,
      "fade_duration_ms": 500
    }
  ]
}
```

## Example Scenarios

### Scenario 1: Simple Ducking

```
Time  | music    | narration | music_volume | narration_volume
------|----------|-----------|--------------|------------------
0s    | playing  | -         | 1.0          | -
1s    | playing  | starts    | 1.0 → 0.1    | 1.0
3s    | playing  | playing   | 0.1          | 1.0
5s    | playing  | ends      | 0.1 → 1.0    | -
7s    | playing  | -         | 1.0          | -
```

### Scenario 2: Multiple Rules

Config:
- Rule A: narration ducks music to 0.2 over 1000ms
- Rule B: dialog ducks music to 0.1 over 500ms

```
Time  | music    | narration | dialog  | music_volume | rule applied
------|----------|-----------|---------|--------------|-------------
0s    | playing  | -         | -       | 1.0          | none
1s    | playing  | starts    | -       | 1.0 → 0.2    | A
2s    | playing  | playing   | starts  | 0.2 → 0.1    | A + B (use B: lower, faster)
3s    | playing  | playing   | playing | 0.1          | A + B
4s    | playing  | ends      | playing | 0.1          | B only
5s    | playing  | -         | playing | 0.1          | B only
6s    | playing  | -         | ends    | 0.1 → 1.0    | none
```

## Performance Considerations

### Audio Callback Impact

The ducking multiplier calculation must be very fast since it runs in the audio callback:

1. **Pre-computed states**: All fade calculations done in advance
2. **Simple lookup**: `get_multiplier()` is just a HashMap lookup and simple math
3. **No allocations**: All data structures pre-allocated
4. **No blocking**: Pure computation, no I/O or locks

### Expected Overhead

- Per-sample cost: 1 HashMap lookup + 3-4 float multiplications
- Typical overhead: < 1% of callback time (well within budget)

## Testing Strategy

### Unit Tests

1. **Rule matching**
   - Single rule applies correctly
   - Multiple rules select lowest volume and fastest fade
   - Rules don't apply when primary voice inactive
   - Rules correctly identify ducked voices

2. **Fade calculation**
   - Smooth fade from 1.0 to target over duration
   - Smooth restore from target to 1.0
   - Zero-duration fades work (instant)
   - Very long fades don't overflow

3. **Voice activity tracking**
   - Notify active correctly triggers ducking
   - Notify inactive correctly triggers restore
   - Multiple samples in same voice handled correctly
   - Edge cases (notify same state twice, etc.)

4. **Multiple simultaneous ducks**
   - Voice ducked by multiple primaries uses lowest target
   - Voice ducked by multiple primaries uses fastest fade
   - Transitioning from one rule to another is smooth

5. **Integration with mixer**
   - Ducking multiplier applied correctly to volume
   - Works with voice volume
   - Works with sample volume
   - Works with fade in/out

### Integration Tests

1. **End-to-end ducking**
   - Play music, start narration, verify music ducks
   - Verify smooth fade over correct duration
   - Verify restore when narration ends

2. **Multiple voices**
   - Multiple ducked voices respond to single primary
   - Multiple primaries duck overlapping sets
   - Complex scenarios with 3+ voices

3. **Config loading**
   - Valid configs load correctly
   - Invalid configs rejected with clear errors
   - Missing ducking_rules field is acceptable (no ducking)

### Performance Tests

1. **Callback timing**
   - Ducking adds < 1% overhead to mixer
   - No allocations in callback
   - Scales linearly with sample count

## Migration Path

### Phase 1: Core Implementation (This PR)
- Implement DuckingEngine
- Integrate with mixer and audio engine
- Config file support
- Comprehensive tests

### Phase 2: Future Enhancements
- MQTT commands to configure ducking at runtime
- Optional delay of primary sounds until duck completes
- Advanced ducking curves (not just linear)
- Per-rule priority overrides

## Edge Cases

1. **Voice doesn't exist yet**: No problem, ducking state created on demand
2. **Rule references non-existent voice**: Ignored (rule won't match)
3. **Circular ducking**: A ducks B ducks A - Both active = both ducked (undefined but safe)
4. **Overlapping primaries**: Multiple primaries trigger same duck - uses most aggressive settings
5. **Sample rate change**: Ducking engine recreated with new sample rate
6. **Very short fades**: Handled correctly (instant when duration = 0)

## Implementation Checklist

- [ ] Create `src/audio/ducking.rs` module
- [ ] Define data structures (DuckingRule, DuckState, DuckingEngine)
- [ ] Implement rule matching logic
- [ ] Implement smooth fade calculations
- [ ] Add ducking_rules to config.rs
- [ ] Integrate with audio engine (notifications)
- [ ] Integrate with mixer (multiplier queries)
- [ ] Write unit tests (15+ tests)
- [ ] Write integration tests
- [ ] Create demo script with tones
- [ ] Update documentation (README, commands.md, configuration.md)
- [ ] Performance testing and validation

## Open Questions

None - design is clear and implementation is straightforward.
