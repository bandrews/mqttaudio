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

/// Build the formatting log layer for the chosen `logging.format`, filtered to
/// `level` and writing through `make_writer` (F13/D40). `"json"` produces
/// line-delimited JSON records for log aggregation; anything else (the validated
/// default `"text"`) produces the human-readable rendering. Boxed so both formats
/// share one type and compose with the optional MQTT layer in the registry.
fn build_fmt_layer<S, W>(
    format: &str,
    level: tracing::Level,
    make_writer: W,
) -> Box<dyn tracing_subscriber::Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    W: for<'w> tracing_subscriber::fmt::MakeWriter<'w> + Send + Sync + 'static,
{
    use tracing_subscriber::Layer;
    let filter = tracing_subscriber::filter::LevelFilter::from_level(level);
    if format == "json" {
        tracing_subscriber::fmt::layer()
            .json()
            .with_writer(make_writer)
            .with_filter(filter)
            .boxed()
    } else {
        tracing_subscriber::fmt::layer()
            .with_writer(make_writer)
            .with_filter(filter)
            .boxed()
    }
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

    // Initialize logging: a console fmt layer (text or JSON per logging.format)
    // plus, when an MQTT log topic is configured, a composable MQTT publish layer.
    // Both sinks live in one registry so the format choice applies regardless of
    // whether MQTT logging is on (F13/D40).
    let mqtt_log_receiver = {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;

        let fmt_layer = build_fmt_layer(&config.logging.format, log_level, std::io::stdout);

        if config.logging.mqtt_topic.is_some() {
            let (sender, receiver) = mqtt::logger::create_log_channel(100);
            let mqtt_layer = mqtt::logger::MqttLogLayer::new(sender, log_level);
            tracing_subscriber::registry()
                .with(fmt_layer)
                .with(mqtt_layer)
                .init();
            Some(receiver)
        } else {
            tracing_subscriber::registry().with(fmt_layer).init();
            None
        }
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

    // Warn (do not fail) if the config declares a schema version newer than this
    // build understands — newer fields may be silently ignored (F4/D38).
    if let Some(warning) = config.schema_version_warning() {
        tracing::warn!("{}", warning);
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

    // Control-side per-voice ducking multiplier snapshot the HTTP handlers read
    // (D40). The control thread updates it through `notify_voice_activity` as
    // ducking targets resolve; the audio thread never touches it (D20). Created
    // before the input loop so configured-input voices that trigger ducking can
    // record their target.
    let ducking_snapshot: Arc<RwLock<HashMap<String, f32>>> = Arc::new(RwLock::new(HashMap::new()));

    // Create bass management from config. Also keep the resolved LFE channel (when
    // enabled) so the input-setup loop can warn about routes that collide with it.
    let mut bass_lfe_channel: Option<usize> = None;
    let bass_management = if config.bass_management.enabled {
        let resolved = match config.resolve_bass_management() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Bass management channel resolution failed: {}", e);
                std::process::exit(1);
            }
        };
        bass_lfe_channel = Some(resolved.lfe_channel);
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
        streamed_sources: Vec::with_capacity(audio::mixer::MAX_STREAMED_SOURCES),
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
    // Bounded well above MAX_STREAMED_SOURCES so the callback can always hand a
    // finished source off for off-RT drop.
    let (streamed_grave_tx, mut streamed_grave_rx) = rt_engine::streamed_graveyard_channel(256);
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

                    // Now that the device's channel count is known, warn about any
                    // route reading a source channel the device does not have; the
                    // mixer would otherwise drop it silently (F5).
                    if let Some(warning) =
                        out_of_range_input_routes(idx, &channel_map, active_input.channels)
                    {
                        tracing::warn!("{}", warning);
                    }

                    // With bass management enabled, warn once if any route lands
                    // content directly on the LFE channel: that content bypasses the
                    // crossover and the extracted bass is summed on top (F7).
                    if let Some(lfe) = bass_lfe_channel {
                        if let Some(warning) = input_routes_collide_with_lfe(idx, &channel_map, lfe)
                        {
                            tracing::warn!("{}", warning);
                        }
                    }

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
                        &ducking_snapshot,
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
        streamed_graveyard: streamed_grave_tx,
        output_sample_rate,
    }));
    let xruns = Arc::new(AtomicU64::new(0));

    let voice_manager = Arc::new(Mutex::new(VoiceManager::new()));

    // Control-side voice activity bookkeeping and the authoritative status
    // snapshot the HTTP handlers read. `active_counts` drives ducking restore;
    // `playing` mirrors the live sample list for status (position excluded).
    let mut active_counts: HashMap<String, usize> = HashMap::new();
    let mut streamed_voices: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut playing: HashMap<u64, http::SampleStatus> = HashMap::new();
    let status_snapshot = Arc::new(RwLock::new(http::StatusSnapshot {
        active_samples: 0,
        output_channels,
        samples: Vec::new(),
        inputs: input_statuses.clone(),
    }));

    // When the daemon started, for the `/metrics` uptime field (D40).
    let start_time = std::time::Instant::now();

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
            xruns.clone(),
            start_time,
            ducking_snapshot.clone(),
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
                            streamed_voices: &mut streamed_voices,
                            playing: &mut playing,
                            snapshot: &status_snapshot,
                            ducking_snapshot: &ducking_snapshot,
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
                    &mut streamed_grave_rx,
                    &mut cmd_return_rx,
                    &mut cmd_tx,
                    ducking_engine.as_mut(),
                    &mut active_counts,
                    &mut streamed_voices,
                    &mut playing,
                    &status_snapshot,
                    &ducking_snapshot,
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

/// Build the warning for an input whose resolved routes read source channels the
/// device does not have, or `None` when every source is within range (F5). The
/// device channel count is only known once the stream opens, so this runs at input
/// setup rather than during config validation; the mixer silently drops such routes,
/// so the warning is what makes the misconfiguration visible. Source channels are
/// 0-based, so a valid index is `< channels`.
fn out_of_range_input_routes(
    input_index: usize,
    channel_map: &[(usize, usize)],
    channels: usize,
) -> Option<String> {
    let mut offending: Vec<usize> = channel_map
        .iter()
        .map(|&(src, _)| src)
        .filter(|&src| src >= channels)
        .collect();
    if offending.is_empty() {
        return None;
    }
    offending.sort_unstable();
    offending.dedup();
    Some(format!(
        "Input {} routes read source channel(s) {:?} but the device has only {} channel(s) \
         (0..{}); those routes will be silently dropped — fix the input's routes config",
        input_index,
        offending,
        channels,
        channels.saturating_sub(1)
    ))
}

/// Build the warning for an input whose resolved routes land content directly on
/// the bass-management LFE channel, or `None` when no route targets it (F7/D41).
/// Content routed straight to the LFE output index is *not* high-passed by the
/// crossover, and bass management then sums the extracted bass on top of it (the
/// additive-LFE behavior) — so the sub carries that full-range content unfiltered.
/// This is a documented footgun, not a hard error: routing to the LFE deliberately
/// is valid, so this warns rather than rejecting. Only meaningful when bass
/// management is enabled; the caller gates on that.
fn input_routes_collide_with_lfe(
    input_index: usize,
    channel_map: &[(usize, usize)],
    lfe_channel: usize,
) -> Option<String> {
    if !channel_map.iter().any(|&(_, dest)| dest == lfe_channel) {
        return None;
    }
    Some(format!(
        "Input {} routes content to the bass-management LFE channel {}; that content is sent to the \
         sub full-range (the crossover is bypassed) and the extracted bass is added on top — route \
         to the LFE deliberately or change the destination",
        input_index, lfe_channel
    ))
}

/// Generate a unique voice id for a Play that did not specify one. The wall-clock
/// millisecond keeps the id human-readable/orderable, and a process-global monotonic
/// counter makes two Plays in the same millisecond distinct (F5/D41) so a later
/// voice_stop/voice_volume/ducking on one does not affect the other.
fn next_auto_voice_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static AUTO_VOICE_COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = AUTO_VOICE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("_auto_{}_{}", millis, n)
}

/// Notify the control-side ducking engine that a voice's activity changed and
/// forward any resulting target changes to the audio thread. Used by both the
/// sample-playback path and the configured-input path (D4), so live-input voices
/// can trigger ducking through the same off-RT `compute_changes` path. A no-op when
/// ducking is not configured.
///
/// Each forwarded change also updates the control-side per-voice ducking snapshot
/// the HTTP `/metrics` and `/status/voices` handlers read (D40): a target below
/// full volume records the voice's multiplier, a restore (>= 1.0) clears it. The
/// snapshot tracks resolved targets, not the in-flight fade, matching what the
/// control thread authoritatively knows (D20).
fn notify_voice_activity(
    voice_id: &str,
    is_active: bool,
    ducking_engine: Option<&mut audio::ducking::DuckingEngine>,
    cmd_tx: &mut rt_engine::CommandProducer,
    ducking_snapshot: &std::sync::Arc<std::sync::RwLock<std::collections::HashMap<String, f32>>>,
) {
    if let Some(engine) = ducking_engine {
        for change in engine.compute_changes(voice_id, is_active) {
            {
                let mut guard = ducking_snapshot.write().unwrap();
                if change.target_volume >= 1.0 {
                    guard.remove(&change.voice);
                } else {
                    guard.insert(change.voice.clone(), change.target_volume);
                }
            }
            if cmd_tx
                .push(rt_engine::AudioCommand::SetDuckTarget(change))
                .is_err()
            {
                tracing::error!("Audio command ring full; duck target dropped");
            }
        }
    }
}

/// Reconcile control-side bookkeeping for one finished voice-bearing entry (a sample
/// or a streamed source): drop it from the live `playing` map and decrement its
/// voice's active count; when that count reaches zero, restore the voice through the
/// shared notify path (which, for a ducking primary, recomputes restore targets sent
/// back to the audio thread). Returns whether the voice's count reached zero (it is
/// now fully idle), so the caller can drop it from the streamed-voice set.
fn reconcile_finished_voice(
    voice: &str,
    id: u64,
    cmd_tx: &mut rt_engine::CommandProducer,
    ducking_engine: Option<&mut audio::ducking::DuckingEngine>,
    active_counts: &mut std::collections::HashMap<String, usize>,
    playing: &mut std::collections::HashMap<u64, http::SampleStatus>,
    ducking_snapshot: &std::sync::Arc<std::sync::RwLock<std::collections::HashMap<String, f32>>>,
) -> bool {
    playing.remove(&id);
    if let Some(count) = active_counts.get_mut(voice) {
        *count -= 1;
        if *count == 0 {
            active_counts.remove(voice);
            notify_voice_activity(voice, false, ducking_engine, cmd_tx, ducking_snapshot);
            return true;
        }
    }
    false
}

/// Drain the sample and streamed-source graveyards and the command-return ring,
/// dropping finished voices and spent mutation commands off the audio thread, and
/// reconcile control-side voice activity. When a voice's last sample or source
/// finishes, the ducking engine computes restore targets that are sent back to the
/// audio thread. Refreshes the status snapshot if anything was reaped.
#[allow(clippy::too_many_arguments)]
fn reap_finished_samples(
    grave_rx: &mut rt_engine::GraveyardConsumer,
    streamed_grave_rx: &mut rt_engine::StreamedGraveyardConsumer,
    cmd_return_rx: &mut rt_engine::CommandReturnConsumer,
    cmd_tx: &mut rt_engine::CommandProducer,
    mut ducking_engine: Option<&mut audio::ducking::DuckingEngine>,
    active_counts: &mut std::collections::HashMap<String, usize>,
    streamed_voices: &mut std::collections::HashSet<String>,
    playing: &mut std::collections::HashMap<u64, http::SampleStatus>,
    snapshot: &std::sync::Arc<std::sync::RwLock<http::StatusSnapshot>>,
    ducking_snapshot: &std::sync::Arc<std::sync::RwLock<std::collections::HashMap<String, f32>>>,
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
        if reconcile_finished_voice(
            &voice,
            id,
            cmd_tx,
            ducking_engine.as_deref_mut(),
            active_counts,
            playing,
            ducking_snapshot,
        ) {
            streamed_voices.remove(&voice);
        }
        reaped += 1;
    }

    // Finished streamed sources reconcile the same way. The off-RT drop frees the
    // source's ring buffer and Arcs here (never in the callback) and, via the
    // source's Drop, signals its producer thread to stop (a no-op once it has already
    // exited at EOF).
    while let Some(finished) = streamed_grave_rx.pop() {
        let voice = finished.voice_id.clone();
        let id = finished.id;
        drop(finished);
        if reconcile_finished_voice(
            &voice,
            id,
            cmd_tx,
            ducking_engine.as_deref_mut(),
            active_counts,
            playing,
            ducking_snapshot,
        ) {
            streamed_voices.remove(&voice);
        }
        reaped += 1;
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
    /// Voices that currently have a windowed/streamed source, so the seek/speed gate
    /// can warn that those commands do not apply to them.
    streamed_voices: &'a mut std::collections::HashSet<String>,
    playing: &'a mut std::collections::HashMap<u64, http::SampleStatus>,
    snapshot: &'a std::sync::Arc<std::sync::RwLock<http::StatusSnapshot>>,
    ducking_snapshot: &'a std::sync::Arc<std::sync::RwLock<std::collections::HashMap<String, f32>>>,
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

/// Start a windowed streamed play of a local file: spawn the bounded-ring producer,
/// gate on a prebuffer for a glitch-free start, then hand a `StreamedSource` to the
/// audio thread. Streamed voices play forward only — seek/loop-crossfade/reverse/
/// speed/pitch do not apply (the producer loops by re-opening; the seek/speed command
/// arms warn for these voices). Bypasses the cache, so it enforces the file allowlist.
#[allow(clippy::too_many_arguments)]
async fn handle_stream_play(
    file: String,
    id: Option<String>,
    volume: f32,
    voice: Option<String>,
    channel_map: Option<Vec<mqtt::commands::ChannelMapping>>,
    fade_in: Option<u32>,
    loop_mode: bool,
    window_ms: Option<u32>,
    prebuffer_ms: Option<u32>,
    ctx: &mut CommandCtx<'_>,
) {
    use std::sync::atomic::Ordering;
    let config = ctx.config;
    let output_sample_rate = ctx.output_sample_rate;

    // A streamed play bypasses the cache, so enforce the file allowlist here.
    if !config::Config::is_path_allowed(
        std::path::Path::new(&file),
        &config.security.allowed_directories,
    ) {
        tracing::error!(
            "Streamed play of {} rejected: not permitted by allowed_directories",
            file
        );
        return;
    }

    let voice_id = voice.unwrap_or_else(next_auto_voice_id);

    // Window + prebuffer (per-play override beats config).
    let window_ms = window_ms.unwrap_or(config.cache.stream_window_ms);
    let prebuffer_ms = prebuffer_ms.unwrap_or(config.cache.stream_prebuffer_ms);
    let deadline_ms = config.cache.stream_prebuffer_deadline_ms.max(prebuffer_ms);
    let rate = output_sample_rate as usize;
    let window_frames = (window_ms as usize * rate / 1000).max(1);
    let prebuffer_frames = prebuffer_ms as usize * rate / 1000;
    let quality = config.advanced.resampler_quality;

    // Open + probe is blocking, so run it off the async runtime; the producer then
    // streams on its own dedicated thread.
    let spawn_file = file.clone();
    let handles = match tokio::task::spawn_blocking(move || {
        audio::streamed_source::spawn_local_file_stream(
            spawn_file,
            output_sample_rate,
            quality,
            window_frames,
            loop_mode,
        )
    })
    .await
    {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            tracing::error!("Failed to start streamed source for {}: {}", file, e);
            return;
        }
        Err(e) => {
            tracing::error!("Streamed source spawn task failed for {}: {}", file, e);
            return;
        }
    };

    // Gate on the prebuffer so the first audio is glitch-free, but never wait past the
    // deadline (start anyway; the underrun fade covers any gap). Polling with a tokio
    // sleep never blocks a worker thread.
    let started = tokio::time::Instant::now();
    let deadline = std::time::Duration::from_millis(deadline_ms as u64);
    loop {
        let buffered = handles.frames_buffered.load(Ordering::Acquire);
        if buffered >= prebuffer_frames || handles.producer_done.load(Ordering::Acquire) {
            break;
        }
        if started.elapsed() >= deadline {
            tracing::warn!(
                "Streamed source {} hit the {}ms prebuffer deadline with {}/{} frames; \
                 starting anyway",
                file,
                deadline_ms,
                buffered,
                prebuffer_frames
            );
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    // Resolve the internal id and the voice's current volume.
    let mut voice_mgr = ctx.voice_manager.lock();
    let internal_id = voice_mgr.add_sample_to_voice(&voice_id);
    let voice_volume = voice_mgr.get_voice_volume(&voice_id).unwrap_or(1.0);
    drop(voice_mgr);

    // Channel routing: resolve aliases if given, else default 1:1 over decoded channels.
    let channels = handles.channels;
    let resolved_map: Vec<(usize, usize)> = match channel_map {
        Some(map) => {
            let mut out = Vec::with_capacity(map.len());
            for m in &map {
                match (
                    config.resolve_channel(&m.src),
                    config.resolve_channel(&m.dest),
                ) {
                    (Ok(s), Ok(d)) => out.push((s, d)),
                    (Err(e), _) | (_, Err(e)) => {
                        tracing::error!("Failed to resolve channel alias for streamed play: {}", e);
                        return;
                    }
                }
            }
            out
        }
        None => (0..channels).map(|c| (c, c)).collect(),
    };

    let mut source = audio::mixer::StreamedSource::new(
        internal_id,
        voice_id.clone(),
        file.clone(),
        id.clone(),
        handles.consumer,
        channels,
        volume,
        resolved_map,
        handles.producer_done,
        handles.stop_flag,
    );
    // Start at the voice's current level (no ramp), like a sample play.
    source.voice_volume = voice_volume;
    source.target_voice_volume = voice_volume;
    if let Some(fade_ms) = fade_in {
        source.set_fade(audio::mixer::FadeState::fade_in(
            fade_ms,
            output_sample_rate,
        ));
    }

    let status = http::SampleStatus {
        internal_id: source.id,
        sample_id: source.sample_id.clone(),
        voice_id: voice_id.clone(),
        file_path: source.file_path.clone(),
        total_frames: 0, // unbounded / unknown for a stream
        sample_rate: output_sample_rate,
        volume: source.volume,
        voice_volume: source.voice_volume,
        speed: 1.0,
        loop_mode,
    };

    let became_active = !ctx.active_counts.contains_key(&voice_id);
    if became_active {
        notify_voice_activity(
            &voice_id,
            true,
            ctx.ducking_engine.as_mut(),
            ctx.cmd_tx,
            ctx.ducking_snapshot,
        );
    }
    *ctx.active_counts.entry(voice_id.clone()).or_insert(0) += 1;
    ctx.playing.insert(status.internal_id, status);
    ctx.streamed_voices.insert(voice_id.clone());
    ctx.send(rt_engine::AudioCommand::AddStreamedSource(source));
    ctx.refresh();
    tracing::info!(
        "Now streaming {} (voice '{}'); {} active voices",
        file,
        voice_id,
        ctx.playing.len()
    );
}

/// Whether a selector targets a voice that currently has a windowed/streamed source.
/// Streamed voices are forward-only, so seek/speed/pitch do not apply. Best-effort:
/// it matches on the selector's voice (the common case) and is used only to warn — a
/// stray seek/speed would be a structural no-op on streamed sources anyway, since they
/// live in a separate voice list the sample-targeted commands never touch.
fn selector_targets_streamed_voice(
    selector: &mqtt::commands::SampleSelector,
    streamed_voices: &std::collections::HashSet<String>,
) -> bool {
    selector
        .voice
        .as_deref()
        .is_some_and(|v| streamed_voices.contains(v))
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
            mode,
            window_ms,
            prebuffer_ms,
        } => {
            // An explicit windowed/streamed play takes the bounded-memory producer
            // path for local files (low time-to-first-sample, O(window) memory). HTTP
            // windowed streaming is S2; until then an HTTP mode=stream falls through to
            // the full load with a note.
            let is_http = file.starts_with("http://") || file.starts_with("https://");
            if mode == config::LoadMode::Stream {
                if is_http {
                    tracing::warn!(
                        "mode=stream is not yet supported for HTTP ({}); loading fully",
                        file
                    );
                } else {
                    handle_stream_play(
                        file,
                        id,
                        volume,
                        voice,
                        channel_map,
                        fade_in,
                        loop_mode,
                        window_ms,
                        prebuffer_ms,
                        ctx,
                    )
                    .await;
                    return;
                }
            }

            // Load file (with streaming support for faster startup)
            let mut cache_mgr = cache_manager.lock().await;
            let buffer_result = cache_mgr
                .get_or_load_streaming(&file, output_sample_rate)
                .await;
            drop(cache_mgr);

            match buffer_result {
                Ok(buffer) => {
                    // Use provided voice or auto-generate a unique one.
                    let voice_id = voice.unwrap_or_else(next_auto_voice_id);

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

                    // The loop crossfade only engages on a loop boundary of a buffer
                    // long enough to hold a crossfade at both ends
                    // (`buffer_frames > crossfade_samples * 2`, see
                    // `mixer::crossfade_active`). Warn when a requested crossfade can
                    // therefore never run, so the silent drop is visible (F8/D41). This
                    // is feedback only — the blend math is unchanged.
                    if crossfade_ms > 0 {
                        if !loop_mode {
                            tracing::warn!(
                                "Play of {} requested crossfade {}ms without loop:true; \
                                 the crossfade only applies at loop boundaries and will be \
                                 ignored",
                                file,
                                crossfade_ms
                            );
                        } else if let Some(total) = buffer.total_frames_or_estimate() {
                            // Judge the length only when the total is known. A streaming
                            // buffer with an unknown total cannot be judged yet (the
                            // mixer defers looping until the buffer is complete, D11), so
                            // skip rather than warn on the small loaded-so-far count.
                            if crossfade_samples * 2 >= total {
                                tracing::warn!(
                                    "Play of {} requested crossfade {}ms ({} frames) but the clip \
                                     is only {} frames; a crossfade needs more than twice its \
                                     length and will be ignored",
                                    file,
                                    crossfade_ms,
                                    crossfade_samples,
                                    total
                                );
                            }
                        }
                    }

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
                            ctx.ducking_snapshot,
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

            // A live input shares this voice id even when no sample-backed voice
            // exists. The control plane cannot read the audio thread's live_inputs
            // (D22a), so it checks the configured input voice ids it does own (D35).
            let input_match = ctx.inputs.iter().any(|i| i.voice_id == voice);

            if success || input_match {
                // For a sample-backed voice send the VoiceManager's clamped stored
                // value; for an input-only voice clamp the requested value directly.
                let target = if success {
                    actual_volume
                } else {
                    new_volume.clamp(0.0, 1.0)
                };
                // The audio thread ramps samples and live inputs in this voice
                // toward the new target (SetVoiceVolume covers both).
                ctx.send(rt_engine::AudioCommand::SetVoiceVolume {
                    voice: voice.clone(),
                    volume: target,
                });
                // Keep the status snapshot's per-sample voice volume in step.
                for status in ctx.playing.values_mut() {
                    if status.voice_id == voice {
                        status.voice_volume = target;
                    }
                }
                ctx.refresh();
                tracing::info!("Set voice '{}' volume to {:.2}", voice, target);
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
            // Mute stores the input's current volume and zeroes it; unmute restores
            // that stored volume (D34) — the audio thread owns the live input, so the
            // save/restore happens there.
            tracing::info!(
                "Input '{}' {}",
                input,
                if mute { "muted" } else { "unmuted" }
            );
            ctx.send(rt_engine::AudioCommand::SetInputMute { input, mute });
        }
        mqtt::commands::AudioCommand::Seek {
            selector,
            position_ms,
        } => {
            if selector.is_empty() {
                tracing::warn!("Seek command with empty selector - no samples targeted");
            } else if selector_targets_streamed_voice(&selector, ctx.streamed_voices) {
                tracing::warn!(
                    "Seek is not supported for windowed/streamed voices; ignoring (a streamed \
                     source plays forward only)"
                );
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
            } else if selector_targets_streamed_voice(&selector, ctx.streamed_voices) {
                tracing::warn!(
                    "Speed/pitch is not supported for windowed/streamed voices; ignoring (a \
                     streamed source plays forward only)"
                );
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
        streamed_grave_tx: rt_engine::StreamedGraveyardProducer,
        streamed_grave_rx: rt_engine::StreamedGraveyardConsumer,
        mixer: MixerState,
        ducking_engine: Option<DuckingEngine>,
        active_counts: HashMap<String, usize>,
        streamed_voices: std::collections::HashSet<String>,
        playing: HashMap<u64, http::SampleStatus>,
        snapshot: Arc<RwLock<http::StatusSnapshot>>,
        ducking_snapshot: Arc<RwLock<HashMap<String, f32>>>,
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
            let (streamed_grave_tx, streamed_grave_rx) = rt_engine::streamed_graveyard_channel(256);
            Self {
                cache_manager: Arc::new(tokio::sync::Mutex::new(cache)),
                voice_manager: Arc::new(Mutex::new(VoiceManager::new())),
                cmd_tx,
                cmd_rx,
                cmd_return_tx,
                cmd_return_rx,
                grave_tx,
                grave_rx,
                streamed_grave_tx,
                streamed_grave_rx,
                mixer,
                ducking_engine,
                active_counts: HashMap::new(),
                streamed_voices: std::collections::HashSet::new(),
                playing: HashMap::new(),
                snapshot: Arc::new(RwLock::new(http::StatusSnapshot {
                    active_samples: 0,
                    output_channels: 2,
                    samples: Vec::new(),
                    inputs: Vec::new(),
                })),
                ducking_snapshot: Arc::new(RwLock::new(HashMap::new())),
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
                streamed_voices: &mut self.streamed_voices,
                playing: &mut self.playing,
                snapshot: &self.snapshot,
                ducking_snapshot: &self.ducking_snapshot,
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
                &mut self.streamed_grave_rx,
                &mut self.cmd_return_rx,
                &mut self.cmd_tx,
                self.ducking_engine.as_mut(),
                &mut self.active_counts,
                &mut self.streamed_voices,
                &mut self.playing,
                &self.snapshot,
                &self.ducking_snapshot,
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
                &self.ducking_snapshot,
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
            mode: config::LoadMode::Auto,
            window_ms: None,
            prebuffer_ms: None,
        }
    }

    /// A windowed/streamed Play (mode=stream) of the test WAV into `voice`.
    fn play_stream(voice: Option<&str>) -> AudioCommand {
        AudioCommand::Play {
            file: TEST_WAV.to_string(),
            id: None,
            volume: 1.0,
            voice: voice.map(str::to_string),
            channel_map: None,
            fade_in: None,
            start_position_ms: None,
            loop_mode: false,
            crossfade_ms: 0,
            mode: config::LoadMode::Stream,
            window_ms: Some(200),
            prebuffer_ms: Some(20),
        }
    }

    #[test]
    fn auto_voice_ids_are_unique_within_a_millisecond() {
        // F5/D41: two Plays with no explicit voice must get distinct ids even when
        // generated in the same millisecond. `next_auto_voice_id` is called back to
        // back here (no sleep), so the millisecond component is identical; only the
        // appended monotonic counter makes them differ. The old `_auto_<millis>`
        // form would collide and merge the two sounds under one voice.
        let a = next_auto_voice_id();
        let b = next_auto_voice_id();
        assert_ne!(
            a, b,
            "same-millisecond auto voice ids must differ: {a} vs {b}"
        );
        assert!(
            a.starts_with("_auto_"),
            "auto voice id must keep the _auto_ prefix, got {a}"
        );
    }

    #[tokio::test]
    async fn two_no_voice_plays_get_distinct_voice_ids() {
        // End-to-end through handle_command: two Plays without a `voice` must land in
        // two distinct voices so a later voice_stop/voice_volume/ducking on one does
        // not affect the other.
        let mut fixture = Fixture::new(vec![]);
        fixture.run(play(None, 1.0)).await;
        fixture.run(play(None, 1.0)).await;
        fixture.drain();

        assert_eq!(fixture.mixer.active_samples.len(), 2);
        let v0 = &fixture.mixer.active_samples[0].voice_id;
        let v1 = &fixture.mixer.active_samples[1].voice_id;
        assert_ne!(v0, v1, "two no-voice Plays must get distinct voice ids");
    }

    fn seek_voice(voice: &str, position_ms: u64) -> AudioCommand {
        AudioCommand::Seek {
            selector: mqtt::commands::SampleSelector {
                internal_id: None,
                id: None,
                file: None,
                voice: Some(voice.to_string()),
            },
            position_ms,
        }
    }

    #[tokio::test]
    async fn stream_play_emits_add_streamed_source_and_tracks_the_voice() {
        let mut fixture = Fixture::new(vec![]);
        fixture.run(play_stream(Some("bed"))).await;
        fixture.drain();

        // The windowed path adds a StreamedSource, not an ActiveSample.
        assert_eq!(fixture.mixer.streamed_sources.len(), 1);
        assert!(fixture.mixer.active_samples.is_empty());
        assert_eq!(fixture.mixer.streamed_sources[0].voice_id, "bed");
        // The voice is tracked so the seek/speed gate can warn for it.
        assert!(fixture.streamed_voices.contains("bed"));
    }

    #[tokio::test]
    async fn seek_on_a_streamed_voice_is_gated_and_pushes_nothing() {
        let mut fixture = Fixture::new(vec![]);
        fixture.run(play_stream(Some("bed"))).await;
        fixture.drain(); // consume the AddStreamedSource; the ring is now empty

        // A seek targeting the streamed voice is skipped (the gate warns instead of
        // pushing a SeekMatching that would be a no-op on a forward-only ring).
        fixture.run(seek_voice("bed", 1000)).await;
        assert!(
            fixture.cmd_rx.pop().is_none(),
            "seek on a streamed voice must push no command"
        );

        // A seek on a normal voice still pushes (positive control).
        fixture.run(seek_voice("other", 1000)).await;
        assert!(
            matches!(
                fixture.cmd_rx.pop(),
                Some(rt_engine::AudioCommand::SeekMatching { .. })
            ),
            "seek on a normal voice must still push SeekMatching"
        );
    }

    /// A Play of the test WAV with explicit `loop_mode`/`crossfade_ms`, for the
    /// crossfade-without-loop warning (F8).
    fn play_crossfade(loop_mode: bool, crossfade_ms: u32) -> AudioCommand {
        AudioCommand::Play {
            file: TEST_WAV.to_string(),
            id: None,
            volume: 1.0,
            voice: Some("v".to_string()),
            channel_map: None,
            fade_in: None,
            start_position_ms: None,
            loop_mode,
            crossfade_ms,
            mode: config::LoadMode::Auto,
            window_ms: None,
            prebuffer_ms: None,
        }
    }

    /// Captures the `message` field of a tracing event into a shared buffer.
    struct CaptureVisitor<'a> {
        message: &'a mut String,
    }

    impl tracing::field::Visit for CaptureVisitor<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                *self.message = format!("{value:?}");
            }
        }
    }

    /// Records WARN-and-above event messages so a test can assert on emitted log
    /// output without it leaking to the console (keeps test output pristine).
    struct CaptureLayer {
        warnings: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl<S> tracing_subscriber::Layer<S> for CaptureLayer
    where
        S: tracing::Subscriber,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if *event.metadata().level() > tracing::Level::WARN {
                return;
            }
            let mut message = String::new();
            event.record(&mut CaptureVisitor {
                message: &mut message,
            });
            self.warnings.lock().unwrap().push(message);
        }
    }

    /// Serializes the WARN-capturing tests: `set_default` installs a thread-local
    /// subscriber, and tokio may schedule async tests on shared worker threads, so
    /// without this lock one capturing test could observe another's events.
    static MAIN_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Run `cmd` through `handle_command` under a WARN-capturing subscriber and
    /// return every captured warning message.
    async fn run_capturing_warnings(fixture: &mut Fixture, cmd: AudioCommand) -> Vec<String> {
        use tracing_subscriber::layer::SubscriberExt;
        let warnings = Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(CaptureLayer {
            warnings: warnings.clone(),
        });
        // `with_default` needs a sync scope, but `handle_command` is async; use a
        // dispatcher guard that stays in scope across the await.
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);
        fixture.run(cmd).await;
        let captured = warnings.lock().unwrap().clone();
        captured
    }

    #[tokio::test]
    async fn crossfade_without_loop_warns() {
        // F8: a Play with crossfade_ms but no loop:true silently drops the crossfade
        // (the blend only runs on loop boundaries). Dispatch must warn so the user
        // gets feedback. Use a Mutex to serialize the thread-local subscriber.
        let _serial = MAIN_TEST_LOCK.lock().await;
        let mut fixture = Fixture::new(vec![]);
        let warnings = run_capturing_warnings(&mut fixture, play_crossfade(false, 100)).await;
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("crossfade") && w.contains("loop")),
            "a crossfade without loop must warn about the loop requirement, got {warnings:?}"
        );
    }

    #[tokio::test]
    async fn crossfade_with_loop_within_buffer_does_not_warn() {
        // The happy path: loop:true with a crossfade well under half the buffer
        // (the 2s test WAV is ~96000 frames; 100ms crossfade is ~4800 frames) must
        // NOT warn (pristine output).
        let _serial = MAIN_TEST_LOCK.lock().await;
        let mut fixture = Fixture::new(vec![]);
        let warnings = run_capturing_warnings(&mut fixture, play_crossfade(true, 100)).await;
        assert!(
            warnings.is_empty(),
            "a valid looped crossfade must not warn, got {warnings:?}"
        );
    }

    #[tokio::test]
    async fn crossfade_longer_than_half_buffer_warns_even_with_loop() {
        // Even with loop:true, a crossfade whose 2x exceeds the buffer length never
        // engages in the mixer (it needs buffer_frames > crossfade_samples * 2). The
        // 2s test WAV is ~2000ms; a 5000ms crossfade is far past half, so it is
        // silently dropped and must warn.
        let _serial = MAIN_TEST_LOCK.lock().await;
        let mut fixture = Fixture::new(vec![]);
        let warnings = run_capturing_warnings(&mut fixture, play_crossfade(true, 5000)).await;
        assert!(
            warnings.iter().any(|w| w.contains("crossfade")),
            "a crossfade longer than half the buffer must warn, got {warnings:?}"
        );
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
    async fn voice_volume_reaches_an_input_only_voice() {
        // D35: a VoiceVolume targeting a voice that has only a live input (no
        // sample-backed voice in the VoiceManager) must still ramp that input. The
        // handler used to gate the SetVoiceVolume send behind a VoiceManager hit and
        // silently no-op for input-only voices.
        let mut fixture = Fixture::new(vec![]);
        push_live_input(&mut fixture, "mic", 1.0);

        fixture
            .run(AudioCommand::VoiceVolume {
                voice: "mic".to_string(),
                volume: 0.4,
            })
            .await;
        fixture.drain();

        let input = &fixture.mixer.live_inputs[0];
        assert!(
            (input.target_voice_volume - 0.4).abs() < 1e-6,
            "an input-only voice's target_voice_volume must update, got {}",
            input.target_voice_volume
        );
        // The ramp is gradual, so the current value has not jumped to the target.
        assert!((input.voice_volume - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn voice_volume_still_updates_a_sample_backed_voice() {
        // The input-only path must not regress the normal sample case: a VoiceVolume
        // on a voice with a playing sample still ramps that sample.
        let mut fixture = Fixture::new(vec![]);
        fixture.run(play(Some("music"), 1.0)).await;
        fixture.drain();

        fixture
            .run(AudioCommand::VoiceVolume {
                voice: "music".to_string(),
                volume: 0.25,
            })
            .await;
        fixture.drain();

        assert!((fixture.mixer.active_samples[0].target_voice_volume - 0.25).abs() < 1e-6);
    }

    /// Build a mono live input on `voice` at `volume` with an empty ring and push
    /// it onto the fixture's mixer, registering a matching control-side
    /// `InputStatus` so input-targeting commands (which resolve input voice ids on
    /// the control side, D22a) can see it. The ring content is irrelevant to the
    /// control-plane tests.
    fn push_live_input(fixture: &mut Fixture, voice: &str, volume: f32) {
        use ringbuf::HeapRb;
        let consumer = HeapRb::<f32>::new(16).split().1;
        let index = fixture.mixer.live_inputs.len();
        fixture.mixer.live_inputs.push(audio::mixer::LiveInput::new(
            voice.to_string(),
            consumer,
            1,
            volume,
            vec![(0, 0)],
        ));
        fixture.inputs.push(http::InputStatus {
            index,
            voice_id: voice.to_string(),
            volume,
            channels: 1,
        });
    }

    #[test]
    fn out_of_range_input_route_produces_a_warning() {
        // F5: once the device channel count is known, a route whose SOURCE channel is
        // >= that count is a misconfiguration (the mixer silently drops it). It must
        // produce a warning naming the offending channel(s) and the device count.
        // A 2-channel device with a route reading source channel 3 is out of range.
        let warning = out_of_range_input_routes(1, &[(0, 0), (3, 1)], 2)
            .expect("an out-of-range source channel must warn");
        assert!(
            warning.contains('3') && warning.contains('2'),
            "warning must name the out-of-range source channel and the device count: {warning}"
        );
    }

    #[test]
    fn in_range_input_routes_do_not_warn() {
        // Every source channel within the device count: no warning (pristine output).
        assert!(
            out_of_range_input_routes(0, &[(0, 0), (1, 1)], 2).is_none(),
            "in-range routes must not warn"
        );
        // Boundary: channel index == count is out of range (0-based); count-1 is the
        // last valid index.
        assert!(out_of_range_input_routes(0, &[(1, 0)], 2).is_none());
        assert!(out_of_range_input_routes(0, &[(2, 0)], 2).is_some());
    }

    #[test]
    fn input_route_to_lfe_channel_produces_a_warning() {
        // F7/D41: with bass management enabled, an input route whose DESTINATION is
        // the LFE channel lands full-range content on the sub unfiltered; bass
        // management then *adds* extracted bass on top (the crossover is bypassed for
        // that directly-routed content). It must warn, naming the offending channel.
        // LFE is channel 3; a route 0->3 collides.
        let warning = input_routes_collide_with_lfe(2, &[(0, 0), (0, 3)], 3)
            .expect("a route destined for the LFE channel must warn");
        assert!(
            warning.contains('3'),
            "warning must name the LFE channel, got {warning}"
        );
    }

    #[test]
    fn input_routes_clear_of_lfe_do_not_warn() {
        // No route destined for the LFE channel: pristine output. LFE is 3; routes go
        // to 0 and 1.
        assert!(
            input_routes_collide_with_lfe(0, &[(0, 0), (1, 1)], 3).is_none(),
            "routes that avoid the LFE channel must not warn"
        );
    }

    #[tokio::test]
    async fn input_mute_unmute_restores_the_calibrated_volume() {
        // D34: a calibrated input volume must survive a mute/unmute round-trip —
        // unmute restores the prior level (0.7), not a hardcoded 1.0.
        let mut fixture = Fixture::new(vec![]);
        push_live_input(&mut fixture, "mic", 1.0);

        // Calibrate the input to 0.7 via InputVolume.
        fixture
            .run(AudioCommand::InputVolume {
                input: "mic".to_string(),
                volume: 0.7,
            })
            .await;
        fixture.drain();
        assert!((fixture.mixer.live_inputs[0].volume - 0.7).abs() < 1e-6);

        // Mute: the input drops to silence.
        fixture
            .run(AudioCommand::InputMute {
                input: "mic".to_string(),
                mute: true,
            })
            .await;
        fixture.drain();
        assert_eq!(fixture.mixer.live_inputs[0].volume, 0.0);

        // Unmute: the calibrated 0.7 is restored, NOT 1.0.
        fixture
            .run(AudioCommand::InputMute {
                input: "mic".to_string(),
                mute: false,
            })
            .await;
        fixture.drain();
        assert!(
            (fixture.mixer.live_inputs[0].volume - 0.7).abs() < 1e-6,
            "unmute must restore the calibrated 0.7, got {}",
            fixture.mixer.live_inputs[0].volume
        );
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
    async fn control_thread_records_ducking_into_the_status_snapshot() {
        // F3/D40: the control-side ducking snapshot the HTTP `/metrics` and
        // `/status/voices` handlers read must be populated by the control thread as
        // targets resolve, and cleared when a voice restores — proving the value
        // surfaced over HTTP is the real resolved target, not a placeholder.
        let rule = DuckingRule {
            primary_voice: "narration".to_string(),
            ducked_voices: vec!["music".to_string()],
            target_volume: 0.1,
            fade_duration_ms: 0,
        };
        let mut fixture = Fixture::new(vec![rule]);

        // Music alone is not ducked: the snapshot stays empty.
        fixture.run(play(Some("music"), 1.0)).await;
        fixture.drain();
        assert!(
            fixture.ducking_snapshot.read().unwrap().is_empty(),
            "no voice is ducked yet"
        );

        // The primary plays: the snapshot records music's resolved target (0.1).
        fixture.run(play(Some("narration"), 1.0)).await;
        fixture.drain();
        {
            let snap = fixture.ducking_snapshot.read().unwrap();
            let music = snap
                .get("music")
                .copied()
                .expect("music must be recorded as ducked");
            assert!(
                (music - 0.1).abs() < 1e-6,
                "the snapshot must hold the resolved target 0.1, got {music}"
            );
            assert!(
                !snap.contains_key("narration"),
                "the primary voice is not itself ducked"
            );
        }

        // The narration sample finishes and is reaped: music restores, and the
        // snapshot drops it (a restored voice is at full volume, so absent).
        finish_voice_sample(&mut fixture, "narration");
        fixture.reap();
        assert!(
            !fixture
                .ducking_snapshot
                .read()
                .unwrap()
                .contains_key("music"),
            "a restored voice must be cleared from the ducking snapshot"
        );
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

    /// Push a finished streamed source for `voice` into the streamed graveyard,
    /// mirroring the RT thread reaping it, so the control-side reaper can reconcile it.
    fn finish_streamed_source(fixture: &mut Fixture, voice: &str, id: u64) {
        use std::sync::atomic::AtomicBool;
        let (_producer, consumer) = ringbuf::HeapRb::<f32>::new(16).split();
        let source = audio::mixer::StreamedSource::new(
            id,
            voice.to_string(),
            "stream.wav".to_string(),
            None,
            consumer,
            2,
            1.0,
            vec![(0, 0), (1, 1)],
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(false)),
        );
        fixture
            .streamed_grave_tx
            .push(source)
            .ok()
            .expect("streamed graveyard has room");
    }

    #[tokio::test]
    async fn reaper_reconciles_a_finished_streamed_source() {
        let mut fixture = Fixture::new(vec![]);

        // A streamed voice is active (as the Play stream branch will register it).
        fixture.active_counts.insert("bed".to_string(), 1);

        // The RT thread hands a finished streamed source to the streamed graveyard;
        // the control-side reaper drains it and clears the voice's active count.
        finish_streamed_source(&mut fixture, "bed", 42);
        fixture.reap();

        assert!(
            !fixture.active_counts.contains_key("bed"),
            "a finished streamed source must clear its voice's active count off-RT"
        );
    }

    #[tokio::test]
    async fn reaper_restores_ducking_when_last_streamed_source_finishes() {
        // A streamed source acting as a ducking primary must restore the ducked voice
        // when it finishes, through the same notify path as a sample (the reaper
        // reconciles samples and streamed sources identically).
        let rule = DuckingRule {
            primary_voice: "bed".to_string(),
            ducked_voices: vec!["music".to_string()],
            target_volume: 0.1,
            fade_duration_ms: 0,
        };
        let mut fixture = Fixture::new(vec![rule]);

        // Music plays; the streamed "bed" primary becomes active and ducks music.
        fixture.run(play(Some("music"), 1.0)).await;
        fixture.drain();
        fixture.active_counts.insert("bed".to_string(), 1);
        fixture.notify_voice("bed", true);
        fixture.drain();
        {
            let applier = fixture.mixer.ducking_applier.as_mut().unwrap();
            assert!(
                applier.get_multiplier("music", 1) < 0.2,
                "music should be ducked while the streamed bed is active"
            );
        }

        // The streamed bed finishes: the reaper restores music toward full volume.
        finish_streamed_source(&mut fixture, "bed", 7);
        fixture.reap();
        fixture.drain();
        {
            let applier = fixture.mixer.ducking_applier.as_mut().unwrap();
            assert!(
                (applier.get_multiplier("music", 1) - 1.0).abs() < 1e-6,
                "music must restore once the streamed bed finishes"
            );
        }
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
            rt_engine::AudioCommand::AddStreamedSource(_) => "AddStreamedSource",
            rt_engine::AudioCommand::SetDuckTarget(_) => "SetDuckTarget",
            rt_engine::AudioCommand::FadeOutAll { .. } => "FadeOutAll",
            rt_engine::AudioCommand::FadeOutSamples { .. } => "FadeOutSamples",
            rt_engine::AudioCommand::FadeOutMatching { .. } => "FadeOutMatching",
            rt_engine::AudioCommand::SetVoiceVolume { .. } => "SetVoiceVolume",
            rt_engine::AudioCommand::SetInputVolume { .. } => "SetInputVolume",
            rt_engine::AudioCommand::SetInputMute { .. } => "SetInputMute",
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

    /// A `MakeWriter` that appends every formatted log record to a shared byte
    /// buffer, so a test can inspect exactly what a tracing layer wrote.
    #[derive(Clone)]
    struct BufferWriter(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for BufferWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufferWriter {
        type Writer = BufferWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn json_logging_format_emits_line_delimited_json() {
        // F13: with logging.format = "json", each record the fmt layer writes must
        // be a standalone JSON object on its own line (line-delimited JSON), so a
        // log aggregator can parse it. Capture the layer's output and assert every
        // non-empty line parses as JSON carrying the fields we emitted.
        use tracing_subscriber::layer::SubscriberExt;

        let buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
        let layer = build_fmt_layer("json", tracing::Level::INFO, BufferWriter(buffer.clone()));
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(voice = "music", "first event");
            tracing::warn!("second event");
        });

        let output = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(
            lines.len(),
            2,
            "two events => two JSON lines, got: {output:?}"
        );

        for line in &lines {
            let value: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("each line must be valid JSON ({e}): {line:?}"));
            assert!(
                value.get("level").is_some(),
                "a JSON log record carries a level field: {line}"
            );
            assert!(
                value.get("fields").and_then(|f| f.get("message")).is_some(),
                "a JSON log record carries the message under fields: {line}"
            );
        }

        // The structured field we attached to the first event is present in JSON.
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(
            first["fields"]["voice"], "music",
            "structured fields are serialized in JSON mode: {}",
            lines[0]
        );
    }

    #[test]
    fn text_logging_format_is_not_json() {
        // The default text format is human-readable, not JSON: a captured line must
        // NOT parse as a JSON object (it is the plain fmt rendering).
        use tracing_subscriber::layer::SubscriberExt;

        let buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
        let layer = build_fmt_layer("text", tracing::Level::INFO, BufferWriter(buffer.clone()));
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("a human line");
        });

        let output = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        assert!(
            output.contains("a human line"),
            "text output carries the message"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(output.trim()).is_err(),
            "text format must not be JSON, got: {output:?}"
        );
    }
}
