# Followup Implementation Tasks

These prompts can be used with Claude Code to implement features that are documented but not yet implemented.

---

## Task 1: Implement Loop Playback

### Prompt

```
Implement the `loop` parameter for the play command in mqttaudio.

The documentation in docs/commands.md, docs/features/voice-management.md, and
docs/getting-started.md describes this feature - audio should loop continuously
until stopped.

Implementation requirements:

1. Uncomment and enable the `loop` parameter in src/mqtt/commands.rs (currently
   commented out as "Future" around line 86-87)

2. Update the PlayMessage struct to include the loop field

3. Pass the loop parameter through to the ActiveSample when creating it in main.rs

4. Modify src/audio/mixer.rs to handle looping:
   - When a sample reaches the end of its buffer, check if loop_mode is true
   - If looping, reset position to 0 instead of marking as finished
   - Ensure fade_out still works correctly even when looping

5. Write unit tests for:
   - Parsing play command with loop: true
   - Parsing play command with loop: false (or omitted)
   - Mixer correctly loops audio (resets position at end)
   - Mixer correctly stops looped audio on stopall/voice_stop

6. Test manually with:
   - Play a short audio file with loop: true, verify it repeats
   - Use voice_fade_out to fade out a looping track
   - Use stopall to stop all looping tracks

The feature should match the documented behavior in docs/commands.md.
```

---

## Task 2: Implement Memory Cache Limit

### Prompt

```
Implement a configurable memory cache limit for mqttaudio.

The memory cache stores decoded PCM audio for instant playback. Currently there's
no limit, which could cause memory exhaustion with many cached files.

Implementation requirements:

1. Add `max_memory_mb` field to CacheConfig in src/config.rs:
   - Default value: 500 (MB)
   - Valid range: 50-4096 MB
   - Add validation in Config::validate()

2. Update src/cache/memory.rs to enforce the limit:
   - Track total memory usage of all cached buffers
   - Calculate size as: frames * channels * 4 bytes (f32)
   - When adding a new entry, if total would exceed limit:
     - Evict least-recently-used entries until there's room
     - Use an LRU tracking mechanism (timestamp or linked list)
   - Log when evictions occur (at debug level)

3. Add a method to report current memory cache usage for diagnostics

4. Update documentation in docs/features/caching.md and docs/configuration.md
   to document the new config option

5. Write unit tests for:
   - Config parsing with max_memory_mb
   - Config validation for valid/invalid values
   - Cache eviction when limit exceeded
   - LRU ordering (oldest entries evicted first)

6. Test manually:
   - Set max_memory_mb: 50 (small limit)
   - Precache several large audio files
   - Verify older entries are evicted when limit is reached
   - Check logs show eviction messages

The decoded buffer size calculation:
- 1 minute of stereo 48kHz audio = 48000 * 60 * 2 * 4 = ~23 MB
- 30 seconds of mono 48kHz = 48000 * 30 * 1 * 4 = ~5.5 MB
```

---

## Notes

Both tasks follow test-driven development as specified in CLAUDE.md:
1. Write failing test first
2. Implement minimal code to pass
3. Refactor if needed
