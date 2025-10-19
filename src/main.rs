// ABOUTME: Entry point for mqttaudio MQTT-controlled audio daemon.
// ABOUTME: Handles CLI parsing, initialization, and main event loop.

mod audio;
mod cache;
mod config;
mod mqtt;
mod voice;

use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;
use tracing_subscriber;

#[derive(Parser, Debug)]
#[command(name = "mqttaudio")]
#[command(version = "0.1.0")]
#[command(about = "MQTT-controlled multichannel audio player", long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<String>,

    /// MQTT server hostname
    #[arg(short, long)]
    server: Option<String>,

    /// MQTT server port
    #[arg(short, long)]
    port: Option<u16>,

    /// MQTT topic to subscribe to
    #[arg(short, long)]
    topic: Option<String>,

    /// Audio output device name
    #[arg(short, long)]
    device: Option<String>,

    /// Sample rate (Hz)
    #[arg(short = 'r', long)]
    sample_rate: Option<u32>,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// List available audio devices and exit
    #[arg(long)]
    list_devices: bool,

    /// Play a 440Hz test tone (for testing audio output)
    #[arg(long)]
    test_tone: bool,

    /// Play an audio file (for testing decoder)
    #[arg(long)]
    file: Option<String>,

    /// Test mixer with multiple simultaneous files (Phase 4)
    #[arg(long)]
    test_mixer: bool,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Initialize logging
    let log_level = if args.verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    tracing_subscriber::fmt()
        .with_max_level(log_level)
        .init();

    tracing::info!("mqttaudio {} starting", env!("CARGO_PKG_VERSION"));
    tracing::info!("Copyright © 2016-2025 Mo Fang Heavy Industries LLC");

    // Handle --list-devices
    if args.list_devices {
        audio::engine::list_devices();
        return;
    }

    // Get device configuration
    match audio::engine::get_default_device_config() {
        Ok(config) => {
            tracing::info!(
                "Default device config: {} Hz, {} channels, {} frame buffer",
                config.sample_rate,
                config.channels,
                config.buffer_size
            );
        }
        Err(e) => {
            tracing::error!("Failed to get device config: {}", e);
            std::process::exit(1);
        }
    }

    // Handle --test-tone (Phase 1)
    if args.test_tone {
        tracing::info!("Starting test tone mode (440Hz sine wave)...");
        match audio::engine::init_test_sine_wave() {
            Ok(_stream) => {
                tracing::info!("Test tone playing. Press Ctrl+C to exit.");

                // Keep stream alive until Ctrl+C
                let (tx, rx) = std::sync::mpsc::channel();
                ctrlc::set_handler(move || {
                    tx.send(()).expect("Could not send signal on channel");
                })
                .expect("Error setting Ctrl-C handler");

                rx.recv().expect("Could not receive from channel");
                tracing::info!("Stopping test tone...");

                // Stream will be dropped here, stopping playback
            }
            Err(e) => {
                tracing::error!("Failed to initialize audio stream: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    // Handle --file (Phase 2)
    if let Some(file_path) = args.file {
        match audio::engine::play_file(&file_path) {
            Ok(_stream) => {
                tracing::info!("File playing. Press Ctrl+C to exit.");

                // Keep stream alive until Ctrl+C
                let (tx, rx) = std::sync::mpsc::channel();
                ctrlc::set_handler(move || {
                    tx.send(()).expect("Could not send signal on channel");
                })
                .expect("Error setting Ctrl-C handler");

                rx.recv().expect("Could not receive from channel");
                tracing::info!("Stopping playback...");

                // Stream will be dropped here, stopping playback
            }
            Err(e) => {
                tracing::error!("Failed to play file: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    // Handle --test-mixer (Phase 4)
    if args.test_mixer {
        tracing::info!("Starting mixer test (multiple simultaneous samples)...");
        match audio::engine::test_mixer() {
            Ok(_stream) => {
                tracing::info!("Mixer test running. Press Ctrl+C to exit.");

                // Keep stream alive until Ctrl+C
                let (tx, rx) = std::sync::mpsc::channel();
                ctrlc::set_handler(move || {
                    tx.send(()).expect("Could not send signal on channel");
                })
                .expect("Error setting Ctrl-C handler");

                rx.recv().expect("Could not receive from channel");
                tracing::info!("Stopping mixer test...");

                // Stream will be dropped here, stopping playback
            }
            Err(e) => {
                tracing::error!("Failed to start mixer test: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    // MQTT mode (Phase 5)
    if let (Some(server), Some(topic)) = (args.server, args.topic) {
        let port = args.port.unwrap_or(1883);

        tracing::info!("Starting MQTT mode");

        // Connect to MQTT broker
        let (_client, eventloop) = match mqtt::client::connect_mqtt(&server, port, &topic).await {
            Ok((c, el)) => (c, el),
            Err(e) => {
                tracing::error!("Failed to connect to MQTT broker: {}", e);
                std::process::exit(1);
            }
        };

        // Set up audio device
        let host = cpal::default_host();
        let device = match host.default_output_device() {
            Some(d) => d,
            None => {
                tracing::error!("No default output device available");
                std::process::exit(1);
            }
        };

        let config = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to get device config: {}", e);
                std::process::exit(1);
            }
        };

        let output_sample_rate = config.sample_rate().0;
        let output_channels = config.channels() as usize;

        tracing::info!("Audio device: {}", device.name().unwrap_or_else(|_| "Unknown".to_string()));
        tracing::info!("  Sample rate: {} Hz", output_sample_rate);
        tracing::info!("  Channels: {}", output_channels);

        // Create mixer state and voice manager
        use audio::mixer::{ActiveSample, MixerState};
        use std::sync::{Arc, Mutex};
        use voice::VoiceManager;

        let mixer_state = Arc::new(Mutex::new(MixerState {
            active_samples: Vec::new(),
            output_channels,
        }));

        let voice_manager = Arc::new(Mutex::new(VoiceManager::new()));

        let mixer_state_clone = mixer_state.clone();

        // Start audio stream
        let stream = device.build_output_stream(
            &config.into(),
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let mut state = mixer_state_clone.lock().unwrap();
                audio::mixer::mix_audio(data, &mut state);

                // Remove finished samples
                state.active_samples.retain(|s| !s.is_finished());
            },
            |err| {
                tracing::error!("Audio stream error: {}", err);
            },
            None,
        ).expect("Failed to build audio stream");

        stream.play().expect("Failed to start audio stream");
        tracing::info!("Audio stream started");

        // Create command channel
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(100);

        // Spawn MQTT event processor
        tokio::spawn(async move {
            mqtt::client::process_mqtt_events(eventloop, cmd_tx).await;
        });

        // Main command processing loop
        tracing::info!("Ready to receive MQTT commands on topic: {}", topic);

        while let Some(payload) = cmd_rx.recv().await {
            match mqtt::commands::parse_command(&payload) {
                Ok(cmd) => {
                    tracing::info!("Processing command: {:?}", cmd);

                    match cmd {
                        mqtt::commands::AudioCommand::Play { file, volume, voice } => {
                            // Decode file
                            match audio::decoder::decode_file(&file, Some(output_sample_rate)) {
                                Ok(buffer) => {
                                    // Use provided voice or auto-generate one
                                    let voice_id = voice.unwrap_or_else(|| {
                                        format!("_auto_{}", std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap()
                                            .as_millis())
                                    });

                                    tracing::info!(
                                        "Loaded {}: {} channels, {} frames ({:.2}s) [voice: {}]",
                                        file,
                                        buffer.channels,
                                        buffer.frames,
                                        buffer.frames as f32 / buffer.sample_rate as f32,
                                        voice_id
                                    );

                                    // Get sample ID and voice volume from voice manager
                                    let mut voice_mgr = voice_manager.lock().unwrap();
                                    let sample_id = voice_mgr.add_sample_to_voice(&voice_id);
                                    let voice_volume = voice_mgr.get_voice_volume(&voice_id).unwrap_or(1.0);
                                    drop(voice_mgr);

                                    // Add to mixer
                                    let sample = ActiveSample::new(
                                        sample_id,
                                        voice_id,
                                        Arc::new(buffer),
                                        volume,
                                        voice_volume,
                                    );
                                    let mut state = mixer_state.lock().unwrap();
                                    state.active_samples.push(sample);

                                    tracing::info!("Now playing {} active samples", state.active_samples.len());
                                }
                                Err(e) => {
                                    tracing::error!("Failed to load {}: {}", file, e);
                                }
                            }
                        }
                        mqtt::commands::AudioCommand::StopAll => {
                            let mut state = mixer_state.lock().unwrap();
                            let count = state.active_samples.len();
                            state.active_samples.clear();
                            drop(state);

                            // Note: We don't need to explicitly clear voices here.
                            // The audio callback cleanup will handle calling voice_manager.remove_sample()
                            // for each finished sample, which will auto-cleanup empty voices.

                            tracing::info!("Stopped {} samples", count);
                        }
                        mqtt::commands::AudioCommand::VoiceStop { voice } => {
                            // Get sample IDs to remove
                            let mut voice_mgr = voice_manager.lock().unwrap();
                            let sample_ids = voice_mgr.clear_voice(&voice);
                            drop(voice_mgr);

                            if sample_ids.is_empty() {
                                tracing::warn!("Voice '{}' not found or already empty", voice);
                            } else {
                                // Remove samples from mixer
                                let mut state = mixer_state.lock().unwrap();
                                let initial_count = state.active_samples.len();
                                state.active_samples.retain(|s| !sample_ids.contains(&s.id));
                                let removed = initial_count - state.active_samples.len();

                                tracing::info!("Stopped voice '{}': removed {} samples", voice, removed);
                            }
                        }
                        mqtt::commands::AudioCommand::VoiceFadeOut { voice, time_ms: _ } => {
                            // TODO: Implement fade out in Phase 9
                            // For now, just stop the voice immediately
                            tracing::warn!("Voice fade out not yet implemented (Phase 9), stopping voice '{}' immediately", voice);

                            let mut voice_mgr = voice_manager.lock().unwrap();
                            let sample_ids = voice_mgr.clear_voice(&voice);
                            drop(voice_mgr);

                            if !sample_ids.is_empty() {
                                let mut state = mixer_state.lock().unwrap();
                                state.active_samples.retain(|s| !sample_ids.contains(&s.id));
                                tracing::info!("Stopped voice '{}': {} samples", voice, sample_ids.len());
                            }
                        }
                        mqtt::commands::AudioCommand::VoiceVolume { voice, volume: new_volume } => {
                            // Set voice volume in voice manager
                            let mut voice_mgr = voice_manager.lock().unwrap();
                            let success = voice_mgr.set_voice_volume(&voice, new_volume);
                            let actual_volume = voice_mgr.get_voice_volume(&voice).unwrap_or(1.0);
                            drop(voice_mgr);

                            if success {
                                // Update all active samples in this voice
                                let mut state = mixer_state.lock().unwrap();
                                let mut updated_count = 0;
                                for sample in state.active_samples.iter_mut() {
                                    if sample.voice_id == voice {
                                        sample.voice_volume = actual_volume;
                                        updated_count += 1;
                                    }
                                }
                                drop(state);

                                tracing::info!(
                                    "Set voice '{}' volume to {:.2} (updated {} samples)",
                                    voice, actual_volume, updated_count
                                );
                            } else {
                                tracing::warn!("Voice '{}' not found", voice);
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to parse command: {}", e);
                }
            }
        }

        // Keep stream alive
        drop(stream);
        return;
    }

    // No mode specified - show help
    tracing::warn!("No mode specified. Use --help for options.");
    tracing::info!("Example: mqttaudio --server localhost --topic audio/commands");
}
