# Implementation Roadmap

## Overview

This document provides a step-by-step guide for implementing mqttaudio. Follow these phases sequentially, testing thoroughly at each stage.

## Phase 0: Project Setup (Day 1)

### Create Rust Project

```bash
cargo init --name mqttaudio
```

### Add Dependencies

Edit `Cargo.toml`:

```toml
[package]
name = "mqttaudio"
version = "0.1.0"
edition = "2021"

[dependencies]
# Audio I/O
cpal = "0.15"

# Audio decoding
symphonia = { version = "0.5", features = ["all"] }

# Sample rate conversion
rubato = "0.14"

# MQTT
rumqttc = "0.24"

# HTTP client
reqwest = { version = "0.11", features = ["stream"] }

# JSON parsing
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# CLI parsing
clap = { version = "4.5", features = ["derive"] }

# Async runtime
tokio = { version = "1", features = ["full"] }

# DSP utilities
dasp = "0.11"

# Logging
tracing = "0.1"
tracing-subscriber = "0.3"

# Lock-free structures
ringbuf = "0.3"

[dev-dependencies]
criterion = "0.5"
```

### Project Structure

```
mqttaudio/
├── src/
│   ├── main.rs              # Entry point, CLI parsing
│   ├── config.rs            # Configuration loading
│   ├── audio/
│   │   ├── mod.rs
│   │   ├── engine.rs        # Audio engine coordinator
│   │   ├── mixer.rs         # Audio callback and mixing
│   │   ├── decoder.rs       # Audio decoding with symphonia
│   │   ├── resampler.rs     # Sample rate conversion
│   │   └── types.rs         # Audio-related types
│   ├── mqtt/
│   │   ├── mod.rs
│   │   ├── client.rs        # MQTT connection management
│   │   └── commands.rs      # Command parsing
│   ├── cache/
│   │   ├── mod.rs
│   │   ├── disk.rs          # Disk cache implementation
│   │   └── memory.rs        # Memory cache
│   └── voice.rs             # Voice management
├── tests/
│   ├── integration_tests.rs
│   └── audio/               # Test audio files
├── benches/
│   └── mixing.rs            # Performance benchmarks
├── docs/                    # Architecture docs (already created)
└── Cargo.toml
```

### Verify Setup

```bash
cargo build
cargo test
```

## Phase 1: Basic Audio Output (Days 2-3)

**Goal:** Play a sine wave through default audio device.

### Step 1.1: Initialize cpal

File: `src/audio/engine.rs`

```rust
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub fn list_devices() {
    let host = cpal::default_host();
    for device in host.output_devices().unwrap() {
        println!("Device: {}", device.name().unwrap());
    }
}

pub fn init_default_output() -> Result<cpal::Stream, Error> {
    let host = cpal::default_host();
    let device = host.default_output_device().unwrap();
    let config = device.default_output_config().unwrap();

    println!("Using device: {}", device.name().unwrap());
    println!("Config: {:?}", config);

    // Build stream (next step)
    Ok(stream)
}
```

### Step 1.2: Create Audio Callback

```rust
fn audio_callback(data: &mut [f32], channels: usize) {
    // Generate 440Hz sine wave
    static mut PHASE: f32 = 0.0;
    let phase_increment = 440.0 * 2.0 * std::f32::consts::PI / 48000.0;

    for frame in data.chunks_mut(channels) {
        let sample = unsafe {
            let s = PHASE.sin();
            PHASE += phase_increment;
            if PHASE > 2.0 * std::f32::consts::PI {
                PHASE -= 2.0 * std::f32::consts::PI;
            }
            s * 0.2 // Low volume
        };

        for channel_sample in frame {
            *channel_sample = sample;
        }
    }
}
```

### Step 1.3: Build and Run Stream

```rust
let stream = device.build_output_stream(
    &config.into(),
    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        audio_callback(data, channels);
    },
    |err| eprintln!("Stream error: {}", err),
    None,
)?;

stream.play()?;
```

### Test

```bash
cargo run
# Should hear a 440Hz tone
```

**Checkpoint:** Can play a sine wave on default device.

## Phase 2: Audio Decoding (Days 4-5)

**Goal:** Load a WAV file and play it.

### Step 2.1: Implement Decoder

File: `src/audio/decoder.rs`

```rust
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use std::fs::File;

pub struct DecodedBuffer {
    pub data: Vec<f32>,
    pub channels: usize,
    pub sample_rate: u32,
    pub frames: usize,
}

pub fn decode_file(path: &str) -> Result<DecodedBuffer, Error> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let format_opts = FormatOptions::default();
    let metadata_opts = Default::default();

    let probed = symphonia::default::get_probe()
        .format(&format_opts, mss, &metadata_opts)?;

    let mut format = probed.format;
    let track = format.default_track().unwrap();
    let track_id = track.id;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &Default::default())?;

    let mut samples = Vec::new();
    let channels = track.codec_params.channels.unwrap().count();
    let sample_rate = track.codec_params.sample_rate.unwrap();

    // Decode all packets
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break,
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = decoder.decode(&packet)?;

        // Convert to f32 and interleave
        // (Implementation details depend on sample format)
        // ...
    }

    Ok(DecodedBuffer {
        data: samples,
        channels,
        sample_rate,
        frames: samples.len() / channels,
    })
}
```

### Step 2.2: Play Decoded Audio

Modify audio callback to read from buffer instead of generating sine wave:

```rust
struct PlaybackState {
    buffer: DecodedBuffer,
    position: usize,
}

fn audio_callback(data: &mut [f32], state: &mut PlaybackState) {
    let frames_needed = data.len() / state.buffer.channels;

    for frame_idx in 0..frames_needed {
        if state.position >= state.buffer.frames {
            // End of buffer - output silence
            for i in 0..state.buffer.channels {
                data[frame_idx * state.buffer.channels + i] = 0.0;
            }
            continue;
        }

        // Copy frame from buffer
        for ch in 0..state.buffer.channels {
            let src_idx = state.position * state.buffer.channels + ch;
            let dst_idx = frame_idx * state.buffer.channels + ch;
            data[dst_idx] = state.buffer.data[src_idx];
        }

        state.position += 1;
    }
}
```

### Test

```bash
cargo run -- --file tests/audio/test.wav
# Should play the WAV file
```

**Checkpoint:** Can decode and play WAV files.

## Phase 3: Sample Rate Conversion (Day 6)

**Goal:** Handle files with different sample rates.

### Step 3.1: Implement Resampler

File: `src/audio/resampler.rs`

```rust
use rubato::{SincFixedIn, Resampler};

pub fn resample(
    input: Vec<f32>,
    input_rate: u32,
    output_rate: u32,
    channels: usize,
) -> Result<Vec<f32>, Error> {
    if input_rate == output_rate {
        return Ok(input); // No resampling needed
    }

    let mut resampler = SincFixedIn::<f32>::new(
        output_rate as f64 / input_rate as f64,
        2.0, // Max relative ratio difference
        rubato::PolynomialDegree::Septic,
        1024, // Chunk size
        channels,
    )?;

    // De-interleave input
    let frames = input.len() / channels;
    let mut input_channels: Vec<Vec<f32>> = vec![vec![0.0; frames]; channels];
    for (frame_idx, frame) in input.chunks(channels).enumerate() {
        for (ch_idx, &sample) in frame.iter().enumerate() {
            input_channels[ch_idx][frame_idx] = sample;
        }
    }

    // Resample
    let output_channels = resampler.process(&input_channels, None)?;

    // Re-interleave
    let output_frames = output_channels[0].len();
    let mut output = Vec::with_capacity(output_frames * channels);
    for frame_idx in 0..output_frames {
        for ch_idx in 0..channels {
            output.push(output_channels[ch_idx][frame_idx]);
        }
    }

    Ok(output)
}
```

### Step 3.2: Integrate into Decoder

```rust
pub fn decode_file(path: &str, target_sample_rate: u32) -> Result<DecodedBuffer, Error> {
    let buffer = decode_file_raw(path)?;

    // Resample if needed
    let data = if buffer.sample_rate != target_sample_rate {
        resample(buffer.data, buffer.sample_rate, target_sample_rate, buffer.channels)?
    } else {
        buffer.data
    };

    Ok(DecodedBuffer {
        data,
        sample_rate: target_sample_rate,
        ..buffer
    })
}
```

### Test

```bash
# Test with various sample rates
cargo run -- --file tests/audio/44khz.wav  # Should resample to 48kHz
cargo run -- --file tests/audio/96khz.wav  # Should downsample to 48kHz
```

**Checkpoint:** Can handle files with any sample rate.

## Phase 4: Multi-Sample Mixing (Days 7-8)

**Goal:** Play multiple sounds simultaneously.

### Step 4.1: Implement Mixer

File: `src/audio/mixer.rs`

```rust
pub struct ActiveSample {
    pub buffer: Arc<DecodedBuffer>,
    pub position: usize,
    pub volume: f32,
    pub channel_map: Vec<(usize, usize)>,
}

pub struct MixerState {
    pub active_samples: Vec<ActiveSample>,
    pub output_channels: usize,
}

pub fn mix_audio(
    output: &mut [f32],
    state: &MixerState,
) {
    // Zero output buffer
    output.fill(0.0);

    let frames = output.len() / state.output_channels;

    for sample in &state.active_samples {
        mix_sample_into_output(sample, output, frames, state.output_channels);
    }

    // Apply saturation
    for s in output.iter_mut() {
        *s = s.clamp(-1.0, 1.0);
    }
}

fn mix_sample_into_output(
    sample: &ActiveSample,
    output: &mut [f32],
    frames: usize,
    output_channels: usize,
) {
    for frame_idx in 0..frames {
        let src_position = sample.position + frame_idx;
        if src_position >= sample.buffer.frames {
            break; // Sample finished
        }

        // Apply channel mapping
        for &(src_ch, dest_ch) in &sample.channel_map {
            let src_idx = src_position * sample.buffer.channels + src_ch;
            let dest_idx = frame_idx * output_channels + dest_ch;

            if src_ch < sample.buffer.channels && dest_ch < output_channels {
                output[dest_idx] += sample.buffer.data[src_idx] * sample.volume;
            }
        }
    }
}
```

### Test

Create integration test that plays 3 sounds simultaneously.

**Checkpoint:** Can mix multiple samples together.

## Phase 5: MQTT Integration (Days 9-10)

**Goal:** Receive play commands via MQTT.

### Step 5.1: MQTT Client

File: `src/mqtt/client.rs`

```rust
use rumqttc::{AsyncClient, MqttOptions, Event, Packet};

pub async fn connect_mqtt(
    server: &str,
    port: u16,
    topic: &str,
) -> Result<(AsyncClient, EventLoop), Error> {
    let mut mqttoptions = MqttOptions::new("mqttaudio", server, port);
    mqttoptions.set_keep_alive(Duration::from_secs(60));

    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);

    client.subscribe(topic, rumqttc::QoS::AtLeastOnce).await?;

    Ok((client, eventloop))
}

pub async fn process_mqtt_events(
    mut eventloop: EventLoop,
    command_tx: mpsc::Sender<String>,
) {
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::Publish(p))) => {
                let payload = String::from_utf8_lossy(&p.payload);
                command_tx.send(payload.to_string()).await.unwrap();
            }
            Err(e) => {
                eprintln!("MQTT error: {}", e);
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
            _ => {}
        }
    }
}
```

### Step 5.2: Command Parsing

File: `src/mqtt/commands.rs`

```rust
#[derive(Deserialize)]
struct MqttCommand {
    command: String,
    message: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct PlayMessage {
    file: String,
    voice: Option<String>,
    volume: Option<f32>,
    #[serde(rename = "loop")]
    loop_mode: Option<bool>,
    // ... other fields
}

pub fn parse_command(json: &str) -> Result<AudioCommand, Error> {
    let cmd: MqttCommand = serde_json::from_str(json)?;

    match cmd.command.as_str() {
        "play" | "soundPlay" => {
            let msg: PlayMessage = serde_json::from_value(cmd.message.unwrap())?;
            Ok(AudioCommand::Play {
                file: msg.file,
                voice: msg.voice,
                volume: msg.volume.unwrap_or(1.0),
                loop_mode: msg.loop_mode.unwrap_or(false),
                // ...
            })
        }
        "stopall" | "soundStopAll" => Ok(AudioCommand::StopAll),
        // ... other commands
        _ => Err(Error::UnknownCommand),
    }
}
```

### Test

```bash
# Terminal 1
cargo run

# Terminal 2
mosquitto_pub -t "audio/commands" -m '{"command": "play", "message": {"file": "tests/audio/test.wav"}}'
```

**Checkpoint:** Can receive MQTT commands and play audio.

## Phase 6: Voice Management (Days 11-12)

**Goal:** Implement voice grouping and control.

### Implementation

See architecture.md and audio-engine.md for detailed design.

Key components:
- Voice struct with list of sample IDs
- voice_stop, voice_fade_out, voice_volume commands
- Tracking samples per voice

### Test

```bash
# Play multiple sounds in same voice
mosquitto_pub -t "audio/commands" -m '{"command": "play", "message": {"file": "a.wav", "voice": "bg"}}'
mosquitto_pub -t "audio/commands" -m '{"command": "play", "message": {"file": "b.wav", "voice": "bg"}}'

# Stop all sounds in voice
mosquitto_pub -t "audio/commands" -m '{"command": "voice_stop", "message": {"voice": "bg"}}'
```

**Checkpoint:** Voice commands work correctly.

## Phase 7: Channel Routing (Days 13-14)

**Goal:** Implement multichannel routing and named channels.

### Implementation

- Parse channel_map from commands
- Implement channel mapping in mixer
- Add channel name resolution from config

### Test

```bash
# Play 4-channel file with custom routing
mosquitto_pub -t "audio/commands" -m '{
  "command": "play",
  "message": {
    "file": "tests/audio/quad.wav",
    "channel_map": [
      {"src": 0, "dest": 6},
      {"src": 1, "dest": 7},
      {"src": 2, "dest": 8},
      {"src": 3, "dest": 9}
    ]
  }
}'
```

**Checkpoint:** Multichannel routing works.

## Phase 8: Caching (Days 15-17)

**Goal:** Implement disk cache and HTTP downloading.

### Step 8.1: HTTP Downloader

```rust
use reqwest;

pub async fn download_file(url: &str) -> Result<Vec<u8>, Error> {
    let response = reqwest::get(url).await?;
    let bytes = response.bytes().await?;
    Ok(bytes.to_vec())
}
```

### Step 8.2: Disk Cache

See caching.md for detailed design.

Implement:
- Cache metadata management
- File storage
- Validation with ETag/Last-Modified
- cache_clear and cache_invalidate commands

### Test

```bash
# First play (downloads)
mosquitto_pub -t "audio/commands" -m '{"command": "play", "message": {"file": "http://localhost:8000/test.wav"}}'

# Second play (cached, instant)
mosquitto_pub -t "audio/commands" -m '{"command": "play", "message": {"file": "http://localhost:8000/test.wav"}}'
```

**Checkpoint:** HTTP files are cached and reused.

## Phase 9: Fading (Days 18-19)

**Goal:** Implement fade in/out.

### Implementation

- Add FadeState to ActiveSample
- Implement fade envelope calculation
- Apply fade multiplier in mixer
- Remove samples when fade out completes

### Test

```bash
# Fade in
mosquitto_pub -t "audio/commands" -m '{"command": "play", "message": {"file": "test.wav", "fade_in": 2000}}'

# Fade out voice
mosquitto_pub -t "audio/commands" -m '{"command": "voice_fade_out", "message": {"voice": "bg", "time": 3000}}'
```

**Checkpoint:** Fading works smoothly.

## Phase 10: Configuration (Day 20)

**Goal:** Load settings from JSON config file.

### Implementation

```rust
#[derive(Deserialize)]
struct Config {
    mqtt: MqttConfig,
    audio: AudioConfig,
    cache: CacheConfig,
    security: SecurityConfig,
}

// Load from file, merge with CLI args
```

### Test

```bash
cargo run --config config.json
```

**Checkpoint:** Config file loads and applies settings.

## Phase 11: Polish & Testing (Days 21-25)

### Tasks

- Write comprehensive unit tests (target >80% coverage)
- Write integration tests for all commands
- Performance testing and optimization
- Error handling improvements
- Logging improvements
- Documentation review
- Cross-platform testing (macOS, Linux, Windows if possible)

### Benchmarks

```bash
cargo bench
```

Ensure performance targets met (see performance.md).

### Stress Testing

```bash
# tests/stress_test.sh - rapid commands, many files, long duration
```

**Checkpoint:** All tests pass, performance targets met.

## Phase 12: Release Preparation (Days 26-27)

### Tasks

- [ ] Update README.md with usage instructions
- [ ] Create example config file
- [ ] Build release binaries
- [ ] Write CHANGELOG.md
- [ ] Tag version 0.1.0
- [ ] Test installation on fresh systems

### Build Release

```bash
cargo build --release
strip target/release/mqttaudio  # Reduce binary size
```

### Package

```bash
tar -czf mqttaudio-0.1.0-linux-x86_64.tar.gz -C target/release mqttaudio
```

**Checkpoint:** Ready for deployment.

## Development Tips

### Iterative Development

Don't try to build everything at once. Each phase should:
1. Add one feature
2. Test that feature thoroughly
3. Commit to git
4. Move to next phase

### Debug Logging

Use `tracing` extensively:

```rust
tracing::info!("Playing file: {}", file);
tracing::debug!("Decoded {} frames", buffer.frames);
tracing::warn!("Cache miss for {}", url);
```

Enable with:
```bash
RUST_LOG=mqttaudio=debug cargo run
```

### Common Pitfalls

1. **Don't allocate in audio callback** - Pre-allocate everything
2. **Test on real hardware early** - Emulators may not catch issues
3. **Profile before optimizing** - Measure first, then optimize
4. **Handle errors gracefully** - Audio should never crash
5. **Test edge cases** - Empty files, corrupt files, huge files

### Git Strategy

Commit frequently:
```bash
git commit -m "Phase 2: Implement basic WAV decoding"
git commit -m "Phase 3: Add sample rate conversion"
```

Tag milestones:
```bash
git tag v0.1.0-phase6  # Voice management complete
```

## Success Criteria

Before calling it done:

- [ ] All commands from commands.md work
- [ ] All unit tests pass
- [ ] Integration tests pass
- [ ] Performance benchmarks meet targets
- [ ] No audio glitches under normal load
- [ ] Works on at least 2 platforms (macOS, Linux)
- [ ] Documentation complete
- [ ] Example config file provided
- [ ] Can run for 1+ hours without issues

## Next Steps After v0.1.0

Future enhancements (see individual docs for details):
- [ ] MQTT authentication support
- [ ] GUI control panel (optional)
- [ ] Advanced DSP effects (EQ, filters)
- [ ] Playlist support
- [ ] Multiple simultaneous MQTT topics
- [ ] Metrics export (Prometheus)
- [ ] Hot reload of configuration
- [ ] File system watching for local files
- [ ] LRU cache eviction
