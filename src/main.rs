// ABOUTME: Entry point for mqttaudio MQTT-controlled audio daemon.
// ABOUTME: Handles CLI parsing, initialization, and main event loop.

mod audio;
mod cache;
mod config;
mod http;
mod mqtt;
mod rt_engine;
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
    use audio::ducking::{DuckingApplier, DuckingEngine};
    use audio::mixer::MixerState;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, RwLock};
    use voice::VoiceManager;

    // The ducking engine that resolves rules lives on the control thread (it is
    // driven by command sends and graveyard completions, never by the callback);
    // the audio thread only applies the resolved targets via a DuckingApplier.
    let mut ducking_engine = if !config.ducking_rules.is_empty() {
        Some(DuckingEngine::new(
            config.ducking_rules.clone(),
            output_sample_rate,
        ))
    } else {
        None
    };
    let ducking_applier = if ducking_engine.is_some() {
        Some(DuckingApplier::new())
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
            lfe_gain: resolved.lfe_gain,
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

    // Shared clip/over counter: the audio thread increments it (lock-free) when the
    // limiter acts; HTTP `/status` reads it. Created once and cloned into both sides.
    let clip_count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let mixer = MixerState {
        active_samples: Vec::with_capacity(audio::mixer::MAX_VOICES),
        live_inputs: Vec::with_capacity(audio::mixer::MAX_LIVE_INPUTS),
        output_channels,
        ducking_applier,
        bass_management,
        channel_gains: config.resolve_channel_gains(output_channels),
        output_ceiling: audio::mixer::db_to_linear(config.audio.output_ceiling_db),
        master_gain: config.audio.master_gain,
        clip_count: clip_count.clone(),
    };

    // Control->audio command ring, audio->reaper graveyard ring, and audio->reaper
    // command-return ring. The control thread holds the command producer and the two
    // reaper consumers; the audio callback owns the bundle behind one uncontended
    // mutex (D15/D22a). Spent heap-owning mutation commands travel back over the
    // return ring so the callback never frees their heap.
    let (cmd_tx, cmd_rx) = rt_engine::command_channel(1024);
    let (grave_tx, mut grave_rx) = rt_engine::graveyard_channel(1024);
    let (cmd_return_tx, mut cmd_return_rx) = rt_engine::command_return_channel(1024);

    // Initialize audio inputs from config.
    // Keep active input streams alive - they will be kept alive until the app exits.
    // Live inputs are added to the mix by command (AddLiveInput); their static
    // status is recorded for the control-side snapshot.
    let mut _active_inputs: Vec<audio::input::ActiveInput> = Vec::new();
    let mut input_statuses: Vec<http::InputStatus> = Vec::new();
    let mut cmd_tx = cmd_tx;
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

                    input_statuses.push(http::InputStatus {
                        index: input_statuses.len(),
                        voice_id: input_config.voice_id.clone(),
                        volume: input_config.volume,
                        channels: active_input.channels,
                    });

                    // Add to the mix via the command ring (drained once the
                    // output stream starts).
                    if cmd_tx
                        .push(rt_engine::AudioCommand::AddLiveInput(live_input))
                        .is_err()
                    {
                        tracing::error!("Command ring full while adding live input {}", idx);
                    }

                    // Mark the configured input voice active so it can trigger ducking
                    // as a primary (D4), through the same off-RT notify path as sample
                    // voices. The stream is open for the process lifetime, so the voice
                    // is active for it; signal-gated activation is Sprint 8 (D36).
                    notify_voice_activity(
                        &input_config.voice_id,
                        true,
                        ducking_engine.as_mut(),
                        &mut cmd_tx,
                    );
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

    // The bundle the cpal callback owns. The control thread never locks this.
    let callback_state = Arc::new(Mutex::new(rt_engine::AudioCallbackState {
        mixer,
        commands: cmd_rx,
        command_returns: cmd_return_tx,
        graveyard: grave_tx,
        output_sample_rate,
    }));
    let xruns = Arc::new(AtomicU64::new(0));

    let voice_manager = Arc::new(Mutex::new(VoiceManager::new()));

    // Control-side voice activity bookkeeping and the authoritative status
    // snapshot the HTTP handlers read. `active_counts` drives ducking restore;
    // `playing` mirrors the live sample list for status (position excluded).
    let mut active_counts: HashMap<String, usize> = HashMap::new();
    let mut playing: HashMap<u64, http::SampleStatus> = HashMap::new();
    let status_snapshot = Arc::new(RwLock::new(http::StatusSnapshot {
        active_samples: 0,
        output_channels,
        samples: Vec::new(),
        inputs: input_statuses.clone(),
    }));

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
        callback_state.clone(),
        xruns.clone(),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );

    // Create the text-command channel (HTTP/MQTT payloads -> control loop).
    let (text_tx, mut text_rx) = mpsc::channel::<String>(100);

    // Start HTTP server if enabled
    if config.http.enabled {
        let http_cmd_tx = text_tx.clone();
        match http::start_server(
            &config.http,
            http_cmd_tx,
            status_snapshot.clone(),
            voice_manager.clone(),
            cache_manager.clone(),
            clip_count.clone(),
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
        let mqtt_cmd_tx = text_tx.clone();
        let mqtt_topic = config.mqtt.topic.clone().unwrap_or_default();
        tokio::spawn(async move {
            mqtt::client::process_mqtt_events(client, mqtt_topic, eventloop, mqtt_cmd_tx).await;
        });
        tracing::info!(
            "Ready to receive MQTT commands on topic: {}",
            config.mqtt.topic.as_ref().unwrap()
        );
    }

    // Keep the text-command sender alive if only HTTP is running (no MQTT)
    let _text_tx_keepalive = text_tx;

    // Main command processing loop
    if mqtt_enabled || http_enabled {
        tracing::info!("Command processing loop started");
    }

    // Periodic reaper: drains finished samples returned by the audio thread,
    // decrements voice activity, and lets the ducking engine restore voices.
    let mut reaper_tick = tokio::time::interval(std::time::Duration::from_millis(20));

    loop {
        tokio::select! {
            maybe_payload = text_rx.recv() => {
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
                    Ok(cmd) => {
                        let mut ctx = CommandCtx {
                            cache_manager: &cache_manager,
                            voice_manager: &voice_manager,
                            cmd_tx: &mut cmd_tx,
                            ducking_engine: &mut ducking_engine,
                            active_counts: &mut active_counts,
                            playing: &mut playing,
                            snapshot: &status_snapshot,
                            inputs: &input_statuses,
                            output_channels,
                            output_sample_rate,
                            config: &config,
                        };
                        handle_command(cmd, &mut ctx).await;
                    }
                    Err(e) => {
                        tracing::error!("Failed to parse command: {}", e);
                    }
                }
            }
            _ = reaper_tick.tick() => {
                reap_finished_samples(
                    &mut grave_rx,
                    &mut cmd_return_rx,
                    &mut cmd_tx,
                    ducking_engine.as_mut(),
                    &mut active_counts,
                    &mut playing,
                    &status_snapshot,
                    &input_statuses,
                    output_channels,
                );
            }
            _ = shutdown_signal() => {
                tracing::info!("Shutdown signal received");
                shutdown(&mut cmd_tx, &cache_manager).await;
                break;
            }
        }
    }
}

/// Rebuild the control-side status snapshot from the live sample map and the
/// configured inputs. Cheap; called whenever the playing set changes.
fn refresh_snapshot(
    snapshot: &std::sync::Arc<std::sync::RwLock<http::StatusSnapshot>>,
    playing: &std::collections::HashMap<u64, http::SampleStatus>,
    inputs: &[http::InputStatus],
    output_channels: usize,
) {
    let mut samples: Vec<http::SampleStatus> = playing.values().cloned().collect();
    samples.sort_by_key(|s| s.internal_id);
    let mut guard = snapshot.write().unwrap();
    guard.active_samples = samples.len();
    guard.output_channels = output_channels;
    guard.samples = samples;
    guard.inputs = inputs.to_vec();
}

/// Notify the control-side ducking engine that a voice's activity changed and
/// forward any resulting target changes to the audio thread. Used by both the
/// sample-playback path and the configured-input path (D4), so live-input voices
/// can trigger ducking through the same off-RT `compute_changes` path. A no-op when
/// ducking is not configured.
fn notify_voice_activity(
    voice_id: &str,
    is_active: bool,
    ducking_engine: Option<&mut audio::ducking::DuckingEngine>,
    cmd_tx: &mut rt_engine::CommandProducer,
) {
    if let Some(engine) = ducking_engine {
        for change in engine.compute_changes(voice_id, is_active) {
            if cmd_tx
                .push(rt_engine::AudioCommand::SetDuckTarget(change))
                .is_err()
            {
                tracing::error!("Audio command ring full; duck target dropped");
            }
        }
    }
}

/// Drain the graveyard and the command-return ring, dropping both finished samples
/// and spent mutation commands off the audio thread, and reconcile control-side
/// voice activity. When a voice's last sample finishes, the ducking engine computes
/// restore targets that are sent back to the audio thread. Refreshes the status
/// snapshot if anything was reaped.
#[allow(clippy::too_many_arguments)]
fn reap_finished_samples(
    grave_rx: &mut rt_engine::GraveyardConsumer,
    cmd_return_rx: &mut rt_engine::CommandReturnConsumer,
    cmd_tx: &mut rt_engine::CommandProducer,
    mut ducking_engine: Option<&mut audio::ducking::DuckingEngine>,
    active_counts: &mut std::collections::HashMap<String, usize>,
    playing: &mut std::collections::HashMap<u64, http::SampleStatus>,
    snapshot: &std::sync::Arc<std::sync::RwLock<http::StatusSnapshot>>,
    inputs: &[http::InputStatus],
    output_channels: usize,
) {
    // Drop spent mutation commands the callback returned, off the audio thread.
    // Their String/Vec/SampleSelector heap is freed here, never in the callback.
    while let Some(spent) = cmd_return_rx.pop() {
        drop(spent);
    }

    let mut reaped = 0;
    while let Some(finished) = grave_rx.pop() {
        let voice = finished.voice_id.clone();
        let id = finished.id;
        // Dropping `finished` here is the off-RT free of the sample's buffers.
        drop(finished);

        playing.remove(&id);
        reaped += 1;

        if let Some(count) = active_counts.get_mut(&voice) {
            *count -= 1;
            if *count == 0 {
                active_counts.remove(&voice);
                // Voice's last sample finished: restore through the shared notify path.
                notify_voice_activity(&voice, false, ducking_engine.as_deref_mut(), cmd_tx);
            }
        }
    }

    if reaped > 0 {
        refresh_snapshot(snapshot, playing, inputs, output_channels);
    }
}

/// Bundles the control-side state a single command operates on. All audio-state
/// mutations are sent to the audio thread through `cmd_tx`; the control thread
/// owns the ducking engine, voice-activity counts, live sample map, and the
/// status snapshot, and never touches the audio thread's `MixerState`.
struct CommandCtx<'a> {
    cache_manager: &'a std::sync::Arc<tokio::sync::Mutex<cache::CacheManager>>,
    voice_manager: &'a std::sync::Arc<parking_lot::Mutex<voice::VoiceManager>>,
    cmd_tx: &'a mut rt_engine::CommandProducer,
    ducking_engine: &'a mut Option<audio::ducking::DuckingEngine>,
    active_counts: &'a mut std::collections::HashMap<String, usize>,
    playing: &'a mut std::collections::HashMap<u64, http::SampleStatus>,
    snapshot: &'a std::sync::Arc<std::sync::RwLock<http::StatusSnapshot>>,
    inputs: &'a [http::InputStatus],
    output_channels: usize,
    output_sample_rate: u32,
    config: &'a config::Config,
}

impl CommandCtx<'_> {
    /// Push a resolved command to the audio thread, logging if the ring is full.
    fn send(&mut self, command: rt_engine::AudioCommand) {
        if self.cmd_tx.push(command).is_err() {
            tracing::error!("Audio command ring full; command dropped");
        }
    }

    /// Rebuild the status snapshot from the current live sample map and inputs.
    fn refresh(&self) {
        refresh_snapshot(
            self.snapshot,
            self.playing,
            self.inputs,
            self.output_channels,
        );
    }
}

/// Apply a single parsed command to the shared audio state.
async fn handle_command(cmd: mqtt::commands::AudioCommand, ctx: &mut CommandCtx<'_>) {
    let cache_manager = ctx.cache_manager;
    let voice_manager = ctx.voice_manager;
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
                        // Per-route downmix gains parallel to the resolved map (D29);
                        // a route with no gain defaults to unity, leaving plain 1:1
                        // mappings unchanged. Clamp to a sane finite, non-negative
                        // range (matching audio.master_gain) so a bad value can never
                        // push NaN/Inf or a wild level onto the bus.
                        let route_gains: Vec<f32> = map
                            .iter()
                            .map(|m| match m.gain {
                                Some(g) if g.is_finite() => g.clamp(0.0, 8.0),
                                _ => 1.0,
                            })
                            .collect();
                        let mut sample = ActiveSample::new_with_mapping(
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
                        );
                        if route_gains.iter().any(|&g| g != 1.0) {
                            sample.set_channel_route_gains(route_gains);
                        }
                        sample
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

                    // Record the control-side status before the sample is moved
                    // into the command (live position stays audio-thread-owned).
                    let status = http::SampleStatus {
                        internal_id: sample.id,
                        sample_id: sample.sample_id.clone(),
                        voice_id: voice_id.clone(),
                        file_path: sample.file_path.clone(),
                        total_frames: sample.buffer.frames(),
                        sample_rate: sample.buffer.sample_rate(),
                        volume: sample.volume,
                        voice_volume: sample.voice_volume,
                        speed: sample.speed,
                        loop_mode: sample.loop_mode,
                    };

                    // Voice became active: compute and forward ducking targets
                    // through the shared off-RT notify path (also used by inputs, D4).
                    let became_active = !ctx.active_counts.contains_key(&voice_id);
                    if became_active {
                        notify_voice_activity(
                            &voice_id,
                            true,
                            ctx.ducking_engine.as_mut(),
                            ctx.cmd_tx,
                        );
                    }
                    *ctx.active_counts.entry(voice_id.clone()).or_insert(0) += 1;
                    ctx.playing.insert(status.internal_id, status);
                    ctx.send(rt_engine::AudioCommand::AddSample(sample));
                    ctx.refresh();
                    tracing::info!("Now playing {} active samples", ctx.playing.len());
                }
                Err(e) => {
                    tracing::error!("Failed to load {}: {}", file, e);
                }
            }
        }
        mqtt::commands::AudioCommand::StopAll => {
            // Apply quick 10ms fade-out to prevent clicks/pops. The audio thread
            // fades and finishes the samples; the reaper then reconciles voice
            // activity and the snapshot from the graveyard returns.
            const STOP_FADE_MS: u32 = 10;
            let count = ctx.playing.len();
            ctx.send(rt_engine::AudioCommand::FadeOutAll {
                fade_ms: STOP_FADE_MS,
            });
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
                tracing::info!(
                    "Stopping voice '{}': {} samples ({}ms fade-out)",
                    voice,
                    sample_ids.len(),
                    STOP_FADE_MS
                );
                ctx.send(rt_engine::AudioCommand::FadeOutSamples {
                    ids: sample_ids,
                    fade_ms: STOP_FADE_MS,
                });
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
                tracing::info!(
                    "Applied {}ms fade out to voice '{}' ({} samples)",
                    time_ms,
                    voice,
                    sample_ids.len()
                );
                ctx.send(rt_engine::AudioCommand::FadeOutSamples {
                    ids: sample_ids,
                    fade_ms: time_ms,
                });
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
                // The audio thread ramps samples and live inputs in this voice
                // toward the new target (SetVoiceVolume covers both).
                ctx.send(rt_engine::AudioCommand::SetVoiceVolume {
                    voice: voice.clone(),
                    volume: actual_volume,
                });
                // Keep the status snapshot's per-sample voice volume in step.
                for status in ctx.playing.values_mut() {
                    if status.voice_id == voice {
                        status.voice_volume = actual_volume;
                    }
                }
                ctx.refresh();
                tracing::info!("Set voice '{}' volume to {:.2}", voice, actual_volume);
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
            // The audio thread resolves the input by index or voice id and clamps.
            tracing::info!("Set input '{}' volume to {:.2}", input, new_volume);
            ctx.send(rt_engine::AudioCommand::SetInputVolume {
                input,
                volume: new_volume,
            });
        }
        mqtt::commands::AudioCommand::InputMute { input, mute } => {
            // Mute by setting volume to 0, unmute restores to 1.0.
            tracing::info!(
                "Input '{}' {}",
                input,
                if mute { "muted" } else { "unmuted" }
            );
            ctx.send(rt_engine::AudioCommand::SetInputVolume {
                input,
                volume: if mute { 0.0 } else { 1.0 },
            });
        }
        mqtt::commands::AudioCommand::Seek {
            selector,
            position_ms,
        } => {
            if selector.is_empty() {
                tracing::warn!("Seek command with empty selector - no samples targeted");
            } else {
                // The audio thread matches the selector against the live samples
                // and converts ms to frames per sample's rate.
                tracing::info!("Seeking matching samples to {}ms", position_ms);
                ctx.send(rt_engine::AudioCommand::SeekMatching {
                    selector,
                    position_ms,
                });
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
                let mode = if pitch_correction {
                    "pitch-corrected"
                } else {
                    "normal"
                };
                tracing::info!("Set speed to {}x ({}) for matching samples", speed, mode);
                // Keep the snapshot's per-sample speed in step for samples the
                // control thread knows match the selector.
                for status in ctx.playing.values_mut() {
                    if sample_status_matches(&selector, status) {
                        status.speed = speed;
                    }
                }
                ctx.send(rt_engine::AudioCommand::SetSpeedMatching {
                    selector,
                    speed,
                    pitch_correction,
                });
                ctx.refresh();
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
                tracing::info!("Stopping matching samples ({}ms fade-out)", fade_ms);
                ctx.send(rt_engine::AudioCommand::FadeOutMatching { selector, fade_ms });
            }
        }
        mqtt::commands::AudioCommand::Volume { selector, volume } => {
            if selector.is_empty() {
                tracing::warn!("Volume command with empty selector - no samples targeted");
            } else {
                tracing::info!("Set volume to {:.2} for matching samples", volume);
                // Keep the snapshot's per-sample volume in step.
                let clamped = volume.clamp(0.0, 1.0);
                for status in ctx.playing.values_mut() {
                    if sample_status_matches(&selector, status) {
                        status.volume = clamped;
                    }
                }
                ctx.send(rt_engine::AudioCommand::SetVolumeMatching { selector, volume });
                ctx.refresh();
            }
        }
    }
}

/// Whether a control-side sample status matches a selector. Mirrors the
/// audio-side matching so the status snapshot can track selector-targeted
/// changes the control thread can see (the audio thread remains authoritative).
fn sample_status_matches(
    selector: &mqtt::commands::SampleSelector,
    status: &http::SampleStatus,
) -> bool {
    selector.matches(
        status.internal_id,
        status.sample_id.as_deref(),
        &status.file_path,
        &status.voice_id,
    )
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

/// Graceful shutdown: fade out active samples via the command ring, let the
/// callback drain the fade, then flush cache metadata before the process exits.
async fn shutdown(
    cmd_tx: &mut rt_engine::CommandProducer,
    cache_manager: &std::sync::Arc<tokio::sync::Mutex<cache::CacheManager>>,
) {
    const SHUTDOWN_FADE_MS: u32 = 50;

    tracing::info!("Shutdown: fading out active samples");
    let _ = cmd_tx.push(rt_engine::AudioCommand::FadeOutAll {
        fade_ms: SHUTDOWN_FADE_MS,
    });

    // Let the audio callback drain the fade so output ends on silence, not a click.
    tokio::time::sleep(std::time::Duration::from_millis(
        SHUTDOWN_FADE_MS as u64 + 30,
    ))
    .await;

    // Flush cache metadata to disk.
    let cache_mgr = cache_manager.lock().await;
    if let Err(e) = cache_mgr.flush_metadata() {
        tracing::error!("Failed to flush cache metadata on shutdown: {}", e);
    }
    drop(cache_mgr);

    tracing::info!("Shutdown complete");
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio::ducking::{DuckingApplier, DuckingEngine, DuckingRule};
    use audio::mixer::{FadeState, MixerState};
    use mqtt::commands::{AudioCommand, SampleSelector};
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};
    use voice::VoiceManager;

    const TEST_WAV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/audio/test_440hz_2s.wav");
    const SR: u32 = 48000;

    /// Owns the control-side state needed to drive `handle_command` in tests and
    /// the audio-side `MixerState` the resulting commands are applied to. The
    /// control thread never touches `MixerState` directly: `handle_command` pushes
    /// `rt_engine::AudioCommand`s onto the ring, and `drain` applies them to the
    /// mixer exactly as the audio callback would — so a test asserts the same end
    /// state the running engine would reach.
    struct Fixture {
        cache_manager: Arc<tokio::sync::Mutex<cache::CacheManager>>,
        voice_manager: Arc<Mutex<VoiceManager>>,
        cmd_tx: rt_engine::CommandProducer,
        cmd_rx: rt_engine::CommandConsumer,
        cmd_return_tx: rt_engine::CommandReturnProducer,
        cmd_return_rx: rt_engine::CommandReturnConsumer,
        grave_tx: rt_engine::GraveyardProducer,
        grave_rx: rt_engine::GraveyardConsumer,
        mixer: MixerState,
        ducking_engine: Option<DuckingEngine>,
        active_counts: HashMap<String, usize>,
        playing: HashMap<u64, http::SampleStatus>,
        snapshot: Arc<RwLock<http::StatusSnapshot>>,
        inputs: Vec<http::InputStatus>,
        config: config::Config,
        _cache_dir: tempfile::TempDir,
    }

    impl Fixture {
        /// Build a fixture. When `ducking_rules` is non-empty, a control-side
        /// `DuckingEngine` resolves rules and the mixer carries a `DuckingApplier`
        /// that applies the resolved `SetDuckTarget` commands — mirroring runtime.
        fn new(ducking_rules: Vec<DuckingRule>) -> Self {
            let cache_dir = tempfile::tempdir().unwrap();
            let cache = cache::CacheManager::new(cache_dir.path().to_path_buf()).unwrap();
            let (ducking_engine, ducking_applier) = if ducking_rules.is_empty() {
                (None, None)
            } else {
                (
                    Some(DuckingEngine::new(ducking_rules, SR)),
                    Some(DuckingApplier::new()),
                )
            };
            let mixer = MixerState {
                ducking_applier,
                ..MixerState::new(2)
            };
            let (cmd_tx, cmd_rx) = rt_engine::command_channel(1024);
            let (cmd_return_tx, cmd_return_rx) = rt_engine::command_return_channel(1024);
            let (grave_tx, grave_rx) = rt_engine::graveyard_channel(1024);
            Self {
                cache_manager: Arc::new(tokio::sync::Mutex::new(cache)),
                voice_manager: Arc::new(Mutex::new(VoiceManager::new())),
                cmd_tx,
                cmd_rx,
                cmd_return_tx,
                cmd_return_rx,
                grave_tx,
                grave_rx,
                mixer,
                ducking_engine,
                active_counts: HashMap::new(),
                playing: HashMap::new(),
                snapshot: Arc::new(RwLock::new(http::StatusSnapshot {
                    active_samples: 0,
                    output_channels: 2,
                    samples: Vec::new(),
                    inputs: Vec::new(),
                })),
                inputs: Vec::new(),
                config: config::Config::default(),
                _cache_dir: cache_dir,
            }
        }

        /// Run a parsed command through `handle_command`, pushing the resulting
        /// audio commands onto the ring (but not yet applying them to the mixer).
        async fn run(&mut self, cmd: AudioCommand) {
            let mut ctx = CommandCtx {
                cache_manager: &self.cache_manager,
                voice_manager: &self.voice_manager,
                cmd_tx: &mut self.cmd_tx,
                ducking_engine: &mut self.ducking_engine,
                active_counts: &mut self.active_counts,
                playing: &mut self.playing,
                snapshot: &self.snapshot,
                inputs: &self.inputs,
                output_channels: 2,
                output_sample_rate: SR,
                config: &self.config,
            };
            handle_command(cmd, &mut ctx).await;
        }

        /// Apply every queued audio command to the mixer, as the audio callback
        /// would, so the test can assert the resulting `MixerState`. Spent mutation
        /// commands are routed to the return ring, mirroring the callback.
        fn drain(&mut self) {
            rt_engine::drain_commands(
                &mut self.cmd_rx,
                &mut self.mixer,
                &mut self.cmd_return_tx,
                SR,
                1024,
            );
        }

        /// Drive the control-side reaper exactly as the 20ms tick does, draining
        /// the graveyard and command-return rings and reconciling voice activity.
        fn reap(&mut self) {
            reap_finished_samples(
                &mut self.grave_rx,
                &mut self.cmd_return_rx,
                &mut self.cmd_tx,
                self.ducking_engine.as_mut(),
                &mut self.active_counts,
                &mut self.playing,
                &self.snapshot,
                &self.inputs,
                2,
            );
        }

        /// Drive the off-RT voice-activity notify path directly, as the input setup
        /// (D4) and a future input-activity gate (Sprint 8) will, forwarding any
        /// resulting duck targets onto the command ring.
        fn notify_voice(&mut self, voice: &str, is_active: bool) {
            notify_voice_activity(
                voice,
                is_active,
                self.ducking_engine.as_mut(),
                &mut self.cmd_tx,
            );
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
        let mut fixture = Fixture::new(vec![]);
        fixture.run(play(Some("music"), 0.5)).await;
        fixture.drain();

        assert_eq!(fixture.mixer.active_samples.len(), 1);
        let sample = &fixture.mixer.active_samples[0];
        assert_eq!(sample.voice_id, "music");
        assert_eq!(sample.volume, 0.5);
        assert_eq!(sample.position, 0);
        assert!(matches!(sample.fade_state, FadeState::None));
        assert_eq!(sample.crossfade_samples, 0);

        // The control thread tracks the voice as active for ducking/status.
        assert!(fixture.active_counts.contains_key("music"));
    }

    #[tokio::test]
    async fn play_with_fade_in_sets_fade_state() {
        let mut fixture = Fixture::new(vec![]);
        let mut cmd = play(Some("music"), 1.0);
        if let AudioCommand::Play { fade_in, .. } = &mut cmd {
            *fade_in = Some(100);
        }
        fixture.run(cmd).await;
        fixture.drain();

        // 100 ms at 48 kHz = 4800 frames
        assert!(matches!(
            fixture.mixer.active_samples[0].fade_state,
            FadeState::In {
                elapsed: 0,
                duration: 4800
            }
        ));
    }

    #[tokio::test]
    async fn stop_all_applies_fade_out_to_every_sample() {
        let mut fixture = Fixture::new(vec![]);
        fixture.run(play(Some("a"), 1.0)).await;
        fixture.run(play(Some("b"), 1.0)).await;
        // StopAll fades whatever is active; draining in order adds both samples
        // before the fade-out is applied, exactly as the callback would.
        fixture.run(AudioCommand::StopAll).await;
        fixture.drain();

        assert_eq!(fixture.mixer.active_samples.len(), 2);
        for sample in &fixture.mixer.active_samples {
            assert!(matches!(sample.fade_state, FadeState::Out { .. }));
        }
    }

    #[tokio::test]
    async fn stop_by_voice_selector_fades_only_matching_samples() {
        let mut fixture = Fixture::new(vec![]);
        fixture.run(play(Some("music"), 1.0)).await;
        fixture.run(play(Some("sfx"), 1.0)).await;

        let stop = AudioCommand::Stop {
            selector: SampleSelector {
                internal_id: None,
                id: None,
                file: None,
                voice: Some("music".to_string()),
            },
            fade_out_ms: Some(50),
        };
        fixture.run(stop).await;
        fixture.drain();

        let expected_duration = 50 * SR as usize / 1000; // 2400 frames
        for sample in &fixture.mixer.active_samples {
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
        let mut fixture = Fixture::new(vec![]);
        fixture.run(play(Some("music"), 1.0)).await;
        fixture
            .run(AudioCommand::VoiceVolume {
                voice: "music".to_string(),
                volume: 0.3,
            })
            .await;
        fixture.drain();

        let sample = &fixture.mixer.active_samples[0];
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
        let mut fixture = Fixture::new(vec![rule]);

        // Background music starts first. It is not a ducking primary, so it stays
        // full: the control engine resolves no target change for "music".
        fixture.run(play(Some("music"), 1.0)).await;
        fixture.drain();
        {
            let applier = fixture.mixer.ducking_applier.as_mut().unwrap();
            assert!((applier.get_multiplier("music", 1) - 1.0).abs() < 1e-6);
        }

        // The primary voice starts: handle_command must drive the control-side
        // ducking engine, which emits a SetDuckTarget that ducks "music" once the
        // mixer's applier applies it.
        fixture.run(play(Some("narration"), 1.0)).await;
        fixture.drain();
        {
            let applier = fixture.mixer.ducking_applier.as_mut().unwrap();
            let multiplier = applier.get_multiplier("music", 1);
            assert!(
                multiplier < 0.2,
                "expected music ducked toward 0.1, got {multiplier}"
            );
        }
    }

    #[tokio::test]
    async fn an_active_input_voice_ducks_the_background_voice() {
        // D4: a configured live-input voice used as a ducking primary must duck the
        // background, driven through the same off-RT notify/compute_changes path as
        // sample voices. Activity detection is Sprint 8; here the notify is driven
        // directly (as input setup will, "active while the stream is open").
        let rule = DuckingRule {
            primary_voice: "mic".to_string(),
            ducked_voices: vec!["music".to_string()],
            target_volume: 0.1,
            fade_duration_ms: 0, // instant, for a deterministic assertion
        };
        let mut fixture = Fixture::new(vec![rule]);

        // Background music plays and is not ducked while the mic is idle.
        fixture.run(play(Some("music"), 1.0)).await;
        fixture.drain();
        {
            let applier = fixture.mixer.ducking_applier.as_mut().unwrap();
            assert!((applier.get_multiplier("music", 1) - 1.0).abs() < 1e-6);
        }

        // The mic input voice becomes active: the notify path must emit a duck for
        // "music" that the applier applies.
        fixture.notify_voice("mic", true);
        fixture.drain();
        {
            let applier = fixture.mixer.ducking_applier.as_mut().unwrap();
            let multiplier = applier.get_multiplier("music", 1);
            assert!(
                multiplier < 0.2,
                "an active input primary must duck music toward 0.1, got {multiplier}"
            );
        }

        // The mic goes idle again: music restores toward full volume.
        fixture.notify_voice("mic", false);
        fixture.drain();
        {
            let applier = fixture.mixer.ducking_applier.as_mut().unwrap();
            let multiplier = applier.get_multiplier("music", 1);
            assert!(
                (multiplier - 1.0).abs() < 1e-6,
                "music must restore once the input primary is idle, got {multiplier}"
            );
        }
    }

    /// Move the (single) active sample for `voice` out of the mixer into the
    /// graveyard, mirroring the RT thread reaping a finished sample, so the
    /// control-side reaper can then reconcile it.
    fn finish_voice_sample(fixture: &mut Fixture, voice: &str) {
        let idx = fixture
            .mixer
            .active_samples
            .iter()
            .position(|s| s.voice_id == voice)
            .expect("an active sample for the voice");
        let finished = fixture.mixer.active_samples.swap_remove(idx);
        fixture
            .grave_tx
            .push(finished)
            .ok()
            .expect("graveyard has room");
    }

    #[tokio::test]
    async fn reaper_restores_ducking_and_clears_voice_when_last_sample_finishes() {
        let rule = DuckingRule {
            primary_voice: "narration".to_string(),
            ducked_voices: vec!["music".to_string()],
            target_volume: 0.1,
            fade_duration_ms: 0, // instant, for a deterministic assertion
        };
        let mut fixture = Fixture::new(vec![rule]);

        // Background music + the ducking primary both play and are applied.
        fixture.run(play(Some("music"), 1.0)).await;
        fixture.run(play(Some("narration"), 1.0)).await;
        fixture.drain();

        // Music is ducked while narration is active.
        {
            let applier = fixture.mixer.ducking_applier.as_mut().unwrap();
            assert!(
                applier.get_multiplier("music", 1) < 0.2,
                "music should be ducked while narration plays"
            );
        }
        assert_eq!(fixture.active_counts.get("narration"), Some(&1));
        assert_eq!(fixture.playing.len(), 2);

        // The narration sample finishes: the RT thread hands it to the graveyard.
        finish_voice_sample(&mut fixture, "narration");
        fixture.reap();

        // (a) The voice's count hit 0 and the voice was removed.
        assert!(
            !fixture.active_counts.contains_key("narration"),
            "narration should be cleared from active_counts"
        );
        // (c) The snapshot/playing map dropped the finished sample.
        assert_eq!(fixture.playing.len(), 1, "the finished sample is removed");
        assert!(
            fixture.playing.values().all(|s| s.voice_id == "music"),
            "only the music sample remains in the playing map"
        );
        {
            let snap = fixture.snapshot.read().unwrap();
            assert_eq!(snap.active_samples, 1);
        }

        // (b) A restore SetDuckTarget (target 1.0) for "music" was emitted on the
        // command ring; applying it restores music toward full volume.
        let restore = fixture.cmd_rx.pop().expect("a restore command on the ring");
        match restore {
            rt_engine::AudioCommand::SetDuckTarget(change) => {
                assert_eq!(change.voice, "music");
                assert!(
                    (change.target_volume - 1.0).abs() < 1e-6,
                    "restore target must be 1.0, got {}",
                    change.target_volume
                );
                // Applying the restore and advancing the full fade brings music back
                // to full volume (the restore fades over the resolved restore time).
                let fade = change.fade_frames.max(1);
                let applier = fixture.mixer.ducking_applier.as_mut().unwrap();
                applier.apply_target(&change);
                assert!(
                    (applier.get_multiplier("music", fade) - 1.0).abs() < 1e-6,
                    "music should be restored to full volume after the restore fade"
                );
            }
            other => panic!(
                "expected a SetDuckTarget restore, got {:?}",
                other_variant(&other)
            ),
        }
    }

    #[tokio::test]
    async fn reaper_does_not_restore_while_voice_still_has_samples() {
        let rule = DuckingRule {
            primary_voice: "narration".to_string(),
            ducked_voices: vec!["music".to_string()],
            target_volume: 0.1,
            fade_duration_ms: 0,
        };
        let mut fixture = Fixture::new(vec![rule]);

        // Music plus TWO narration samples (count == 2).
        fixture.run(play(Some("music"), 1.0)).await;
        fixture.run(play(Some("narration"), 1.0)).await;
        fixture.run(play(Some("narration"), 1.0)).await;
        fixture.drain();
        assert_eq!(fixture.active_counts.get("narration"), Some(&2));

        // Drain the duck command the second narration Play may have queued so the
        // ring only holds whatever the reaper emits next.
        while fixture.cmd_rx.pop().is_some() {}

        // One narration sample finishes, but the voice is still active (count -> 1).
        finish_voice_sample(&mut fixture, "narration");
        fixture.reap();

        // The guard holds: the voice is NOT cleared and NO restore is emitted early.
        assert_eq!(
            fixture.active_counts.get("narration"),
            Some(&1),
            "narration still has one sample, count must be 1"
        );
        assert!(
            fixture.cmd_rx.pop().is_none(),
            "no restore should be emitted while the voice still has samples"
        );
        assert_eq!(fixture.playing.len(), 2, "one narration + music remain");
    }

    #[tokio::test]
    async fn reaper_restore_uses_the_rules_fade_duration_not_a_fixed_default() {
        // D25/D3: a 200ms rule must restore over ~200ms end-to-end through the
        // control-side reaper path, not the old hardcoded 2000ms.
        let rule = DuckingRule {
            primary_voice: "narration".to_string(),
            ducked_voices: vec!["music".to_string()],
            target_volume: 0.1,
            fade_duration_ms: 200,
        };
        let mut fixture = Fixture::new(vec![rule]);

        fixture.run(play(Some("music"), 1.0)).await;
        fixture.run(play(Some("narration"), 1.0)).await;
        fixture.drain();
        // Drain the duck commands so the ring only holds what the reaper emits next.
        while fixture.cmd_rx.pop().is_some() {}

        // The narration sample finishes; the reaper emits the restore for "music".
        finish_voice_sample(&mut fixture, "narration");
        fixture.reap();

        let restore = fixture.cmd_rx.pop().expect("a restore command on the ring");
        match restore {
            rt_engine::AudioCommand::SetDuckTarget(change) => {
                assert_eq!(change.voice, "music");
                assert!((change.target_volume - 1.0).abs() < 1e-6);
                // 200ms at 48kHz == 9600 frames, not the old 2000ms (96000 frames).
                assert_eq!(
                    change.fade_frames,
                    (SR / 5) as usize,
                    "restore must use the rule's 200ms fade, got {} frames",
                    change.fade_frames
                );
            }
            other => panic!(
                "expected a SetDuckTarget restore, got {:?}",
                other_variant(&other)
            ),
        }
    }

    /// Describe a command variant for panic messages without requiring `Debug` on
    /// the payloads (which carry non-Debug ring consumers).
    fn other_variant(cmd: &rt_engine::AudioCommand) -> &'static str {
        match cmd {
            rt_engine::AudioCommand::AddSample(_) => "AddSample",
            rt_engine::AudioCommand::AddLiveInput(_) => "AddLiveInput",
            rt_engine::AudioCommand::SetDuckTarget(_) => "SetDuckTarget",
            rt_engine::AudioCommand::FadeOutAll { .. } => "FadeOutAll",
            rt_engine::AudioCommand::FadeOutSamples { .. } => "FadeOutSamples",
            rt_engine::AudioCommand::FadeOutMatching { .. } => "FadeOutMatching",
            rt_engine::AudioCommand::SetVoiceVolume { .. } => "SetVoiceVolume",
            rt_engine::AudioCommand::SetInputVolume { .. } => "SetInputVolume",
            rt_engine::AudioCommand::SeekMatching { .. } => "SeekMatching",
            rt_engine::AudioCommand::SetSpeedMatching { .. } => "SetSpeedMatching",
            rt_engine::AudioCommand::SetVolumeMatching { .. } => "SetVolumeMatching",
        }
    }

    #[tokio::test]
    async fn shutdown_fades_samples_and_flushes_metadata() {
        let mut fixture = Fixture::new(vec![]);
        fixture.run(play(Some("v"), 1.0)).await;

        shutdown(&mut fixture.cmd_tx, &fixture.cache_manager).await;
        fixture.drain();

        // Every active sample now has a fade-out applied (drained to silence by the
        // real callback at runtime; here we just assert the fade was set).
        assert!(!fixture.mixer.active_samples.is_empty());
        for sample in &fixture.mixer.active_samples {
            assert!(matches!(sample.fade_state, FadeState::Out { .. }));
        }
        // Cache metadata was flushed to disk on shutdown.
        let metadata = fixture._cache_dir.path().join("metadata.json");
        assert!(
            metadata.exists(),
            "metadata.json should be flushed on shutdown"
        );
    }
}
