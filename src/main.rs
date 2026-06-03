// ABOUTME: Entry point for mqttaudio MQTT-controlled audio daemon.
// ABOUTME: Handles CLI parsing, initialization, and main event loop.

mod audio;
mod cache;
mod config;
mod http;
mod mqtt;
mod voice;

use clap::Parser;
use cpal::traits::DeviceTrait;
use tokio::sync::mpsc;

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
        let fmt_layer = tracing_subscriber::fmt::layer().with_filter(
            tracing_subscriber::filter::LevelFilter::from_level(log_level),
        );

        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(mqtt_layer)
            .init();

        Some(receiver)
    } else {
        tracing_subscriber::fmt().with_max_level(log_level).init();
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
                tracing::info!(
                    "Will attempt to use specified device: {:?}",
                    config.audio.device
                );
            } else {
                tracing::warn!(
                    "No device specified and default device unavailable - audio may fail"
                );
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
        match mqtt::client::connect_mqtt(&config.mqtt, topic).await {
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
    let output_config = match audio::engine::find_output_config(
        &device,
        config.audio.channels,
        Some(config.audio.sample_rate),
        config.audio.buffer_size,
    ) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to configure output device '{}': {}", device_name, e);
            tracing::error!(
                "Requested: {} channels, {} Hz sample rate",
                config
                    .audio
                    .channels
                    .map_or("default".to_string(), |c| c.to_string()),
                config.audio.sample_rate
            );
            std::process::exit(1);
        }
    };

    let stream_config = output_config.stream_config;
    let sample_format = output_config.sample_format;
    let output_sample_rate = stream_config.sample_rate.0;
    let output_channels = stream_config.channels as usize;

    tracing::info!("Audio device: {}", device_name);
    tracing::info!("  Sample rate: {} Hz", output_sample_rate);
    tracing::info!("  Channels: {}", output_channels);
    tracing::info!("  Sample format: {:?}", sample_format);

    // Create mixer state and voice manager
    use audio::ducking::DuckingEngine;
    use audio::mixer::MixerState;
    use parking_lot::Mutex;
    use std::collections::HashSet;
    use std::sync::Arc;
    use voice::VoiceManager;

    // Create ducking engine from config rules
    let ducking_engine = if !config.ducking_rules.is_empty() {
        Some(DuckingEngine::new(
            config.ducking_rules.clone(),
            output_sample_rate,
        ))
    } else {
        None
    };

    // Create bass management from config
    let bass_management = if config.bass_management.enabled {
        let resolved = match config.resolve_bass_management() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Bass management channel resolution failed: {}", e);
                std::process::exit(1);
            }
        };
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
        Some(audio::bass_management::BassManagement::new(
            bm_config,
            output_sample_rate,
            output_channels,
        ))
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
        let stream_config = audio::input::InputStreamConfig {
            device_name: input_config.device.clone(),
            latency_ms: input_config.latency_ms,
        };

        match audio::input::create_input_stream(stream_config, output_sample_rate) {
            Ok(mut active_input) => {
                tracing::info!(
                    "Opened input device: {} ({} channels, voice '{}')",
                    input_config.device.as_deref().unwrap_or("default"),
                    active_input.channels,
                    input_config.voice_id
                );

                // Take ownership of the consumer for the mixer
                if let Some(consumer) = active_input.take_consumer() {
                    // Build channel map from routes config (resolve aliases)
                    let channel_map: Vec<(usize, usize)> =
                        match config.resolve_input_routes(&input_config.routes) {
                            Ok(m) => m,
                            Err(e) => {
                                tracing::error!("Input route channel resolution failed: {}", e);
                                std::process::exit(1);
                            }
                        };

                    tracing::info!(
                        "Input {} routed: {:?}",
                        idx,
                        channel_map
                            .iter()
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
                    );

                    // Add to mixer state
                    mixer_state.lock().live_inputs.push(live_input);
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

    let voice_manager = Arc::new(Mutex::new(VoiceManager::new()));

    // Track active voices for ducking notifications
    let active_voices: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

    // Create cache manager using config
    let cache_dir = config.cache_directory();
    let resampler_quality = config.advanced.resampler_quality;
    let max_memory_mb = config.cache.max_memory_mb;
    tracing::info!("Cache directory: {}", cache_dir.display());
    tracing::info!("Resampler quality: {:?}", resampler_quality);

    if config.security.allowed_directories.is_empty() {
        tracing::warn!(
            "No security.allowed_directories configured: any local file path in a Play command \
             will be opened (open-by-default). Set security.allowed_directories to restrict access."
        );
    } else {
        tracing::info!(
            "Local file access restricted to {} allowed director{}",
            config.security.allowed_directories.len(),
            if config.security.allowed_directories.len() == 1 {
                "y"
            } else {
                "ies"
            }
        );
    }

    let cache_manager = match cache::CacheManager::with_options(
        cache_dir,
        resampler_quality,
        max_memory_mb,
        config.security.allowed_directories.clone(),
        config.cache.revalidate_after_seconds,
    ) {
        Ok(cm) => Arc::new(tokio::sync::Mutex::new(cm)),
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
                let mut cache_mgr = cache_manager.lock().await;
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
            tracing::info!(
                "Starting background precache for {} files (non-blocking)...",
                precache_files.len()
            );
            for file_path in &precache_files {
                let mut cache_mgr = cache_manager.lock().await;
                match cache_mgr
                    .precache_streaming(file_path, output_sample_rate)
                    .await
                {
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

    // Start audio stream — build a typed stream that matches the device's
    // native sample format and convert the f32 mix bus to it per sample.
    // Supervise the output stream on its own thread so a fatal device error
    // rebuilds it (with backoff) instead of going permanently silent. The
    // supervisor re-resolves the device by name and reuses this negotiated
    // config, exiting for a service-manager restart if it can't recover.
    let _output_supervisor = audio::engine::spawn_output_supervisor(
        config.audio.device.clone(),
        stream_config,
        sample_format,
        mixer_state.clone(),
        active_voices.clone(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );

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
        )
        .await
        {
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
    if let Some((client, eventloop)) = mqtt_connection {
        let mqtt_cmd_tx = cmd_tx.clone();
        let mqtt_topic = config.mqtt.topic.clone().unwrap_or_default();
        tokio::spawn(async move {
            mqtt::client::process_mqtt_events(client, mqtt_topic, eventloop, mqtt_cmd_tx).await;
        });
        tracing::info!(
            "Ready to receive MQTT commands on topic: {}",
            config.mqtt.topic.as_ref().unwrap()
        );
    }

    // Keep cmd_tx alive if only HTTP is running (no MQTT)
    let _cmd_tx_keepalive = cmd_tx;

    // Main command processing loop
    if mqtt_enabled || http_enabled {
        tracing::info!("Command processing loop started");
    }

    let command_ctx = CommandCtx {
        cache_manager: &cache_manager,
        voice_manager: &voice_manager,
        mixer_state: &mixer_state,
        active_voices: &active_voices,
        output_sample_rate,
        config: &config,
    };

    loop {
        tokio::select! {
            maybe_payload = cmd_rx.recv() => {
                let payload = match maybe_payload {
                    Some(p) => p,
                    None => break, // command channel closed
                };

                // Expand macros before parsing
                let expanded = match mqtt::commands::expand_macros(&payload, &config.macros) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::error!("Macro expansion error: {}", e);
                        continue;
                    }
                };

                match mqtt::commands::parse_command(&expanded) {
                    Ok(cmd) => handle_command(cmd, &command_ctx).await,
                    Err(e) => {
                        tracing::error!("Failed to parse command: {}", e);
                    }
                }
            }
            _ = shutdown_signal() => {
                tracing::info!("Shutdown signal received");
                shutdown(&command_ctx).await;
                break;
            }
        }
    }
}

/// Bundles the shared state a single command operates on.
struct CommandCtx<'a> {
    cache_manager: &'a std::sync::Arc<tokio::sync::Mutex<cache::CacheManager>>,
    voice_manager: &'a std::sync::Arc<parking_lot::Mutex<voice::VoiceManager>>,
    mixer_state: &'a std::sync::Arc<parking_lot::Mutex<audio::mixer::MixerState>>,
    active_voices: &'a std::sync::Arc<parking_lot::Mutex<std::collections::HashSet<String>>>,
    output_sample_rate: u32,
    config: &'a config::Config,
}

/// Apply a single parsed command to the shared audio state.
async fn handle_command(cmd: mqtt::commands::AudioCommand, ctx: &CommandCtx<'_>) {
    let cache_manager = ctx.cache_manager;
    let voice_manager = ctx.voice_manager;
    let mixer_state = ctx.mixer_state;
    let active_voices = ctx.active_voices;
    let output_sample_rate = ctx.output_sample_rate;
    let config = ctx.config;
    use audio::mixer::ActiveSample;

    tracing::info!("Processing command: {:?}", cmd);

    match cmd {
        mqtt::commands::AudioCommand::Play {
            file,
            id,
            volume,
            voice,
            channel_map,
            fade_in,
            start_position_ms,
            loop_mode,
            crossfade_ms,
        } => {
            // Load file (with streaming support for faster startup)
            let mut cache_mgr = cache_manager.lock().await;
            let buffer_result = cache_mgr
                .get_or_load_streaming(&file, output_sample_rate)
                .await;
            drop(cache_mgr);

            match buffer_result {
                Ok(buffer) => {
                    // Use provided voice or auto-generate one
                    let voice_id = voice.unwrap_or_else(|| {
                        format!(
                            "_auto_{}",
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_millis()
                        )
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
                            if crossfade_ms > 0 {
                                format!(", crossfade {}ms", crossfade_ms)
                            } else {
                                String::new()
                            }
                        );
                    }

                    // Get sample ID and voice volume from voice manager
                    let mut voice_mgr = voice_manager.lock();
                    let sample_id = voice_mgr.add_sample_to_voice(&voice_id);
                    let voice_volume = voice_mgr.get_voice_volume(&voice_id).unwrap_or(1.0);
                    drop(voice_mgr);

                    // Convert crossfade_ms to samples
                    let crossfade_samples =
                        (crossfade_ms as usize * output_sample_rate as usize) / 1000;

                    // Convert channel_map to mixer format (resolve any aliases)
                    let mut sample = if let Some(map) = channel_map {
                        // Custom channel mapping - resolve aliases
                        let mapping_result: Result<Vec<(usize, usize)>, String> = map
                            .iter()
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
                                return;
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
                        let max_frame = buffer
                            .total_frames_or_estimate()
                            .unwrap_or(usize::MAX)
                            .saturating_sub(1);
                        sample.position = target_frame.min(max_frame);
                        if is_streaming && !buffer.is_frame_loaded(sample.position) {
                            tracing::debug!(
                                "Starting at position {}ms (frame {}), waiting for data to load",
                                start_ms,
                                sample.position
                            );
                        } else {
                            tracing::debug!(
                                "Starting at position {}ms (frame {})",
                                start_ms,
                                sample.position
                            );
                        }
                    }

                    // Apply fade in if requested
                    if let Some(fade_ms) = fade_in {
                        sample.set_fade(audio::mixer::FadeState::fade_in(
                            fade_ms,
                            output_sample_rate,
                        ));
                        tracing::debug!("Applied {}ms fade in to voice '{}'", fade_ms, voice_id);
                    }

                    // Notify ducking engine if voice became active
                    let mut active_voices_guard = active_voices.lock();
                    let was_active = active_voices_guard.contains(&voice_id);
                    if !was_active {
                        active_voices_guard.insert(voice_id.clone());
                        drop(active_voices_guard);

                        // Notify ducking engine that voice became active
                        let mut state = mixer_state.lock();
                        if let Some(ref mut engine) = state.ducking_engine {
                            engine.notify_voice_active(&voice_id, true);
                        }
                        state.active_samples.push(sample);
                        tracing::info!("Now playing {} active samples", state.active_samples.len());
                    } else {
                        drop(active_voices_guard);
                        let mut state = mixer_state.lock();
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

            let mut state = mixer_state.lock();
            let count = state.active_samples.len();

            for sample in state.active_samples.iter_mut() {
                sample.set_fade(audio::mixer::FadeState::fade_out(
                    STOP_FADE_MS,
                    output_sample_rate,
                ));
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
            let mut voice_mgr = voice_manager.lock();
            let sample_ids = voice_mgr.clear_voice(&voice);
            drop(voice_mgr);

            if sample_ids.is_empty() {
                tracing::warn!("Voice '{}' not found or already empty", voice);
            } else {
                // Apply fade-out to all samples in the voice
                let mut state = mixer_state.lock();
                let mut updated_count = 0;
                for sample in state.active_samples.iter_mut() {
                    if sample_ids.contains(&sample.id) {
                        sample.set_fade(audio::mixer::FadeState::fade_out(
                            STOP_FADE_MS,
                            output_sample_rate,
                        ));
                        updated_count += 1;
                    }
                }
                drop(state);

                tracing::info!(
                    "Stopping voice '{}': {} samples ({}ms fade-out)",
                    voice,
                    updated_count,
                    STOP_FADE_MS
                );
            }
        }
        mqtt::commands::AudioCommand::VoiceFadeOut { voice, time_ms } => {
            // Get sample IDs in the voice
            let voice_mgr = voice_manager.lock();
            let sample_ids = voice_mgr.get_voice_sample_ids(&voice);
            drop(voice_mgr);

            if sample_ids.is_empty() {
                tracing::warn!("Voice '{}' not found or already empty", voice);
            } else {
                // Apply fade out to all samples in the voice
                let mut state = mixer_state.lock();
                let mut updated_count = 0;
                for sample in state.active_samples.iter_mut() {
                    if sample_ids.contains(&sample.id) {
                        sample.set_fade(audio::mixer::FadeState::fade_out(
                            time_ms,
                            output_sample_rate,
                        ));
                        updated_count += 1;
                    }
                }
                drop(state);

                tracing::info!(
                    "Applied {}ms fade out to voice '{}' ({} samples)",
                    time_ms,
                    voice,
                    updated_count
                );
            }
        }
        mqtt::commands::AudioCommand::VoiceVolume {
            voice,
            volume: new_volume,
        } => {
            // Set voice volume in voice manager
            let mut voice_mgr = voice_manager.lock();
            let success = voice_mgr.set_voice_volume(&voice, new_volume);
            let actual_volume = voice_mgr.get_voice_volume(&voice).unwrap_or(1.0);
            drop(voice_mgr);

            if success {
                // Update all active samples and live inputs in this voice with smooth ramping
                let mut state = mixer_state.lock();
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
                    voice,
                    actual_volume,
                    sample_count,
                    input_count
                );
            } else {
                tracing::warn!("Voice '{}' not found", voice);
            }
        }
        mqtt::commands::AudioCommand::Precache { file } => {
            // Non-blocking precache - starts loading and returns immediately
            let mut cache_mgr = cache_manager.lock().await;
            match cache_mgr
                .precache_streaming(&file, output_sample_rate)
                .await
            {
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
            let mut cache_mgr = cache_manager.lock().await;
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
            let mut cache_mgr = cache_manager.lock().await;
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
        mqtt::commands::AudioCommand::InputVolume {
            input,
            volume: new_volume,
        } => {
            let mut state = mixer_state.lock();
            let mut found = false;

            // Try to find by index first
            if let Ok(idx) = input.parse::<usize>() {
                if idx < state.live_inputs.len() {
                    state.live_inputs[idx].volume = new_volume.clamp(0.0, 1.0);
                    tracing::info!(
                        "Set input {} volume to {:.2}",
                        idx,
                        state.live_inputs[idx].volume
                    );
                    found = true;
                }
            }

            // Try to find by voice_id if not found by index
            if !found {
                for live_input in state.live_inputs.iter_mut() {
                    if live_input.voice_id == input {
                        live_input.volume = new_volume.clamp(0.0, 1.0);
                        tracing::info!("Set input '{}' volume to {:.2}", input, live_input.volume);
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
            let mut state = mixer_state.lock();
            let mut found = false;

            // Try to find by index first
            if let Ok(idx) = input.parse::<usize>() {
                if idx < state.live_inputs.len() {
                    // Mute by setting volume to 0, unmute restores to 1.0
                    // Note: This is a simple mute - a more sophisticated version
                    // would store the previous volume
                    state.live_inputs[idx].volume = if mute { 0.0 } else { 1.0 };
                    tracing::info!("Input {} {}", idx, if mute { "muted" } else { "unmuted" });
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
                            input,
                            if mute { "muted" } else { "unmuted" }
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
        mqtt::commands::AudioCommand::Seek {
            selector,
            position_ms,
        } => {
            if selector.is_empty() {
                tracing::warn!("Seek command with empty selector - no samples targeted");
            } else {
                let mut state = mixer_state.lock();
                let mut updated_count = 0;

                for sample in state.active_samples.iter_mut() {
                    if selector.matches(
                        sample.id,
                        sample.sample_id.as_deref(),
                        &sample.file_path,
                        &sample.voice_id,
                    ) {
                        // Convert milliseconds to frames
                        let target_frame =
                            ((position_ms * sample.buffer.sample_rate() as u64) / 1000) as usize;
                        // Clamp to buffer bounds
                        sample.position =
                            target_frame.min(sample.buffer.frames().saturating_sub(1));
                        updated_count += 1;
                    }
                }
                drop(state);

                if updated_count > 0 {
                    tracing::info!("Seeked {} samples to {}ms", updated_count, position_ms);
                } else {
                    tracing::warn!("Seek command matched no active samples");
                }
            }
        }
        mqtt::commands::AudioCommand::Speed {
            selector,
            speed,
            pitch_correction,
        } => {
            if selector.is_empty() {
                tracing::warn!("Speed command with empty selector - no samples targeted");
            } else {
                let mut state = mixer_state.lock();
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
                    let mode = if pitch_correction {
                        "pitch-corrected"
                    } else {
                        "normal"
                    };
                    tracing::info!(
                        "Set speed to {}x ({}) for {} samples",
                        speed,
                        mode,
                        updated_count
                    );
                } else {
                    tracing::warn!("Speed command matched no active samples");
                }
            }
        }
        mqtt::commands::AudioCommand::Stop {
            selector,
            fade_out_ms,
        } => {
            if selector.is_empty() {
                tracing::warn!("Stop command with empty selector - no samples targeted");
            } else {
                // Default to 10ms fade for smooth stop if not specified
                let fade_ms = fade_out_ms.unwrap_or(10);

                let mut state = mixer_state.lock();
                let mut updated_count = 0;

                for sample in state.active_samples.iter_mut() {
                    if selector.matches(
                        sample.id,
                        sample.sample_id.as_deref(),
                        &sample.file_path,
                        &sample.voice_id,
                    ) {
                        sample.set_fade(audio::mixer::FadeState::fade_out(
                            fade_ms,
                            output_sample_rate,
                        ));
                        updated_count += 1;
                    }
                }
                drop(state);

                if updated_count > 0 {
                    tracing::info!(
                        "Stopping {} samples ({}ms fade-out)",
                        updated_count,
                        fade_ms
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
                let mut state = mixer_state.lock();
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
                    tracing::info!("Set volume to {:.2} for {} samples", volume, updated_count);
                } else {
                    tracing::warn!("Volume command matched no active samples");
                }
            }
        }
    }
}

/// Resolve when the process receives SIGINT (Ctrl-C) or, on unix, SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::error!("Failed to install SIGTERM handler: {}", e);
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

/// Graceful shutdown: fade out active samples, let the callback drain the fade,
/// then flush cache metadata before the process exits.
async fn shutdown(ctx: &CommandCtx<'_>) {
    const SHUTDOWN_FADE_MS: u32 = 50;

    {
        let mut state = ctx.mixer_state.lock();
        let count = state.active_samples.len();
        for sample in state.active_samples.iter_mut() {
            sample.set_fade(audio::mixer::FadeState::fade_out(
                SHUTDOWN_FADE_MS,
                ctx.output_sample_rate,
            ));
        }
        tracing::info!("Shutdown: fading out {} active samples", count);
    }

    // Let the audio callback drain the fade so output ends on silence, not a click.
    tokio::time::sleep(std::time::Duration::from_millis(
        SHUTDOWN_FADE_MS as u64 + 30,
    ))
    .await;

    // Flush cache metadata to disk.
    let cache_mgr = ctx.cache_manager.lock().await;
    if let Err(e) = cache_mgr.flush_metadata() {
        tracing::error!("Failed to flush cache metadata on shutdown: {}", e);
    }
    drop(cache_mgr);

    tracing::info!("Shutdown complete");
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio::ducking::{DuckingEngine, DuckingRule};
    use audio::mixer::{FadeState, MixerState};
    use mqtt::commands::{AudioCommand, SampleSelector};
    use parking_lot::Mutex;
    use std::collections::HashSet;
    use std::sync::Arc;
    use voice::VoiceManager;

    const TEST_WAV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/audio/test_440hz_2s.wav");
    const SR: u32 = 48000;

    /// Owns the shared state needed to drive `handle_command` in tests.
    struct Fixture {
        cache_manager: Arc<tokio::sync::Mutex<cache::CacheManager>>,
        voice_manager: Arc<Mutex<VoiceManager>>,
        mixer_state: Arc<Mutex<MixerState>>,
        active_voices: Arc<Mutex<HashSet<String>>>,
        config: config::Config,
        _cache_dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new(ducking_engine: Option<DuckingEngine>) -> Self {
            let cache_dir = tempfile::tempdir().unwrap();
            let cache = cache::CacheManager::new(cache_dir.path().to_path_buf()).unwrap();
            let mixer_state = MixerState {
                active_samples: Vec::new(),
                live_inputs: Vec::new(),
                output_channels: 2,
                ducking_engine,
                bass_management: None,
            };
            Self {
                cache_manager: Arc::new(tokio::sync::Mutex::new(cache)),
                voice_manager: Arc::new(Mutex::new(VoiceManager::new())),
                mixer_state: Arc::new(Mutex::new(mixer_state)),
                active_voices: Arc::new(Mutex::new(HashSet::new())),
                config: config::Config::default(),
                _cache_dir: cache_dir,
            }
        }

        fn ctx(&self) -> CommandCtx<'_> {
            CommandCtx {
                cache_manager: &self.cache_manager,
                voice_manager: &self.voice_manager,
                mixer_state: &self.mixer_state,
                active_voices: &self.active_voices,
                output_sample_rate: SR,
                config: &self.config,
            }
        }
    }

    fn play(voice: Option<&str>, volume: f32) -> AudioCommand {
        AudioCommand::Play {
            file: TEST_WAV.to_string(),
            id: None,
            volume,
            voice: voice.map(str::to_string),
            channel_map: None,
            fade_in: None,
            start_position_ms: None,
            loop_mode: false,
            crossfade_ms: 0,
        }
    }

    #[tokio::test]
    async fn play_adds_active_sample_and_marks_voice_active() {
        let fixture = Fixture::new(None);
        handle_command(play(Some("music"), 0.5), &fixture.ctx()).await;

        let state = fixture.mixer_state.lock();
        assert_eq!(state.active_samples.len(), 1);
        let sample = &state.active_samples[0];
        assert_eq!(sample.voice_id, "music");
        assert_eq!(sample.volume, 0.5);
        assert_eq!(sample.position, 0);
        assert!(matches!(sample.fade_state, FadeState::None));
        assert_eq!(sample.crossfade_samples, 0);

        assert!(fixture.active_voices.lock().contains("music"));
    }

    #[tokio::test]
    async fn play_with_fade_in_sets_fade_state() {
        let fixture = Fixture::new(None);
        let mut cmd = play(Some("music"), 1.0);
        if let AudioCommand::Play { fade_in, .. } = &mut cmd {
            *fade_in = Some(100);
        }
        handle_command(cmd, &fixture.ctx()).await;

        let state = fixture.mixer_state.lock();
        // 100 ms at 48 kHz = 4800 frames
        assert!(matches!(
            state.active_samples[0].fade_state,
            FadeState::In {
                elapsed: 0,
                duration: 4800
            }
        ));
    }

    #[tokio::test]
    async fn stop_all_applies_fade_out_to_every_sample() {
        let fixture = Fixture::new(None);
        handle_command(play(Some("a"), 1.0), &fixture.ctx()).await;
        handle_command(play(Some("b"), 1.0), &fixture.ctx()).await;
        assert_eq!(fixture.mixer_state.lock().active_samples.len(), 2);

        handle_command(AudioCommand::StopAll, &fixture.ctx()).await;

        let state = fixture.mixer_state.lock();
        assert_eq!(state.active_samples.len(), 2);
        for sample in &state.active_samples {
            assert!(matches!(sample.fade_state, FadeState::Out { .. }));
        }
    }

    #[tokio::test]
    async fn stop_by_voice_selector_fades_only_matching_samples() {
        let fixture = Fixture::new(None);
        handle_command(play(Some("music"), 1.0), &fixture.ctx()).await;
        handle_command(play(Some("sfx"), 1.0), &fixture.ctx()).await;

        let stop = AudioCommand::Stop {
            selector: SampleSelector {
                internal_id: None,
                id: None,
                file: None,
                voice: Some("music".to_string()),
            },
            fade_out_ms: Some(50),
        };
        handle_command(stop, &fixture.ctx()).await;

        let expected_duration = 50 * SR as usize / 1000; // 2400 frames
        let state = fixture.mixer_state.lock();
        for sample in &state.active_samples {
            if sample.voice_id == "music" {
                assert!(matches!(
                    sample.fade_state,
                    FadeState::Out { duration, .. } if duration == expected_duration
                ));
            } else {
                assert!(matches!(sample.fade_state, FadeState::None));
            }
        }
    }

    #[tokio::test]
    async fn voice_volume_sets_ramp_target_without_jumping() {
        let fixture = Fixture::new(None);
        handle_command(play(Some("music"), 1.0), &fixture.ctx()).await;

        handle_command(
            AudioCommand::VoiceVolume {
                voice: "music".to_string(),
                volume: 0.3,
            },
            &fixture.ctx(),
        )
        .await;

        let state = fixture.mixer_state.lock();
        let sample = &state.active_samples[0];
        assert!((sample.target_voice_volume - 0.3).abs() < 1e-6);
        // Current voice volume is ramped over time, so it must not jump immediately.
        assert!((sample.voice_volume - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn playing_a_ducking_primary_ducks_the_background_voice() {
        let rule = DuckingRule {
            primary_voice: "narration".to_string(),
            ducked_voices: vec!["music".to_string()],
            target_volume: 0.1,
            fade_duration_ms: 0, // instant, for a deterministic assertion
        };
        let fixture = Fixture::new(Some(DuckingEngine::new(vec![rule], SR)));

        // Background music starts first. It is not a ducking primary, so it stays full.
        handle_command(play(Some("music"), 1.0), &fixture.ctx()).await;
        {
            let mut state = fixture.mixer_state.lock();
            let engine = state.ducking_engine.as_mut().unwrap();
            assert!((engine.get_multiplier("music", 1) - 1.0).abs() < 1e-6);
        }

        // The primary voice starts: handle_command must notify the ducking engine,
        // which ducks "music".
        handle_command(play(Some("narration"), 1.0), &fixture.ctx()).await;
        {
            let mut state = fixture.mixer_state.lock();
            let engine = state.ducking_engine.as_mut().unwrap();
            let multiplier = engine.get_multiplier("music", 1);
            assert!(
                multiplier < 0.2,
                "expected music ducked toward 0.1, got {multiplier}"
            );
        }
    }

    #[tokio::test]
    async fn shutdown_fades_samples_and_flushes_metadata() {
        let fixture = Fixture::new(None);
        handle_command(play(Some("v"), 1.0), &fixture.ctx()).await;
        assert_eq!(fixture.mixer_state.lock().active_samples.len(), 1);

        shutdown(&fixture.ctx()).await;

        // Every active sample now has a fade-out applied (drained to silence by the
        // real callback at runtime; here we just assert the fade was set).
        {
            let state = fixture.mixer_state.lock();
            assert!(!state.active_samples.is_empty());
            for sample in &state.active_samples {
                assert!(matches!(sample.fade_state, FadeState::Out { .. }));
            }
        }
        // Cache metadata was flushed to disk on shutdown.
        let metadata = fixture._cache_dir.path().join("metadata.json");
        assert!(
            metadata.exists(),
            "metadata.json should be flushed on shutdown"
        );
    }
}
