# Testing Strategy

## Testing Philosophy

Audio software requires rigorous testing because bugs manifest as glitches, dropouts, or silent failures that are hard to diagnose. Our testing strategy covers:

1. **Unit tests** - Individual components in isolation
2. **Integration tests** - Components working together
3. **Real-time performance tests** - Audio callback timing
4. **Format compatibility tests** - All supported audio formats
5. **Stress tests** - Edge cases and limits
6. **Manual tests** - Real-world usage scenarios

## Test Audio Files

### Generate Test Suite

Create a comprehensive set of test audio files:

```bash
# tests/audio/
├── wav/
│   ├── mono_8bit_8khz.wav
│   ├── mono_16bit_44khz.wav
│   ├── mono_24bit_48khz.wav
│   ├── stereo_16bit_44khz.wav
│   ├── stereo_24bit_96khz.wav
│   ├── quad_24bit_48khz.wav      # 4 channels
│   ├── 5.1_24bit_48khz.wav       # 6 channels
│   └── 7.1_24bit_48khz.wav       # 8 channels
├── ogg/
│   ├── mono_44khz.ogg
│   ├── stereo_44khz.ogg
│   └── stereo_48khz.ogg
├── mp3/
│   ├── mono_128kbps.mp3
│   ├── stereo_128kbps.mp3
│   └── stereo_320kbps.mp3
└── flac/
    ├── mono_44khz.flac
    ├── stereo_44khz.flac
    └── stereo_96khz_24bit.flac
```

### Test File Characteristics

Each file should be:
- **Short** (1-5 seconds) for fast tests
- **Identifiable content** (tone sweep, voice, click pattern)
- **Known characteristics** (exact sample count, channel layout)

### Generation Script

```python
# tests/generate_test_files.py
import numpy as np
from scipy.io import wavfile
from pydub import AudioSegment

def generate_sine_wave(freq, duration, sample_rate, channels):
    """Generate multi-channel sine wave test signal"""
    t = np.linspace(0, duration, int(sample_rate * duration))
    audio = np.sin(2 * np.pi * freq * t)

    if channels > 1:
        # Different frequency per channel for identification
        multi = np.zeros((len(t), channels))
        for ch in range(channels):
            multi[:, ch] = np.sin(2 * np.pi * (freq + ch * 100) * t)
        return multi
    return audio

# Generate WAV files
wavfile.write('tests/audio/wav/stereo_16bit_44khz.wav',
              44100,
              (generate_sine_wave(440, 2.0, 44100, 2) * 32767).astype(np.int16))

# Convert to other formats using ffmpeg or pydub
# ...
```

## Unit Tests

### Channel Mapping

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_mapping_mono_to_single_channel() {
        let map = vec![(0, 5)]; // Mono source → output channel 5
        let source = vec![0.5f32]; // 1 frame, 1 channel
        let mut output = vec![0.0f32; 10]; // 1 frame, 10 channels

        apply_channel_mapping(&source, &mut output, &map, 1, 10);

        assert_eq!(output[5], 0.5);
        assert_eq!(output[0], 0.0); // Other channels untouched
    }

    #[test]
    fn test_channel_mapping_stereo_to_arbitrary() {
        let map = vec![(0, 6), (1, 7)]; // L→6, R→7
        let source = vec![0.3f32, 0.7f32]; // 1 frame, 2 channels
        let mut output = vec![0.0f32; 10];

        apply_channel_mapping(&source, &mut output, &map, 2, 10);

        assert_eq!(output[6], 0.3);
        assert_eq!(output[7], 0.7);
    }

    #[test]
    fn test_channel_mapping_multiple_frames() {
        let map = vec![(0, 2)];
        let source = vec![0.1, 0.2, 0.3]; // 3 frames, 1 channel
        let mut output = vec![0.0f32; 12]; // 3 frames, 4 channels

        apply_channel_mapping(&source, &mut output, &map, 1, 4);

        assert_eq!(output[2], 0.1);  // Frame 0, channel 2
        assert_eq!(output[6], 0.2);  // Frame 1, channel 2
        assert_eq!(output[10], 0.3); // Frame 2, channel 2
    }
}
```

### Fade Envelope

```rust
#[test]
fn test_fade_in_linear() {
    let fade = FadeState::new_fade_in(1000, 48000); // 1 second at 48kHz
    let multipliers = calculate_fade_multipliers(&fade, 480); // 10ms chunk

    assert_eq!(multipliers[0], 0.0);      // Starts at 0
    assert!(multipliers[240] > 0.4);      // Middle ~0.5
    assert!(multipliers[240] < 0.6);
}

#[test]
fn test_fade_out_linear() {
    let fade = FadeState::new_fade_out(2000, 48000);
    let multipliers = calculate_fade_multipliers(&fade, 960);

    assert_eq!(multipliers[0], 1.0);      // Starts at 1
    assert!(multipliers[480] > 0.4);      // Middle ~0.5
    assert!(multipliers[480] < 0.6);
}

#[test]
fn test_fade_completion() {
    let mut fade = FadeState::new_fade_out(100, 48000); // 100ms
    fade.current_frame = 5000; // Beyond fade duration

    let multipliers = calculate_fade_multipliers(&fade, 480);
    assert!(multipliers.iter().all(|&m| m == 0.0)); // All zeros
}
```

### Sample Mixing

```rust
#[test]
fn test_additive_mixing() {
    let mut output = vec![0.0f32; 4];
    let sample1 = vec![0.5, 0.5, 0.5, 0.5];
    let sample2 = vec![0.3, 0.3, 0.3, 0.3];

    mix_into_buffer(&sample1, &mut output, 1.0);
    mix_into_buffer(&sample2, &mut output, 1.0);

    assert_eq!(output, vec![0.8, 0.8, 0.8, 0.8]);
}

#[test]
fn test_mixing_with_volume() {
    let mut output = vec![0.0f32; 2];
    let sample = vec![1.0, 1.0];

    mix_into_buffer(&sample, &mut output, 0.5);

    assert_eq!(output, vec![0.5, 0.5]);
}

#[test]
fn test_saturation_protection() {
    let mut output = vec![0.9f32; 2];
    let sample = vec![0.5, 0.5];

    mix_into_buffer(&sample, &mut output, 1.0);
    apply_saturation(&mut output);

    assert_eq!(output, vec![1.0, 1.0]); // Clipped to 1.0
}
```

### Voice Management

```rust
#[test]
fn test_voice_creation() {
    let mut engine = AudioEngine::new();
    let cmd = AudioCommand::Play {
        voice: Some("test".to_string()),
        // ... other fields
    };

    engine.process_command(cmd).await;

    assert!(engine.voices.contains_key("test"));
}

#[test]
fn test_voice_stop_removes_samples() {
    let mut engine = AudioEngine::new();
    // Add samples to voice "test"
    // ...

    engine.process_command(AudioCommand::VoiceStop {
        voice: "test".to_string()
    }).await;

    let state = engine.mixer_state.read().unwrap();
    assert_eq!(state.active_samples.len(), 0);
}

#[test]
fn test_voice_fade_out() {
    let mut engine = AudioEngine::new();
    // Add samples to voice
    // ...

    engine.process_command(AudioCommand::VoiceFadeOut {
        voice: "test".to_string(),
        time_ms: 1000,
    }).await;

    let state = engine.mixer_state.read().unwrap();
    for sample in &state.active_samples {
        assert!(matches!(sample.fade_state.kind, FadeKind::Out));
    }
}
```

## Integration Tests

### Decode and Play

```rust
#[tokio::test]
async fn test_decode_wav_file() {
    let buffer = decode_file("tests/audio/wav/stereo_16bit_44khz.wav", 48000).await.unwrap();

    assert_eq!(buffer.sample_rate, 48000); // Resampled
    assert_eq!(buffer.channels, 2);
    assert!(buffer.frames > 0);
}

#[tokio::test]
async fn test_decode_all_formats() {
    let files = [
        "tests/audio/wav/mono_16bit_44khz.wav",
        "tests/audio/ogg/stereo_44khz.ogg",
        "tests/audio/mp3/stereo_128kbps.mp3",
        "tests/audio/flac/stereo_44khz.flac",
    ];

    for file in &files {
        let buffer = decode_file(file, 48000).await;
        assert!(buffer.is_ok(), "Failed to decode {}", file);
    }
}
```

### End-to-End Playback

```rust
#[tokio::test]
async fn test_play_command_end_to_end() {
    let mut engine = AudioEngine::new();

    let cmd = AudioCommand::Play {
        file: "tests/audio/wav/stereo_16bit_44khz.wav".to_string(),
        voice: None,
        channel_map: vec![(0, 0), (1, 1)],
        volume: 1.0,
        loop_mode: false,
        fade_in_ms: 0,
        max_play_length_ms: None,
    };

    engine.process_command(cmd).await;

    let state = engine.mixer_state.read().unwrap();
    assert_eq!(state.active_samples.len(), 1);
}
```

### Cache Operations

```rust
#[tokio::test]
async fn test_disk_cache_write_and_read() {
    let cache = DiskCache::new("tests/tmp/cache");
    let url = "http://example.com/test.wav";
    let data = vec![1, 2, 3, 4, 5];

    cache.write(url, &data, None, None).await.unwrap();

    let cached = cache.read(url).await.unwrap();
    assert_eq!(cached, data);
}

#[tokio::test]
async fn test_cache_invalidation() {
    let cache = DiskCache::new("tests/tmp/cache");
    let url = "http://example.com/test.wav";

    cache.write(url, &[1, 2, 3], None, None).await.unwrap();
    cache.invalidate(url).await.unwrap();

    let result = cache.read(url).await;
    assert!(result.is_err()); // Should be gone
}
```

## Performance Tests

### Audio Callback Timing

```rust
#[test]
fn test_callback_execution_time() {
    let state = create_test_mixer_state_with_10_samples();
    let mut output = vec![0.0f32; 512 * 2]; // 512 frames, stereo

    let start = Instant::now();
    audio_callback(&mut output, &state);
    let duration = start.elapsed();

    // For 512 frames at 48kHz = 10.67ms buffer duration
    // Callback should complete in < 5ms (50% of buffer duration)
    assert!(duration.as_micros() < 5000,
            "Callback took {:?}, too slow!", duration);
}

#[test]
fn test_callback_with_many_samples() {
    let state = create_test_mixer_state_with_50_samples();
    let mut output = vec![0.0f32; 512 * 8]; // 512 frames, 8 channels

    let start = Instant::now();
    audio_callback(&mut output, &state);
    let duration = start.elapsed();

    assert!(duration.as_micros() < 10000,
            "50 samples took {:?}, too slow!", duration);
}
```

### Memory Allocation

```rust
#[test]
fn test_callback_zero_allocations() {
    // This requires instrumentation or memory profiling tools
    // Can use https://crates.io/crates/dhat

    let state = create_test_mixer_state();
    let mut output = vec![0.0f32; 512 * 2];

    // Set up allocation tracker
    let _profiler = dhat::Profiler::new_heap();

    audio_callback(&mut output, &state);

    // Assert zero allocations occurred
    // (Implementation depends on profiling tool)
}
```

## Format Compatibility Tests

### Sample Rate Conversions

```rust
#[tokio::test]
async fn test_resample_quality() {
    let test_cases = vec![
        (8000, 48000),   // Extreme upsampling
        (44100, 48000),  // Common conversion
        (96000, 48000),  // Downsampling
        (48000, 48000),  // No-op
    ];

    for (input_rate, output_rate) in test_cases {
        let file = generate_test_tone(input_rate, 440.0);
        let buffer = decode_and_resample(&file, output_rate).await.unwrap();

        assert_eq!(buffer.sample_rate, output_rate);
        // Could also verify frequency content with FFT
    }
}
```

### Bit Depth Conversions

```rust
#[test]
fn test_bit_depth_conversions() {
    // Test i16 → f32
    let i16_samples: Vec<i16> = vec![0, 16384, 32767, -32768];
    let f32_samples = convert_i16_to_f32(&i16_samples);

    assert_eq!(f32_samples[0], 0.0);
    assert!((f32_samples[2] - 1.0).abs() < 0.001);
    assert!((f32_samples[3] - (-1.0)).abs() < 0.001);

    // Test i24 → f32
    // Test i32 → f32
    // Test u8 → f32
}
```

## Stress Tests

### Rapid Commands

```rust
#[tokio::test]
async fn test_rapid_play_commands() {
    let mut engine = AudioEngine::new();

    // Send 100 play commands in rapid succession
    for i in 0..100 {
        let cmd = AudioCommand::Play {
            file: format!("tests/audio/wav/test{}.wav", i % 10),
            // ... other fields
        };
        engine.process_command(cmd).await;
    }

    // Engine should handle without crashing
    let state = engine.mixer_state.read().unwrap();
    assert!(state.active_samples.len() <= 128); // Max samples enforced
}
```

### Long-Running Playback

```rust
#[tokio::test]
async fn test_long_running_loop() {
    let mut engine = AudioEngine::new();

    // Play looping audio
    engine.process_command(AudioCommand::Play {
        file: "tests/audio/wav/loop.wav".to_string(),
        loop_mode: true,
        // ...
    }).await;

    // Simulate 1 hour of playback
    for _ in 0..360000 { // 10ms per iteration
        tokio::time::sleep(Duration::from_millis(10)).await;
        // Verify no memory leaks, no glitches
    }
}
```

### Memory Limits

```rust
#[tokio::test]
async fn test_memory_cache_growth() {
    let mut engine = AudioEngine::new();

    // Load 100 different files
    for i in 0..100 {
        engine.process_command(AudioCommand::Precache {
            file: format!("tests/audio/large/file{}.wav", i),
        }).await;
    }

    // Check memory usage
    let cache_size = engine.sample_cache.iter()
        .map(|(_, buf)| buf.data.len() * std::mem::size_of::<f32>())
        .sum::<usize>();

    println!("Cache size: {} MB", cache_size / 1024 / 1024);

    // Should implement eviction before hitting system limits
}
```

## Manual Test Scenarios

### Interactive Test Script

```bash
#!/bin/bash
# tests/manual_test.sh

TOPIC="audio/test"

echo "=== Manual mqttaudio Test Suite ==="

echo "1. Testing basic playback..."
mosquitto_pub -t $TOPIC -m '{"command": "play", "message": {"file": "tests/audio/wav/test.wav"}}'
sleep 3

echo "2. Testing multichannel routing..."
mosquitto_pub -t $TOPIC -m '{"command": "play", "message": {"file": "tests/audio/wav/quad.wav", "channel_map": [{"src":0,"dest":0},{"src":1,"dest":1},{"src":2,"dest":2},{"src":3,"dest":3}]}}'
sleep 5

echo "3. Testing voice grouping..."
mosquitto_pub -t $TOPIC -m '{"command": "play", "message": {"file": "tests/audio/wav/music.wav", "voice": "bg", "loop": true}}'
sleep 2
mosquitto_pub -t $TOPIC -m '{"command": "voice_fade_out", "message": {"voice": "bg", "time": 2000}}'
sleep 3

echo "4. Testing multiple simultaneous sounds..."
for i in {1..10}; do
    mosquitto_pub -t $TOPIC -m '{"command": "play", "message": {"file": "tests/audio/wav/test.wav"}}'
done
sleep 5

echo "5. Testing stop all..."
mosquitto_pub -t $TOPIC -m '{"command": "stopall"}'

echo "=== Tests complete ==="
```

### Real Hardware Test

1. **Set up 8-channel audio interface**
2. **Configure mqttaudio for multichannel**
3. **Play test tones to each channel individually**
4. **Verify correct channel routing with headphones**
5. **Test polyphonic mixing (play to all channels simultaneously)**

## Continuous Integration

### GitHub Actions Workflow

```yaml
name: Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
        rust: [stable, nightly]

    steps:
      - uses: actions/checkout@v2

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: ${{ matrix.rust }}
          override: true

      - name: Install system dependencies (Linux)
        if: runner.os == 'Linux'
        run: |
          sudo apt-get update
          sudo apt-get install -y libasound2-dev

      - name: Run tests
        run: cargo test --verbose

      - name: Run tests (release mode)
        run: cargo test --release

      - name: Check formatting
        run: cargo fmt -- --check

      - name: Run clippy
        run: cargo clippy -- -D warnings
```

## Test Coverage Goals

- **Unit tests:** >80% code coverage
- **Integration tests:** All major workflows
- **Format tests:** Every supported format
- **Platform tests:** macOS, Linux, Windows
- **Performance tests:** Callback timing < 50% budget

## Test Execution

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_channel_mapping

# Run with output
cargo test -- --nocapture

# Run performance tests
cargo test --release -- perf_

# Run with coverage (requires tarpaulin)
cargo tarpaulin --out Html
```
