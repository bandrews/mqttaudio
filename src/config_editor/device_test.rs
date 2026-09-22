// ABOUTME: Headless live device test sessions for the config editor.
// ABOUTME: Plays test tones through the real output pathway and meters input capture.

use crate::audio::bass_management::{BassManagement, BassManagementConfig};
use crate::audio::engine::{build_output_stream, find_output_config, find_output_device};
use crate::audio::input::{create_input_stream, ActiveInput, InputStreamConfig};
use crate::audio::mixer::{db_to_linear, ActiveSample, MixerState};
use crate::audio::types::DecodedBuffer;
use crate::config::Config;
use crate::rt_engine::{
    command_channel, command_return_channel, graveyard_channel, streamed_graveyard_channel,
    AudioCallbackState, AudioCommand, CommandProducer, CommandReturnConsumer, GraveyardConsumer,
    StreamedGraveyardConsumer,
};
use cpal::traits::StreamTrait;
use parking_lot::Mutex;
use ringbuf::HeapConsumer;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Generate a mono sine tone with linear fade-in/out edges so playback never
/// clicks. Returns interleavable mono f32 samples at `sample_rate`.
pub fn generate_tone(
    freq_hz: f32,
    sample_rate: u32,
    duration_ms: u32,
    fade_ms: u32,
    amplitude: f32,
) -> Vec<f32> {
    let total = (sample_rate as u64 * duration_ms as u64 / 1000) as usize;
    let fade = ((sample_rate as u64 * fade_ms as u64 / 1000) as usize).min(total / 2);
    let mut samples = Vec::with_capacity(total);
    for n in 0..total {
        let phase = 2.0 * std::f32::consts::PI * freq_hz * n as f32 / sample_rate as f32;
        let mut s = amplitude * phase.sin();
        if fade > 0 {
            if n < fade {
                s *= n as f32 / fade as f32;
            }
            if n >= total - fade {
                s *= (total - n - 1) as f32 / fade as f32;
            }
        }
        samples.push(s);
    }
    samples
}

/// A live output test: a real stream on the chosen device, fed through the real
/// mixer (channel gains, master gain, limiter, and bass management from the
/// in-progress config), into which test tones are injected per channel.
pub struct OutputTestSession {
    stream: cpal::Stream,
    commands: CommandProducer,
    command_returns: CommandReturnConsumer,
    graveyard: GraveyardConsumer,
    _streamed_graveyard: StreamedGraveyardConsumer,
    error_flag: Arc<AtomicBool>,
    xruns: Arc<AtomicU64>,
    output_meters: Arc<Vec<AtomicU32>>,
    next_id: u64,
    /// Negotiated output channel count.
    pub channels: usize,
    /// Negotiated output sample rate (Hz).
    pub sample_rate: u32,
    /// Human-readable negotiation summary, e.g. "6 ch @ 48000 Hz, F32".
    pub description: String,
}

/// Frequency of the standard speaker-identification tone.
pub const TEST_TONE_HZ: f32 = 440.0;
/// Frequency of the bass test tone (below any sane crossover, so an enabled
/// bass management config routes it to the LFE channel audibly).
pub const BASS_TONE_HZ: f32 = 50.0;
/// Length of a test tone in milliseconds.
pub const TONE_DURATION_MS: u32 = 500;
const TONE_FADE_MS: u32 = 8;
const TONE_AMPLITUDE: f32 = 0.5;

/// Open `device_name` (None = default) through the real pathway —
/// `find_output_device` -> `find_output_config` -> `build_output_stream` — with
/// a mixer configured from `config`'s audio and bass management settings, so a
/// test tone is heard exactly as the daemon would play it.
pub fn start_output_test(
    config: &Config,
    device_name: Option<&str>,
) -> Result<OutputTestSession, String> {
    let device = find_output_device(device_name).map_err(|e| e.to_string())?;
    let output_config = find_output_config(
        &device,
        config.audio.channels,
        Some(config.audio.sample_rate),
        config.audio.buffer_size,
    )
    .map_err(|e| e.to_string())?;
    let channels = output_config.stream_config.channels as usize;
    let sample_rate = output_config.stream_config.sample_rate;
    let description = format!(
        "{} ch @ {} Hz, {:?}",
        channels, sample_rate, output_config.sample_format
    );

    let mut mixer = MixerState::new(channels);
    mixer.channel_gains = config.resolve_channel_gains(channels);
    mixer.master_gain = config.audio.master_gain;
    mixer.output_ceiling = db_to_linear(config.audio.output_ceiling_db);
    if config.bass_management.enabled {
        let resolved = config.resolve_bass_management()?;
        mixer.bass_management = Some(BassManagement::new(
            BassManagementConfig {
                enabled: resolved.enabled,
                lfe_channel: resolved.lfe_channel,
                crossover_frequency_hz: resolved.crossover_frequency_hz,
                source_channels: resolved.source_channels,
                remove_bass_from_sources: resolved.remove_bass_from_sources,
                lfe_gain: resolved.lfe_gain,
            },
            sample_rate,
            channels,
        ));
    }
    mixer.telemetry_enabled.store(true, Ordering::Relaxed);
    let output_meters = mixer.output_meters.clone();

    let (commands, cmd_rx) = command_channel(64);
    let (cmd_return_tx, command_returns) = command_return_channel(64);
    let (grave_tx, graveyard) = graveyard_channel(64);
    let (streamed_grave_tx, streamed_graveyard) = streamed_graveyard_channel(16);
    let callback_state = Arc::new(Mutex::new(AudioCallbackState {
        mixer,
        commands: cmd_rx,
        command_returns: cmd_return_tx,
        graveyard: grave_tx,
        streamed_graveyard: streamed_grave_tx,
        output_sample_rate: sample_rate,
    }));
    let xruns = Arc::new(AtomicU64::new(0));
    let error_flag = Arc::new(AtomicBool::new(false));

    // Built directly (not via the supervisor, which exits the process on an
    // unrecoverable device failure); a mid-test device error surfaces through
    // `error_flag` instead.
    let stream = build_output_stream(
        &device,
        &output_config.stream_config,
        output_config.sample_format,
        callback_state,
        xruns.clone(),
        error_flag.clone(),
    )
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;

    Ok(OutputTestSession {
        stream,
        commands,
        command_returns,
        graveyard,
        _streamed_graveyard: streamed_graveyard,
        error_flag,
        xruns,
        output_meters,
        next_id: 1,
        channels,
        sample_rate,
        description,
    })
}

impl OutputTestSession {
    /// Inject a tone routed to one output channel via the sample channel map,
    /// exactly as a play command with a channel route would.
    pub fn play_tone_on_channel(&mut self, channel: usize, freq_hz: f32) -> Result<(), String> {
        if channel >= self.channels {
            return Err(format!(
                "channel {} is beyond the {} output channels",
                channel, self.channels
            ));
        }
        let samples = generate_tone(
            freq_hz,
            self.sample_rate,
            TONE_DURATION_MS,
            TONE_FADE_MS,
            TONE_AMPLITUDE,
        );
        let buffer = Arc::new(DecodedBuffer::new(samples, 1, self.sample_rate));
        let id = self.next_id;
        self.next_id += 1;
        let sample = ActiveSample::new_with_mapping(
            id,
            "config-editor-test".to_string(),
            buffer,
            1.0,
            1.0,
            vec![(0, channel)],
            "test-tone".to_string(),
            None,
            false,
            0,
        );
        self.commands
            .push(AudioCommand::AddSample(sample))
            .map_err(|_| "command ring is full".to_string())
    }

    /// Per-channel post-limiter peak levels from the most recent audio block.
    pub fn channel_peaks(&self) -> Vec<f32> {
        self.output_meters
            .iter()
            .map(|m| f32::from_bits(m.load(Ordering::Relaxed)))
            .collect()
    }

    /// Drain the off-RT return rings (the editor's stand-in for the daemon's
    /// reaper) and report a stream error if one occurred. Call periodically.
    pub fn poll(&mut self) -> Result<(), String> {
        while self.graveyard.pop().is_some() {}
        while self.command_returns.pop().is_some() {}
        if self.error_flag.load(Ordering::Relaxed) {
            return Err(format!(
                "audio stream error (xruns: {})",
                self.xruns.load(Ordering::Relaxed)
            ));
        }
        Ok(())
    }
}

impl Drop for OutputTestSession {
    fn drop(&mut self) {
        // Stop the callback before the rings are torn down, then free anything
        // still parked in the return rings off-RT (the rings drop with self).
        let _ = self.stream.pause();
        while self.graveyard.pop().is_some() {}
        while self.command_returns.pop().is_some() {}
    }
}

/// A live input test: the real capture stream with a per-channel peak meter,
/// so the user can verify the right microphone before saving an inputs[] entry.
pub struct InputTestSession {
    _input: ActiveInput,
    consumer: HeapConsumer<f32>,
    /// Channel count of the opened input device.
    pub channels: usize,
    /// Current per-channel peak (decayed each poll for a live-meter feel).
    peaks: Vec<f32>,
}

/// Open the input device through the daemon's real capture path
/// (`create_input_stream`, including its async resampler) and meter it.
pub fn start_input_test(
    device_name: Option<&str>,
    latency_ms: u32,
    sample_rate: u32,
) -> Result<InputTestSession, String> {
    let mut input = create_input_stream(
        InputStreamConfig {
            device_name: device_name.map(|s| s.to_string()),
            latency_ms,
            ..InputStreamConfig::default()
        },
        sample_rate,
    )
    .map_err(|e| e.to_string())?;
    let consumer = input
        .take_consumer()
        .ok_or_else(|| "input consumer already taken".to_string())?;
    let channels = input.channels;
    Ok(InputTestSession {
        _input: input,
        consumer,
        channels,
        peaks: vec![0.0; channels],
    })
}

impl InputTestSession {
    /// Drain captured audio and update the per-channel peaks. Existing peaks
    /// decay toward zero so the meter falls back when the room goes quiet.
    pub fn poll(&mut self) {
        for p in &mut self.peaks {
            *p *= 0.7;
        }
        let mut ch = 0usize;
        while let Some(s) = self.consumer.pop() {
            let a = s.abs();
            if a > self.peaks[ch] {
                self.peaks[ch] = a;
            }
            ch = (ch + 1) % self.channels;
        }
    }

    /// Current per-channel peak levels (0.0 - 1.0-ish).
    pub fn peaks(&self) -> &[f32] {
        &self.peaks
    }
}
