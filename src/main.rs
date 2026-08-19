// ABOUTME: Entry point for mqttaudio MQTT-controlled audio daemon.
// ABOUTME: Handles CLI parsing, initialization, and main event loop.

mod audio;
mod cache;
mod config;
mod http;
mod mqtt;
mod voice;

use clap::Parser;
use cpal::traits::{DeviceTrait, StreamTrait};
use tokio::sync::mpsc;
use tracing_subscriber;

#[derive(Parser, Debug)]
#[command(name = "mqttaudio")]
#[command(version = env!("CARGO_PKG_VERSION"))]
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

    /// Number of output channels (use max available if not specified)
    #[arg(short = 'n', long)]
    channels: Option<usize>,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// List available audio output devices and exit
    #[arg(long)]
    list_devices: bool,

    /// List available audio input devices and exit
    #[arg(long)]
    list_inputs: bool,

    /// Play a 440Hz test tone (for testing audio output)
    #[arg(long)]
    test_tone: bool,

    /// Play an audio file (for testing decoder)
    #[arg(long)]
    file: Option<String>,

    /// Test mixer with multiple simultaneous files (Phase 4)
    #[arg(long)]
    test_mixer: bool,

    /// LFE (subwoofer) channel number for bass management
    #[arg(long)]
    lfe_channel: Option<usize>,

    /// Crossover frequency (Hz) for bass management
    #[arg(long)]
    crossover_frequency: Option<f32>,

    /// MQTT topic to publish log messages to
    #[arg(long)]
    log_topic: Option<String>,

    /// MQTT broker username for authentication
    #[arg(long)]
    mqtt_username: Option<String>,

    /// MQTT broker password for authentication
    #[arg(long)]
    mqtt_password: Option<String>,

    /// Enable HTTP server on specified port (enables REST API and WebSocket)
    #[arg(long)]
    http_port: Option<u16>,

    /// Maximum memory cache size in MB (0 = unlimited)
    #[arg(long)]
    max_cache_mb: Option<u32>,
}

/// Report capture health for live inputs on a fixed interval.
///
/// Overruns, backlog trims and starvation are counted inside the audio
/// callbacks, which must not log or allocate; this reports the deltas from an
/// ordinary task so problems are visible without stalling the audio threads.
fn spawn_input_health_monitor(
    mixer_state: std::sync::Arc<std::sync::Mutex<audio::mixer::MixerState>>,
) {
    const INTERVAL_SECS: u64 = 10;

    tokio::spawn(async move {
        let mut previous: Vec<(u64, u64, u64)> = Vec::new();
        let mut ticker =
            tokio::time::interval(std::time::Duration::from_secs(INTERVAL_SECS));
        ticker.tick().await; // the first tick completes immediately

        loop {
            ticker.tick().await;

            let snapshot: Vec<(String, usize, u64, u64, u64)> = {
                let state = mixer_state.lock().unwrap();
                state
                    .live_inputs
                    .iter()
                    .map(|i| {
                        (
                            i.voice_id.clone(),
                            i.backlog_frames(),
                            i.dropped_frames(),
                            i.trimmed_frames,
                            i.underrun_frames,
                        )
                    })
                    .collect()
            };

            previous.resize(snapshot.len(), (0, 0, 0));

            for (idx, (voice_id, backlog, dropped, trimmed, underrun)) in
                snapshot.iter().enumerate()
            {
                let (prev_dropped, prev_trimmed, prev_underrun) = previous[idx];
                previous[idx] = (*dropped, *trimmed, *underrun);

                let new_dropped = dropped.saturating_sub(prev_dropped);
                let new_trimmed = trimmed.saturating_sub(prev_trimmed);
                let new_underrun = underrun.saturating_sub(prev_underrun);

                if new_dropped == 0 && new_trimmed == 0 && new_underrun == 0 {
                    continue;
                }

                tracing::warn!(
                    "Input {} ('{}'): {} frames overran, {} trimmed, {} starved in {}s (backlog {} frames)",
                    idx,
                    voice_id,
                    new_dropped,
                    new_trimmed,
                    new_underrun,
                    INTERVAL_SECS,
                    backlog
                );
            }
        }
    });
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Load configuration
    let mut config = match config::Config::load_from_path_or_default(args.config.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load configuration: {}", e);
            std::process::exit(1);
        }
    };

    // Merge CLI arguments into config (CLI overrides config file)
    config.merge_cli_args(
        args.server.clone(),
        args.port,
        args.topic.clone(),
        args.device.clone(),
        args.sample_rate,
        args.channels,
        args.verbose,
        args.lfe_channel,
        args.crossover_frequency,
        args.log_topic.clone(),
        args.mqtt_username.clone(),
        args.mqtt_password.clone(),
        args.http_port,
        args.max_cache_mb,
    );

    // Initialize logging based on config
    let log_level = match config.logging.level.as_str() {
        "error" => tracing::Level::ERROR,
        "warn" => tracing::Level::WARN,
        "info" => tracing::Level::INFO,
        "debug" => tracing::Level::DEBUG,
        "trace" => tracing::Level::TRACE,
        _ => tracing::Level::INFO,
    };

    // Create log channel for MQTT publishing (if configured)
    let mqtt_log_receiver = if config.logging.mqtt_topic.is_some() {
        let (sender, receiver) = mqtt::logger::create_log_channel(100);
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        use tracing_subscriber::Layer;

        let mqtt_layer = mqtt::logger::MqttLogLayer::new(sender, log_level);
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_filter(tracing_subscriber::filter::LevelFilter::from_level(log_level));

        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(mqtt_layer)
            .init();

        Some(receiver)
    } else {
        tracing_subscriber::fmt()
            .with_max_level(log_level)
            .init();
        None
    };

    tracing::info!("mqttaudio {} starting", env!("CARGO_PKG_VERSION"));
    tracing::info!("Copyright © 2016-2025 Mo Fang Heavy Industries LLC");

    // Handle --list-devices
    if args.list_devices {
        audio::engine::list_devices();
        return;
    }

    // Handle --list-inputs
    if args.list_inputs {
        audio::input::list_input_devices();
        return;
    }

    // Get default device configuration (informational only - not fatal if it fails)
    // On Linux/ALSA, the "default" device may not be usable, but a user-specified
    // device might work fine. We'll validate the actual device later.
    match audio::engine::get_default_device_config() {
        Ok(device_config) => {
            tracing::info!(
                "Default device config: {} Hz, {} channels, {} frame buffer",
                device_config.sample_rate,
                device_config.channels,
                device_config.buffer_size
            );
        }
        Err(e) => {
            // Only warn here - we'll fail later if the actual device can't be configured
            tracing::warn!("Could not probe default audio device: {}", e);
            if config.audio.device.is_some() {
                tracing::info!("Will attempt to use specified device: {:?}", config.audio.device);
            } else {
                tracing::warn!("No device specified and default device unavailable - audio may fail");
            }
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

    // Validate config
    if let Err(errors) = config.validate() {
        eprintln!("Configuration validation failed:");
        for error in errors {
            eprintln!("  - {}", error);
        }
        std::process::exit(1);
    }

    // Determine operating mode
    let mqtt_enabled = config.mqtt.topic.is_some();
    let http_enabled = config.http.enabled;

    if mqtt_enabled {
        tracing::info!("MQTT mode enabled");
        tracing::info!("  Server: {}:{}", config.mqtt.server, config.mqtt.port);
        tracing::info!("  Topic: {}", config.mqtt.topic.as_ref().unwrap());
    }
    if http_enabled {
        tracing::info!("HTTP mode enabled");
        tracing::info!("  Bind: {}:{}", config.http.bind_address, config.http.port);
    }

    // Connect to MQTT broker if enabled
    let mqtt_connection = if mqtt_enabled {
        let topic = config.mqtt.topic.as_ref().unwrap();
        match mqtt::client::connect_mqtt(
            &config.mqtt.server,
            config.mqtt.port,
            topic,
            config.mqtt.username.as_deref(),
            config.mqtt.password.as_deref(),
        ).await {
            Ok((c, el)) => Some((c, el)),
            Err(e) => {
                tracing::error!("Failed to connect to MQTT broker: {}", e);
                if !http_enabled {
                    // MQTT was the only mode, so we must exit
                    std::process::exit(1);
                }
                tracing::warn!("Continuing with HTTP-only mode");
                None
            }
        }
    } else {
        None
    };

    // Spawn MQTT log publisher if configured and connected
    let _log_publisher_handle = if let Some((ref client, _)) = mqtt_connection {
        let client = std::sync::Arc::new(client.clone());
        if let Some(receiver) = mqtt_log_receiver {
            if let Some(ref log_topic) = config.logging.mqtt_topic {
                tracing::info!("MQTT log publishing enabled on topic: {}", log_topic);
                Some(mqtt::logger::spawn_log_publisher(
                    client,
                    log_topic.clone(),
                    receiver,
                ))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // Set up audio device
    let device = match audio::engine::find_output_device(config.audio.device.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Failed to find output device: {}", e);
            std::process::exit(1);
        }
    };

    let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());
    let stream_config = match audio::engine::find_output_config(
        &device,
        config.audio.channels,
        Some(config.audio.sample_rate),
    ) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(
                "Failed to configure output device '{}': {}",
                device_name, e
            );
            tracing::error!(
                "Requested: {} channels, {} Hz sample rate",
                config.audio.channels.map_or("default".to_string(), |c| c.to_string()),
                config.audio.sample_rate
            );
            std::process::exit(1);
        }
    };

    let output_sample_rate = stream_config.sample_rate.0;
    let output_channels = stream_config.channels as usize;

    tracing::info!("Audio device: {}", device_name);
    tracing::info!("  Sample rate: {} Hz", output_sample_rate);
    tracing::info!("  Channels: {}", output_channels);

    // Create mixer state and voice manager
    use audio::mixer::{ActiveSample, MixerState};
    use audio::ducking::DuckingEngine;
    use std::sync::{Arc, Mutex};
    use std::collections::HashSet;
    use voice::VoiceManager;

    // Create ducking engine from config rules
    let ducking_engine = if !config.ducking_rules.is_empty() {
        Some(DuckingEngine::new(config.ducking_rules.clone(), output_sample_rate))
    } else {
        None
    };

    // Create bass management from config
    let bass_management = if config.bass_management.enabled {
        let resolved = config.resolve_bass_management()
            .expect("Channel alias resolution failed (should have been caught during validation)");
        let bm_config = audio::bass_management::BassManagementConfig {
            enabled: resolved.enabled,
            lfe_channel: resolved.lfe_channel,
            crossover_frequency_hz: resolved.crossover_frequency_hz,
            source_channels: resolved.source_channels.clone(),
            remove_bass_from_sources: resolved.remove_bass_from_sources,
        };
        tracing::info!(
            "Bass management enabled: LFE channel {}, crossover {} Hz, sources {:?}",
            bm_config.lfe_channel,
            bm_config.crossover_frequency_hz,
            bm_config.source_channels
        );
        Some(audio::bass_management::BassManagement::new(bm_config, output_sample_rate, output_channels))
    } else {
        None
    };

    let mixer_state = Arc::new(Mutex::new(MixerState {
        active_samples: Vec::new(),
        live_inputs: Vec::new(),
        output_channels,
        ducking_engine,
        bass_management,
    }));

    // Initialize audio inputs from config
    // Keep active input streams alive - they will be kept alive until the app exits
    let mut _active_inputs: Vec<audio::input::ActiveInput> = Vec::new();
    for (idx, input_config) in config.inputs.iter().enumerate() {
        // Resolve routing first: it determines how many capture channels the
        // stream has to be opened with.
        let channel_map: Vec<(usize, usize)> = config.resolve_input_routes(&input_config.routes)
            .expect("Channel alias resolution failed (should have been caught during validation)");

        let stream_config = audio::input::InputStreamConfig {
            device_name: input_config.device.clone(),
            latency_ms: input_config.latency_ms,
            channels: input_config.channels,
            min_channels: audio::input::required_channels(&channel_map),
            sample_rate: input_config.sample_rate,
        };

        match audio::input::create_input_stream(stream_config, output_sample_rate) {
            Ok(mut active_input) => {
                tracing::info!(
                    "Opened input device: {} ({} channels, {} Hz, voice '{}')",
                    input_config.device.as_deref().unwrap_or("default"),
                    active_input.channels,
                    active_input.sample_rate,
                    input_config.voice_id
                );

                // Take ownership of the consumer for the mixer
                if let Some(consumer) = active_input.take_consumer() {
                    tracing::info!(
                        "Input {} routed: {:?}",
                        idx,
                        channel_map.iter()
                            .map(|(src, dest)| format!("{}→{}", src, dest))
                            .collect::<Vec<_>>()
                    );

                    // Create LiveInput for the mixer
                    let live_input = audio::mixer::LiveInput::new(
                        input_config.voice_id.clone(),
                        consumer,
                        active_input.channels,
                        input_config.volume,
                        channel_map,
                    ).with_dropped_frames(active_input.dropped_frames.clone());

                    // Add to mixer state
                    mixer_state.lock().unwrap().live_inputs.push(live_input);
                }

                // Keep the stream alive by storing it
                _active_inputs.push(active_input);
            }
            Err(e) => {
                tracing::error!(
                    "Failed to open input device '{}': {}",
                    input_config.device.as_deref().unwrap_or("default"),
                    e
                );
            }
        }
    }

    if !mixer_state.lock().unwrap().live_inputs.is_empty() {
        spawn_input_health_monitor(mixer_state.clone());
    }

    let voice_manager = Arc::new(Mutex::new(VoiceManager::new()));

    // Track active voices for ducking notifications
    let active_voices: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

    // Create cache manager using config
    let cache_dir = config.cache_directory();
    let resampler_quality = config.advanced.resampler_quality;
    let max_memory_mb = config.cache.max_memory_mb;
    tracing::info!("Cache directory: {}", cache_dir.display());
    tracing::info!("Resampler quality: {:?}", resampler_quality);

    let cache_manager = match cache::CacheManager::with_options(cache_dir, resampler_quality, max_memory_mb) {
        Ok(cm) => Arc::new(Mutex::new(cm)),
        Err(e) => {
            tracing::error!("Failed to initialize cache: {}", e);
            std::process::exit(1);
        }
    };

    // Precache files from config on startup (expands directories to audio files)
    let precache_files = config.expand_precache_entries();
    if !precache_files.is_empty() {
        if config.cache.precache_blocking {
            tracing::info!("Precaching {} files (blocking)...", precache_files.len());
            for file_path in &precache_files {
                let mut cache_mgr = cache_manager.lock().unwrap();
                match cache_mgr.precache(file_path, output_sample_rate).await {
                    Ok(()) => {
                        if config.logging.verbose {
                            cache_mgr.log_stats();
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to precache {}: {}", file_path, e);
                    }
                }
                drop(cache_mgr);
            }
            tracing::info!("Startup precaching complete");
        } else {
            tracing::info!("Starting background precache for {} files (non-blocking)...", precache_files.len());
            for file_path in &precache_files {
                let mut cache_mgr = cache_manager.lock().unwrap();
                match cache_mgr.precache_streaming(file_path, output_sample_rate).await {
                    Ok(()) => {}
                    Err(e) => {
                        tracing::error!("Failed to start precache for {}: {}", file_path, e);
                    }
                }
                drop(cache_mgr);
            }
            tracing::info!("Background precache initiated (files loading asynchronously)");
        }
    }

    let mixer_state_clone = mixer_state.clone();
    let active_voices_clone = active_voices.clone();

    // Start audio stream
    let stream = device.build_output_stream(
        &stream_config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let mut state = mixer_state_clone.lock().unwrap();
                audio::mixer::mix_audio(data, &mut state);

                // Track which voices had samples before cleanup
                let voices_before: HashSet<String> = state.active_samples.iter()
                    .map(|s| s.voice_id.clone())
                    .collect();

                // Remove finished samples
                state.active_samples.retain(|s| !s.is_finished());

                // Track which voices have samples after cleanup
                let voices_after: HashSet<String> = state.active_samples.iter()
                    .map(|s| s.voice_id.clone())
                    .collect();

                // Notify ducking engine of voices that became inactive
                if let Some(ref mut engine) = state.ducking_engine {
                    for voice in voices_before.difference(&voices_after) {
                        engine.notify_voice_active(voice, false);
                    }
                }

                // Update active voices tracker
                let mut active = active_voices_clone.lock().unwrap();
                *active = voices_after;
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

        // Start HTTP server if enabled
        if config.http.enabled {
            let http_cmd_tx = cmd_tx.clone();
            match http::start_server(
                &config.http,
                http_cmd_tx,
                mixer_state.clone(),
                voice_manager.clone(),
                cache_manager.clone(),
            ).await {
                Ok(addr) => {
                    tracing::info!("HTTP REST API available at http://{}", addr);
                    if config.http.websocket_enabled {
                        tracing::info!("WebSocket logs available at ws://{}/ws", addr);
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to start HTTP server: {}", e);
                    // Continue without HTTP server - not fatal
                }
            }
        }

        // Spawn MQTT event processor if connected
        if let Some((_, eventloop)) = mqtt_connection {
            let mqtt_cmd_tx = cmd_tx.clone();
            tokio::spawn(async move {
                mqtt::client::process_mqtt_events(eventloop, mqtt_cmd_tx).await;
            });
            tracing::info!("Ready to receive MQTT commands on topic: {}", config.mqtt.topic.as_ref().unwrap());
        }

        // Keep cmd_tx alive if only HTTP is running (no MQTT)
        let _cmd_tx_keepalive = cmd_tx;

        // Main command processing loop
        if mqtt_enabled || http_enabled {
            tracing::info!("Command processing loop started");
        }

        while let Some(payload) = cmd_rx.recv().await {
            // Expand macros before parsing
            let expanded = match mqtt::commands::expand_macros(&payload, &config.macros) {
                Ok(e) => e,
                Err(e) => {
                    tracing::error!("Macro expansion error: {}", e);
                    continue;
                }
            };

            match mqtt::commands::parse_command(&expanded) {
                Ok(cmd) => {
                    tracing::info!("Processing command: {:?}", cmd);

                    match cmd {
                        mqtt::commands::AudioCommand::Play { file, id, volume, voice, channel_map, fade_in, start_position_ms, loop_mode, crossfade_ms } => {
                            // Load file (with streaming support for faster startup)
                            let mut cache_mgr = cache_manager.lock().unwrap();
                            let buffer_result = cache_mgr.get_or_load_streaming(&file, output_sample_rate).await;
                            drop(cache_mgr);

                            match buffer_result {
                                Ok(buffer) => {
                                    // Use provided voice or auto-generate one
                                    let voice_id = voice.unwrap_or_else(|| {
                                        format!("_auto_{}", std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap()
                                            .as_millis())
                                    });

                                    let is_streaming = !buffer.is_complete();
                                    let frames = buffer.frames();
                                    let sample_rate = buffer.sample_rate();
                                    let channels = buffer.channels();
                                    let duration_secs = if frames > 0 && sample_rate > 0 {
                                        frames as f32 / sample_rate as f32
                                    } else {
                                        0.0
                                    };

                                    if is_streaming {
                                        tracing::info!(
                                            "Playing {} (streaming): {} channels, {} frames loaded so far [voice: {}{}{}]",
                                            file,
                                            channels,
                                            frames,
                                            voice_id,
                                            if loop_mode { ", looping" } else { "" },
                                            if crossfade_ms > 0 { format!(", crossfade {}ms", crossfade_ms) } else { String::new() }
                                        );
                                    } else {
                                        tracing::info!(
                                            "Playing {}: {} channels, {} frames ({:.2}s) [voice: {}{}{}]",
                                            file,
                                            channels,
                                            frames,
                                            duration_secs,
                                            voice_id,
                                            if loop_mode { ", looping" } else { "" },
                                            if crossfade_ms > 0 { format!(", crossfade {}ms", crossfade_ms) } else { String::new() }
                                        );
                                    }

                                    // Get sample ID and voice volume from voice manager
                                    let mut voice_mgr = voice_manager.lock().unwrap();
                                    let sample_id = voice_mgr.add_sample_to_voice(&voice_id);
                                    let voice_volume = voice_mgr.get_voice_volume(&voice_id).unwrap_or(1.0);
                                    drop(voice_mgr);

                                    // Convert crossfade_ms to samples
                                    let crossfade_samples = (crossfade_ms as usize * output_sample_rate as usize) / 1000;

                                    // Convert channel_map to mixer format (resolve any aliases)
                                    let mut sample = if let Some(map) = channel_map {
                                        // Custom channel mapping - resolve aliases
                                        let mapping_result: Result<Vec<(usize, usize)>, String> = map.iter()
                                            .map(|m| {
                                                let src = config.resolve_channel(&m.src)?;
                                                let dest = config.resolve_channel(&m.dest)?;
                                                Ok((src, dest))
                                            })
                                            .collect();

                                        let mapping = match mapping_result {
                                            Ok(m) => m,
                                            Err(e) => {
                                                tracing::error!("Failed to resolve channel alias: {}", e);
                                                continue;
                                            }
                                        };
                                        tracing::debug!("Using custom channel mapping: {:?}", mapping);
                                        ActiveSample::new_with_mapping(
                                            sample_id,
                                            voice_id.clone(),
                                            buffer.clone(),
                                            volume,
                                            voice_volume,
                                            mapping,
                                            file.clone(),
                                            id.clone(),
                                            loop_mode,
                                            crossfade_samples,
                                        )
                                    } else {
                                        // Default channel mapping (1:1)
                                        ActiveSample::new_with_id(
                                            sample_id,
                                            voice_id.clone(),
                                            buffer.clone(),
                                            volume,
                                            voice_volume,
                                            file.clone(),
                                            id.clone(),
                                            loop_mode,
                                            crossfade_samples,
                                        )
                                    };

                                    // Apply start position if requested
                                    if let Some(start_ms) = start_position_ms {
                                        let target_frame = ((start_ms * sample_rate as u64) / 1000) as usize;
                                        // For streaming buffers, use estimate if available, otherwise don't clamp
                                        // (mixer will return silence for unloaded frames)
                                        let max_frame = buffer.total_frames_or_estimate()
                                            .unwrap_or(usize::MAX)
                                            .saturating_sub(1);
                                        sample.position = target_frame.min(max_frame);
                                        if is_streaming && !buffer.is_frame_loaded(sample.position) {
                                            tracing::debug!(
                                                "Starting at position {}ms (frame {}), waiting for data to load",
                                                start_ms, sample.position
                                            );
                                        } else {
                                            tracing::debug!("Starting at position {}ms (frame {})", start_ms, sample.position);
                                        }
                                    }

                                    // Apply fade in if requested
                                    if let Some(fade_ms) = fade_in {
                                        sample.set_fade(audio::mixer::FadeState::fade_in(fade_ms, output_sample_rate));
                                        tracing::debug!("Applied {}ms fade in to voice '{}'", fade_ms, voice_id);
                                    }

                                    // Notify ducking engine if voice became active
                                    let mut active_voices_guard = active_voices.lock().unwrap();
                                    let was_active = active_voices_guard.contains(&voice_id);
                                    if !was_active {
                                        active_voices_guard.insert(voice_id.clone());
                                        drop(active_voices_guard);

                                        // Notify ducking engine that voice became active
                                        let mut state = mixer_state.lock().unwrap();
                                        if let Some(ref mut engine) = state.ducking_engine {
                                            engine.notify_voice_active(&voice_id, true);
                                        }
                                        state.active_samples.push(sample);
                                        tracing::info!("Now playing {} active samples", state.active_samples.len());
                                    } else {
                                        drop(active_voices_guard);
                                        let mut state = mixer_state.lock().unwrap();
                                        state.active_samples.push(sample);
                                        tracing::info!("Now playing {} active samples", state.active_samples.len());
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("Failed to load {}: {}", file, e);
                                }
                            }
                        }
                        mqtt::commands::AudioCommand::StopAll => {
                            // Apply quick 10ms fade-out to prevent clicks/pops
                            const STOP_FADE_MS: u32 = 10;

                            let mut state = mixer_state.lock().unwrap();
                            let count = state.active_samples.len();

                            for sample in state.active_samples.iter_mut() {
                                sample.set_fade(audio::mixer::FadeState::fade_out(STOP_FADE_MS, output_sample_rate));
                            }
                            drop(state);

                            // Note: Samples will be automatically removed by the audio callback
                            // when the fade completes (is_finished() returns true).
                            // Voice cleanup also happens automatically.

                            tracing::info!("Stopping {} samples ({}ms fade-out)", count, STOP_FADE_MS);
                        }
                        mqtt::commands::AudioCommand::VoiceStop { voice } => {
                            // Apply quick 10ms fade-out to prevent clicks/pops
                            const STOP_FADE_MS: u32 = 10;

                            // Get sample IDs in the voice
                            let mut voice_mgr = voice_manager.lock().unwrap();
                            let sample_ids = voice_mgr.clear_voice(&voice);
                            drop(voice_mgr);

                            if sample_ids.is_empty() {
                                tracing::warn!("Voice '{}' not found or already empty", voice);
                            } else {
                                // Apply fade-out to all samples in the voice
                                let mut state = mixer_state.lock().unwrap();
                                let mut updated_count = 0;
                                for sample in state.active_samples.iter_mut() {
                                    if sample_ids.contains(&sample.id) {
                                        sample.set_fade(audio::mixer::FadeState::fade_out(STOP_FADE_MS, output_sample_rate));
                                        updated_count += 1;
                                    }
                                }
                                drop(state);

                                tracing::info!(
                                    "Stopping voice '{}': {} samples ({}ms fade-out)",
                                    voice, updated_count, STOP_FADE_MS
                                );
                            }
                        }
                        mqtt::commands::AudioCommand::VoiceFadeOut { voice, time_ms } => {
                            // Get sample IDs in the voice
                            let voice_mgr = voice_manager.lock().unwrap();
                            let sample_ids = voice_mgr.get_voice_sample_ids(&voice);
                            drop(voice_mgr);

                            if sample_ids.is_empty() {
                                tracing::warn!("Voice '{}' not found or already empty", voice);
                            } else {
                                // Apply fade out to all samples in the voice
                                let mut state = mixer_state.lock().unwrap();
                                let mut updated_count = 0;
                                for sample in state.active_samples.iter_mut() {
                                    if sample_ids.contains(&sample.id) {
                                        sample.set_fade(audio::mixer::FadeState::fade_out(time_ms, output_sample_rate));
                                        updated_count += 1;
                                    }
                                }
                                drop(state);

                                tracing::info!(
                                    "Applied {}ms fade out to voice '{}' ({} samples)",
                                    time_ms, voice, updated_count
                                );
                            }
                        }
                        mqtt::commands::AudioCommand::VoiceVolume { voice, volume: new_volume } => {
                            // Set voice volume in voice manager
                            let mut voice_mgr = voice_manager.lock().unwrap();
                            let success = voice_mgr.set_voice_volume(&voice, new_volume);
                            let actual_volume = voice_mgr.get_voice_volume(&voice).unwrap_or(1.0);
                            drop(voice_mgr);

                            if success {
                                // Update all active samples and live inputs in this voice with smooth ramping
                                let mut state = mixer_state.lock().unwrap();
                                let mut sample_count = 0;
                                let mut input_count = 0;

                                for sample in state.active_samples.iter_mut() {
                                    if sample.voice_id == voice {
                                        // Set target for smooth ramping (avoids pops)
                                        sample.set_target_voice_volume(actual_volume);
                                        sample_count += 1;
                                    }
                                }

                                for input in state.live_inputs.iter_mut() {
                                    if input.voice_id == voice {
                                        // Set target for smooth ramping (avoids pops)
                                        input.set_target_voice_volume(actual_volume);
                                        input_count += 1;
                                    }
                                }
                                drop(state);

                                tracing::info!(
                                    "Set voice '{}' volume to {:.2} (updated {} samples, {} inputs)",
                                    voice, actual_volume, sample_count, input_count
                                );
                            } else {
                                tracing::warn!("Voice '{}' not found", voice);
                            }
                        }
                        mqtt::commands::AudioCommand::Precache { file } => {
                            // Non-blocking precache - starts loading and returns immediately
                            let mut cache_mgr = cache_manager.lock().unwrap();
                            match cache_mgr.precache_streaming(&file, output_sample_rate).await {
                                Ok(()) => {
                                    if config.logging.verbose {
                                        cache_mgr.log_stats();
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("Failed to start precache for {}: {}", file, e);
                                }
                            }
                        }
                        mqtt::commands::AudioCommand::CacheClear => {
                            let mut cache_mgr = cache_manager.lock().unwrap();
                            if config.logging.verbose {
                                let mem = cache_mgr.memory_stats();
                                let disk = cache_mgr.disk_stats();
                                tracing::debug!(
                                    "Clearing cache - Memory: {} entries ({:.2} MB), Disk: {} entries ({:.2} MB)",
                                    mem.entry_count,
                                    mem.size_bytes as f64 / (1024.0 * 1024.0),
                                    disk.entry_count,
                                    disk.size_bytes as f64 / (1024.0 * 1024.0)
                                );
                            }
                            match cache_mgr.clear_all() {
                                Ok(()) => {
                                    tracing::info!("Cache cleared successfully");
                                }
                                Err(e) => {
                                    tracing::error!("Failed to clear cache: {}", e);
                                }
                            }
                        }
                        mqtt::commands::AudioCommand::CacheInvalidate { file } => {
                            let mut cache_mgr = cache_manager.lock().unwrap();
                            match cache_mgr.invalidate(&file) {
                                Ok(()) => {
                                    tracing::info!("Invalidated cache for: {}", file);
                                    if config.logging.verbose {
                                        cache_mgr.log_stats();
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("Failed to invalidate {}: {}", file, e);
                                }
                            }
                        }
                        mqtt::commands::AudioCommand::InputVolume { input, volume: new_volume } => {
                            let mut state = mixer_state.lock().unwrap();
                            let mut found = false;

                            // Try to find by index first
                            if let Ok(idx) = input.parse::<usize>() {
                                if idx < state.live_inputs.len() {
                                    state.live_inputs[idx].volume = new_volume.clamp(0.0, 1.0);
                                    tracing::info!(
                                        "Set input {} volume to {:.2}",
                                        idx, state.live_inputs[idx].volume
                                    );
                                    found = true;
                                }
                            }

                            // Try to find by voice_id if not found by index
                            if !found {
                                for live_input in state.live_inputs.iter_mut() {
                                    if live_input.voice_id == input {
                                        live_input.volume = new_volume.clamp(0.0, 1.0);
                                        tracing::info!(
                                            "Set input '{}' volume to {:.2}",
                                            input, live_input.volume
                                        );
                                        found = true;
                                        break;
                                    }
                                }
                            }

                            if !found {
                                tracing::warn!("Input '{}' not found", input);
                            }
                        }
                        mqtt::commands::AudioCommand::InputMute { input, mute } => {
                            let mut state = mixer_state.lock().unwrap();
                            let mut found = false;

                            // Try to find by index first
                            if let Ok(idx) = input.parse::<usize>() {
                                if idx < state.live_inputs.len() {
                                    // Mute by setting volume to 0, unmute restores to 1.0
                                    // Note: This is a simple mute - a more sophisticated version
                                    // would store the previous volume
                                    state.live_inputs[idx].volume = if mute { 0.0 } else { 1.0 };
                                    tracing::info!(
                                        "Input {} {}",
                                        idx, if mute { "muted" } else { "unmuted" }
                                    );
                                    found = true;
                                }
                            }

                            // Try to find by voice_id if not found by index
                            if !found {
                                for live_input in state.live_inputs.iter_mut() {
                                    if live_input.voice_id == input {
                                        live_input.volume = if mute { 0.0 } else { 1.0 };
                                        tracing::info!(
                                            "Input '{}' {}",
                                            input, if mute { "muted" } else { "unmuted" }
                                        );
                                        found = true;
                                        break;
                                    }
                                }
                            }

                            if !found {
                                tracing::warn!("Input '{}' not found", input);
                            }
                        }
                        mqtt::commands::AudioCommand::Seek { selector, position_ms } => {
                            if selector.is_empty() {
                                tracing::warn!("Seek command with empty selector - no samples targeted");
                            } else {
                                let mut state = mixer_state.lock().unwrap();
                                let mut updated_count = 0;

                                for sample in state.active_samples.iter_mut() {
                                    if selector.matches(
                                        sample.id,
                                        sample.sample_id.as_deref(),
                                        &sample.file_path,
                                        &sample.voice_id,
                                    ) {
                                        // Convert milliseconds to frames
                                        let target_frame = ((position_ms as u64 * sample.buffer.sample_rate() as u64) / 1000) as usize;
                                        // Clamp to buffer bounds
                                        sample.position = target_frame.min(sample.buffer.frames().saturating_sub(1));
                                        updated_count += 1;
                                    }
                                }
                                drop(state);

                                if updated_count > 0 {
                                    tracing::info!(
                                        "Seeked {} samples to {}ms",
                                        updated_count, position_ms
                                    );
                                } else {
                                    tracing::warn!("Seek command matched no active samples");
                                }
                            }
                        }
                        mqtt::commands::AudioCommand::Speed { selector, speed, pitch_correction } => {
                            if selector.is_empty() {
                                tracing::warn!("Speed command with empty selector - no samples targeted");
                            } else {
                                let mut state = mixer_state.lock().unwrap();
                                let mut updated_count = 0;

                                for sample in state.active_samples.iter_mut() {
                                    if selector.matches(
                                        sample.id,
                                        sample.sample_id.as_deref(),
                                        &sample.file_path,
                                        &sample.voice_id,
                                    ) {
                                        sample.set_speed_with_mode(speed, pitch_correction);
                                        updated_count += 1;
                                    }
                                }
                                drop(state);

                                if updated_count > 0 {
                                    let mode = if pitch_correction { "pitch-corrected" } else { "normal" };
                                    tracing::info!(
                                        "Set speed to {}x ({}) for {} samples",
                                        speed, mode, updated_count
                                    );
                                } else {
                                    tracing::warn!("Speed command matched no active samples");
                                }
                            }
                        }
                        mqtt::commands::AudioCommand::Stop { selector, fade_out_ms } => {
                            if selector.is_empty() {
                                tracing::warn!("Stop command with empty selector - no samples targeted");
                            } else {
                                // Default to 10ms fade for smooth stop if not specified
                                let fade_ms = fade_out_ms.unwrap_or(10);

                                let mut state = mixer_state.lock().unwrap();
                                let mut updated_count = 0;

                                for sample in state.active_samples.iter_mut() {
                                    if selector.matches(
                                        sample.id,
                                        sample.sample_id.as_deref(),
                                        &sample.file_path,
                                        &sample.voice_id,
                                    ) {
                                        sample.set_fade(audio::mixer::FadeState::fade_out(fade_ms, output_sample_rate));
                                        updated_count += 1;
                                    }
                                }
                                drop(state);

                                if updated_count > 0 {
                                    tracing::info!(
                                        "Stopping {} samples ({}ms fade-out)",
                                        updated_count, fade_ms
                                    );
                                } else {
                                    tracing::warn!("Stop command matched no active samples");
                                }
                            }
                        }
                        mqtt::commands::AudioCommand::Volume { selector, volume } => {
                            if selector.is_empty() {
                                tracing::warn!("Volume command with empty selector - no samples targeted");
                            } else {
                                let mut state = mixer_state.lock().unwrap();
                                let mut updated_count = 0;

                                for sample in state.active_samples.iter_mut() {
                                    if selector.matches(
                                        sample.id,
                                        sample.sample_id.as_deref(),
                                        &sample.file_path,
                                        &sample.voice_id,
                                    ) {
                                        sample.volume = volume.clamp(0.0, 1.0);
                                        updated_count += 1;
                                    }
                                }
                                drop(state);

                                if updated_count > 0 {
                                    tracing::info!(
                                        "Set volume to {:.2} for {} samples",
                                        volume, updated_count
                                    );
                                } else {
                                    tracing::warn!("Volume command matched no active samples");
                                }
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
}
