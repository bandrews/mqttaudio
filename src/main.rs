// ABOUTME: Entry point for mqttaudio MQTT-controlled audio daemon.
// ABOUTME: Handles CLI parsing, initialization, and main event loop.

mod audio;
mod cache;
mod config;
mod mqtt;
mod voice;

use clap::Parser;
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
}

fn main() {
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

    // TODO: Phase 2 - Set up audio decoding
    // TODO: Phase 5 - Connect to MQTT
    // TODO: Phase 6+ - Implement full audio engine

    tracing::info!("mqttaudio initialized successfully");
    tracing::info!("Press Ctrl+C to exit");

    // For now, just wait for Ctrl+C
    let (tx, rx) = std::sync::mpsc::channel();
    ctrlc::set_handler(move || {
        tx.send(()).expect("Could not send signal on channel");
    })
    .expect("Error setting Ctrl-C handler");

    rx.recv().expect("Could not receive from channel");
    tracing::info!("Shutting down...");
}
