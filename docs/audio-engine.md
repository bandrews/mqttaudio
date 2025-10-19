# Audio Engine Design

## Purpose

The audio engine is the core component that manages audio playback, mixing, and routing. It consists of two main parts:
1. **Audio Engine Coordinator** - Manages state, sample loading, voice management
2. **Audio Callback (Mixer)** - Real-time mixing and output

## Audio Callback (Real-Time Mixer)

### Critical Constraints

**THE AUDIO CALLBACK MUST NEVER:**
- Allocate memory
- Block on I/O
- Acquire locks (except very brief lock-free operations)
- Call syscalls
- Do expensive computation

**WHY:** The audio callback runs on a high-priority thread and must complete within the buffer duration (typically 5-20ms). If it doesn't finish in time, you get buffer underruns = glitches/dropouts.

### Callback Execution Flow

```rust
fn audio_callback(output_buffer: &mut [f32], state: &MixerState) {
    // 1. Zero the output buffer
    output_buffer.fill(0.0);

    // 2. For each active sample
    for sample in state.active_samples.iter() {
        // 3. Read audio data from pre-decoded buffer
        let frames_to_read = output_buffer.len() / state.output_channels;
        let audio_data = sample.buffer.read(sample.position, frames_to_read);

        // 4. Apply fade envelope (if active)
        let fade_multiplier = calculate_fade(sample.fade_state, frames_to_read);

        // 5. Apply volumes (sample volume * voice volume)
        let volume = sample.volume * sample.voice.volume;

        // 6. Map channels and mix into output
        for (frame_idx, frame) in audio_data.chunks(sample.channels).enumerate() {
            for (src_channel, dest_channel) in sample.channel_map.iter() {
                let sample_value = frame[*src_channel];
                let mixed_value = sample_value * volume * fade_multiplier[frame_idx];

                let output_idx = frame_idx * state.output_channels + dest_channel;
                output_buffer[output_idx] += mixed_value;
            }
        }

        // 7. Advance playback position
        sample.position += frames_to_read;
    }

    // 8. Apply per-channel calibration volumes
    for (channel_idx, calibration) in state.channel_calibrations.iter() {
        for frame_idx in 0..(output_buffer.len() / state.output_channels) {
            let idx = frame_idx * state.output_channels + channel_idx;
            output_buffer[idx] *= calibration;
        }
    }

    // 9. Clamp to prevent clipping
    for sample in output_buffer.iter_mut() {
        *sample = sample.clamp(-1.0, 1.0);
    }
}
```

### MixerState Structure

```rust
struct MixerState {
    // List of currently playing samples
    active_samples: Vec<ActiveSample>,

    // Output device configuration
    output_channels: usize,
    sample_rate: u32,

    // Per-channel calibration volumes
    channel_calibrations: Vec<f32>,  // Indexed by channel number
}

struct ActiveSample {
    // Pre-decoded audio buffer (shared, immutable)
    buffer: Arc<DecodedBuffer>,

    // Current playback position (frames)
    position: usize,

    // Number of channels in source audio
    channels: usize,

    // Channel routing: vec![(src_chan, dest_chan), ...]
    channel_map: Vec<(usize, usize)>,

    // Per-sample volume (0.0 - 1.0)
    volume: f32,

    // Reference to parent voice (for voice-level volume)
    voice_volume: f32,  // Cached from voice to avoid indirection

    // Fade state
    fade_state: FadeState,

    // Loop configuration
    loop_mode: LoopMode,
    max_frames: Option<usize>,  // For maxPlayLength

    // Unique ID for removal
    id: u64,
}

enum LoopMode {
    Once,
    Infinite,
}

struct FadeState {
    kind: FadeKind,
    current_frame: usize,
    total_frames: usize,
}

enum FadeKind {
    None,
    In,   // 0.0 → 1.0
    Out,  // 1.0 → 0.0
}
```

### Fade Envelope Calculation

Simple linear fade:
```rust
fn calculate_fade(fade: &FadeState, frame_count: usize) -> Vec<f32> {
    match fade.kind {
        FadeKind::None => vec![1.0; frame_count],

        FadeKind::In => {
            (0..frame_count).map(|i| {
                let frame = fade.current_frame + i;
                if frame >= fade.total_frames {
                    1.0
                } else {
                    (frame as f32) / (fade.total_frames as f32)
                }
            }).collect()
        }

        FadeKind::Out => {
            (0..frame_count).map(|i| {
                let frame = fade.current_frame + i;
                if frame >= fade.total_frames {
                    0.0
                } else {
                    1.0 - ((frame as f32) / (fade.total_frames as f32))
                }
            }).collect()
        }
    }
}
```

**NOTE:** Allocating a Vec in the callback is bad! This is pseudocode. Real implementation should:
- Use a scratch buffer allocated once
- Or compute fade per-sample inline
- Or use a lookup table for common fade lengths

### Channel Mapping

```rust
// Example: 4-channel source to 4 arbitrary output channels
// Source: [FL, FR, RL, RR]
// Output device has 10 channels [0-9]
// Desired mapping: FL→6, FR→7, RL→8, RR→9

let channel_map = vec![
    (0, 6),  // Source channel 0 → Output channel 6
    (1, 7),  // Source channel 1 → Output channel 7
    (2, 8),  // etc.
    (3, 9),
];

// During mixing:
for (src_idx, dest_idx) in channel_map.iter() {
    let src_sample = source_frame[*src_idx];
    output_buffer[frame_offset + dest_idx] += src_sample * volume;
}
```

### Handling Edge Cases

**Sample finishes playing:**
- When `position >= buffer.frames`:
  - If looping: `position = 0`, continue
  - If not looping: Mark sample as complete, remove on next callback

**Buffer underrun (shouldn't happen):**
- If `position + frames_to_read > buffer.frames`:
  - Read what's available
  - Fill rest with zeros
  - Log warning

**Voice is fading out:**
- Continue mixing with decreasing volume
- When fade completes, mark samples for removal

**Too many samples (performance):**
- Set a maximum (e.g., 128 simultaneous samples)
- Reject new play commands or remove oldest

## Audio Engine Coordinator

### Responsibilities

1. **Sample Loading**
   - Check memory cache
   - Check disk cache
   - Spawn decode workers
   - Store decoded buffers

2. **Voice Management**
   - Create/destroy voices
   - Track samples per voice
   - Update voice volumes
   - Trigger fades

3. **Mixer State Updates**
   - Add samples to active list
   - Remove completed samples
   - Update volumes, fades

4. **Cache Management**
   - Disk cache CRUD
   - Background validation
   - Eviction policy

### State Structure

```rust
struct AudioEngine {
    // Memory cache: URL → decoded audio
    sample_cache: HashMap<String, Arc<DecodedBuffer>>,

    // Disk cache manager
    disk_cache: DiskCache,

    // Active voices
    voices: HashMap<String, Voice>,

    // Channel name → number mapping
    channel_map: HashMap<String, usize>,

    // Shared mixer state (updated from here, read from audio callback)
    mixer_state: Arc<RwLock<MixerState>>,

    // Communication channels
    command_rx: mpsc::Receiver<AudioCommand>,

    // Worker pool handle
    decode_pool: DecodePool,

    // Next sample ID
    next_sample_id: AtomicU64,

    // Device configuration
    device_config: DeviceConfig,
}

struct Voice {
    id: String,
    active_sample_ids: Vec<u64>,
    volume: f32,
    fade_state: Option<VoiceFadeState>,
}

struct VoiceFadeState {
    target_volume: f32,
    duration_ms: u32,
    start_time: Instant,
}

struct DeviceConfig {
    sample_rate: u32,
    channels: usize,
    buffer_size: usize,
}
```

### Command Processing

```rust
enum AudioCommand {
    Play {
        file: String,
        voice: Option<String>,
        channel_map: Vec<(usize, usize)>,
        volume: f32,
        loop_mode: bool,
        fade_in_ms: u32,
        max_play_length_ms: Option<u32>,
    },
    StopAll,
    VoiceStop { voice: String },
    VoiceFadeOut { voice: String, time_ms: u32 },
    VoiceVolume { voice: String, volume: f32 },
    Precache { file: String },
    CacheClear,
    CacheInvalidate { file: String },
}

impl AudioEngine {
    async fn process_command(&mut self, cmd: AudioCommand) {
        match cmd {
            AudioCommand::Play { file, voice, channel_map, volume, .. } => {
                // 1. Get or load sample
                let buffer = self.get_or_load_sample(&file).await;

                // 2. Create voice if needed
                let voice_id = voice.unwrap_or_else(|| format!("_auto_{}", self.next_id()));
                let voice_obj = self.voices.entry(voice_id.clone())
                    .or_insert_with(|| Voice::new(voice_id.clone()));

                // 3. Create active sample
                let sample_id = self.next_sample_id.fetch_add(1, Ordering::SeqCst);
                let active_sample = ActiveSample {
                    id: sample_id,
                    buffer,
                    position: 0,
                    channels: buffer.channels,
                    channel_map,
                    volume,
                    voice_volume: voice_obj.volume,
                    fade_state: FadeState::new_fade_in(fade_in_ms, sample_rate),
                    loop_mode: if loop_mode { LoopMode::Infinite } else { LoopMode::Once },
                    max_frames: max_play_length_ms.map(|ms| ms_to_frames(ms, sample_rate)),
                };

                // 4. Add to mixer state
                let mut state = self.mixer_state.write().unwrap();
                state.active_samples.push(active_sample);

                // 5. Track in voice
                voice_obj.active_sample_ids.push(sample_id);
            }

            AudioCommand::VoiceFadeOut { voice, time_ms } => {
                if let Some(voice_obj) = self.voices.get_mut(&voice) {
                    // Update fade state for all samples in this voice
                    let mut state = self.mixer_state.write().unwrap();
                    for sample in state.active_samples.iter_mut() {
                        if voice_obj.active_sample_ids.contains(&sample.id) {
                            sample.fade_state = FadeState::new_fade_out(
                                time_ms,
                                self.device_config.sample_rate
                            );
                        }
                    }
                }
            }

            // ... other commands
        }
    }

    async fn get_or_load_sample(&mut self, file: &str) -> Arc<DecodedBuffer> {
        // Check memory cache
        if let Some(buffer) = self.sample_cache.get(file) {
            return buffer.clone();
        }

        // Check disk cache or download/decode
        let buffer = self.decode_pool.decode(file, self.device_config.sample_rate).await;

        // Store in memory cache
        self.sample_cache.insert(file.to_string(), buffer.clone());

        buffer
    }
}
```

### Cleanup of Completed Samples

```rust
impl AudioEngine {
    fn cleanup_completed_samples(&mut self) {
        let mut state = self.mixer_state.write().unwrap();

        // Remove samples that have finished
        state.active_samples.retain(|sample| {
            // Check if sample is done
            let is_done = sample.position >= sample.buffer.frames &&
                          matches!(sample.loop_mode, LoopMode::Once);

            // If done, remove from voice tracking
            if is_done {
                for voice in self.voices.values_mut() {
                    voice.active_sample_ids.retain(|id| *id != sample.id);
                }
            }

            !is_done
        });

        // Remove empty voices
        self.voices.retain(|_, voice| !voice.active_sample_ids.is_empty());
    }
}
```

This cleanup should run periodically (e.g., every 100ms) from the coordinator thread, not from the audio callback.

## Performance Considerations

### Audio Callback Budget

For a 512-frame buffer at 48kHz:
- Buffer duration: 512 / 48000 = 10.6ms
- Safe callback duration: < 5ms (leave headroom)

### Optimization Checklist

- [ ] Profile callback execution time
- [ ] Ensure zero allocations in callback
- [ ] Use SIMD for mixing if available
- [ ] Minimize cache misses (layout data linearly)
- [ ] Avoid branches in inner loops
- [ ] Pre-compute fade curves
- [ ] Consider using f32 everywhere (faster than f64 on most hardware)

### Lock Contention

Using RwLock:
- Audio callback: read-only access (fast)
- Engine: write access (infrequent)
- Contention should be minimal

If profiling shows lock contention:
- Switch to lock-free queue of updates
- Or use atomic operations
- Or use triple buffering with atomic swap

## Testing the Mixer

See testing.md for comprehensive test plan, but key tests:

1. **Single sample playback** - Verify correct output
2. **Multiple samples** - Verify additive mixing
3. **Channel mapping** - Verify correct routing
4. **Looping** - Verify seamless loop points
5. **Fading** - Verify smooth fade curves
6. **Voice operations** - Verify group control
7. **Saturation** - Verify clipping protection
8. **Sample completion** - Verify cleanup

## Next Steps

1. Implement basic cpal output
2. Implement mixer with single sample
3. Add multi-sample mixing
4. Add channel routing
5. Add fade envelopes
6. Add voice management
7. Optimize and profile
