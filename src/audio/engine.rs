// ABOUTME: Audio engine coordinator managing playback, caching, and state.
// ABOUTME: Handles sample loading, voice management, and mixer state updates.

use crate::audio::device::output_device_identifier;
use crate::audio::types::DeviceConfig;
use crate::rt_engine::AudioCallbackState;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// List available audio output devices
pub fn list_devices() {
    use super::device::format_device_list;

    print!("{}", format_device_list(&output_device_list()));
}

/// Enumerate output devices with their capabilities. On Linux this probes ALSA
/// directly for native hardware capabilities; elsewhere it queries cpal.
#[cfg(target_os = "linux")]
pub fn output_device_list() -> super::device::DeviceList {
    super::alsa_probe::probe_alsa_devices()
}

/// Enumerate output devices with their capabilities. On Linux this probes ALSA
/// directly for native hardware capabilities; elsewhere it queries cpal.
#[cfg(not(target_os = "linux"))]
pub fn output_device_list() -> super::device::DeviceList {
    use super::device::{DeviceCategory, DeviceInfo, DeviceList};

    let host = cpal::default_host();
    let mut list = DeviceList::new();

    match host.output_devices() {
        Ok(devices) => {
            for device in devices {
                if let Ok(name) = device
                    .description()
                    .map(|d| output_device_identifier(&d, cfg!(target_os = "linux")).to_string())
                {
                    let mut info = DeviceInfo::new(name.clone(), DeviceCategory::Hardware);

                    // Query supported configs
                    if let Ok(configs) = device.supported_output_configs() {
                        let mut max_channels = 0u16;
                        let mut min_rate = u32::MAX;
                        let mut max_rate = 0u32;

                        const MAX_REASONABLE_SAMPLE_RATE: u32 = 384000;

                        for config in configs {
                            let config_max_rate = config.max_sample_rate();
                            if config_max_rate > MAX_REASONABLE_SAMPLE_RATE {
                                continue;
                            }

                            max_channels = max_channels.max(config.channels());
                            min_rate = min_rate.min(config.min_sample_rate());
                            max_rate = max_rate.max(config_max_rate);
                        }

                        if max_channels > 0 {
                            info.cpal_channels = Some(max_channels);
                            info.cpal_sample_rate_min = Some(min_rate);
                            info.cpal_sample_rate_max = Some(max_rate);
                        }
                    }

                    // Fallback to default config
                    if info.cpal_channels.is_none() {
                        if let Ok(config) = device.default_output_config() {
                            info.cpal_channels = Some(config.channels());
                            info.cpal_sample_rate_min = Some(config.sample_rate());
                            info.cpal_sample_rate_max = Some(config.sample_rate());
                        }
                    }

                    list.devices.push(info);
                }
            }
        }
        Err(e) => {
            list.discovery_notes
                .push(format!("Error listing devices: {}", e));
        }
    }

    list
}

/// Get default device configuration
pub fn get_default_device_config() -> Result<DeviceConfig, Box<dyn std::error::Error>> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No default output device available")?;

    let config = device.default_output_config()?;

    Ok(DeviceConfig {
        sample_rate: config.sample_rate(),
        channels: config.channels() as usize,
        buffer_size: 512, // Default buffer size
    })
}

/// Find an output device by name (returns default if name is None)
/// Linux matches ALSA PCM IDs; other platforms match human-readable names.
pub fn find_output_device(name: Option<&str>) -> Result<cpal::Device, Box<dyn std::error::Error>> {
    let host = cpal::default_host();

    match name {
        Some(device_name) => {
            // First try to find in enumerated devices. The ALSA null device
            // discards audio and is never matched.
            if let Ok(devices) = host.output_devices() {
                for device in devices {
                    if let Ok(n) = device.description().map(|d| {
                        output_device_identifier(&d, cfg!(target_os = "linux")).to_string()
                    }) {
                        if n != "null" && n == device_name {
                            return Ok(device);
                        }
                    }
                }
            }

            // On ALSA, try partial matching for device names
            // This handles cases like "hw:1,0" matching "hw:CARD=UMC1820,DEV=0"
            // or "plughw:1,0" matching "plughw:CARD=UMC1820,DEV=0"
            #[cfg(target_os = "linux")]
            {
                if let Ok(devices) = host.output_devices() {
                    for device in devices {
                        if let Ok(n) = device.description().map(|d| {
                            output_device_identifier(&d, cfg!(target_os = "linux")).to_string()
                        }) {
                            // Try matching ALSA device names by card number
                            // Supports hw:, plughw:, sysdefault:
                            if let Some(matched) =
                                super::device::try_match_alsa_device(device_name, &n)
                            {
                                if matched {
                                    tracing::info!(
                                        "Matched ALSA device '{}' to '{}'",
                                        device_name,
                                        n
                                    );
                                    return Ok(device);
                                }
                            }
                        }
                    }
                }
            }

            let mut message = format!(
                "Output device not found: {}. Run mqttaudio --list-devices and copy a Device ID exactly into --device or audio.device.",
                device_name
            );
            if cfg!(target_os = "linux") {
                message.push_str(
                    " On Linux, use the ALSA ID (for example plughw:CARD=HD,DEV=0), not the human-readable description.",
                );
            }
            Err(message.into())
        }
        None => host
            .default_output_device()
            .ok_or_else(|| "No default output device available".into()),
    }
}

/// The output configuration chosen for a device: the stream config plus the
/// sample format the typed callback must produce.
pub struct OutputConfig {
    pub stream_config: cpal::StreamConfig,
    pub sample_format: cpal::SampleFormat,
}

/// Find an output configuration for the device — channel count, sample format,
/// sample rate, and buffer size — delegating the choices to the pure helpers in
/// `device_select` and validating the result against the device's supported
/// configs before returning. This negotiates non-f32 and discrete-rate devices
/// correctly instead of crashing at stream-build time.
///
/// `requested_channels`/`requested_sample_rate` of `None` use the device's
/// maximum/preferred values. `requested_buffer_size` is honored when the device
/// supports it, otherwise the device default is used.
pub fn find_output_config(
    device: &cpal::Device,
    requested_channels: Option<usize>,
    requested_sample_rate: Option<u32>,
    requested_buffer_size: u32,
) -> Result<OutputConfig, Box<dyn std::error::Error>> {
    use crate::audio::device_select::{
        select_buffer_size, select_channels, select_sample_format, select_sample_rate,
        BufferLimits, ConfigOption, SelectError,
    };

    let supported: Vec<_> = device.supported_output_configs()?.collect();
    if supported.is_empty() {
        return Err("No supported output configurations found".into());
    }

    let options: Vec<ConfigOption> = supported
        .iter()
        .map(|c| ConfigOption {
            channels: c.channels(),
            sample_format: c.sample_format(),
            min_rate: c.min_sample_rate(),
            max_rate: c.max_sample_rate(),
            buffer: match c.buffer_size() {
                cpal::SupportedBufferSize::Range { min, max } => BufferLimits::Range {
                    min: *min,
                    max: *max,
                },
                cpal::SupportedBufferSize::Unknown => BufferLimits::Unknown,
            },
        })
        .collect();

    let channels = select_channels(&options, requested_channels)?;
    let sample_format =
        select_sample_format(&options, channels).ok_or(SelectError::NoFormat { channels })?;

    // Rate spans for the chosen (channels, format).
    let ranges: Vec<(u32, u32)> = options
        .iter()
        .filter(|o| o.channels == channels && o.sample_format == sample_format)
        .map(|o| (o.min_rate, o.max_rate))
        .collect();

    let requested_rate = requested_sample_rate.unwrap_or_else(|| {
        device
            .default_output_config()
            .map(|c| c.sample_rate())
            .unwrap_or(48000)
    });

    // On ALSA a raw `hw:` device may advertise a continuous range but accept
    // only discrete rates; prefer the probed discrete set when available.
    #[cfg(target_os = "linux")]
    let discrete = crate::audio::alsa_probe::discrete_rates_for(
        &device
            .description()
            .map(|d| output_device_identifier(&d, true).to_string())
            .unwrap_or_default(),
    );
    #[cfg(not(target_os = "linux"))]
    let discrete: Option<Vec<u32>> = None;

    let sample_rate = select_sample_rate(&ranges, discrete.as_deref(), requested_rate);

    let buffer_limits = options
        .iter()
        .find(|o| {
            o.channels == channels
                && o.sample_format == sample_format
                && sample_rate >= o.min_rate
                && sample_rate <= o.max_rate
        })
        .map(|o| o.buffer)
        .unwrap_or(BufferLimits::Unknown);
    let buffer_size = select_buffer_size(buffer_limits, requested_buffer_size);

    // Validate the chosen config against the device's real supported configs.
    let supported_here = supported.iter().any(|c| {
        c.channels() == channels
            && c.sample_format() == sample_format
            && sample_rate >= c.min_sample_rate()
            && sample_rate <= c.max_sample_rate()
    });
    if !supported_here {
        return Err(format!(
            "selected output config ({} ch, {:?}, {} Hz) is not supported by the device",
            channels, sample_format, sample_rate
        )
        .into());
    }

    Ok(OutputConfig {
        stream_config: cpal::StreamConfig {
            channels,
            sample_rate,
            buffer_size,
        },
        sample_format,
    })
}

/// Run one audio callback's worth of work into the f32 bus: drain the pending
/// control commands, mix, then reap finished samples into the graveyard for an
/// off-RT drop. Shared by every typed output stream so the device sample format
/// never changes the mix logic. The control thread never locks this state —
/// mutations arrive over the command ring and voice/ducking reconciliation is
/// done control-side — so this lock is uncontended (D22a).
fn run_mix_callback(bus: &mut [f32], callback_state: &Arc<Mutex<AudioCallbackState>>) {
    let mut guard = callback_state.lock();
    let acs = &mut *guard;
    // Destructure for disjoint &mut borrows of the bundled fields.
    let AudioCallbackState {
        mixer,
        commands,
        command_returns,
        graveyard,
        streamed_graveyard,
        output_sample_rate,
    } = acs;

    crate::rt_engine::drain_commands(
        commands,
        mixer,
        command_returns,
        graveyard,
        *output_sample_rate,
        64,
    );
    crate::audio::mixer::mix_audio(bus, mixer);
    crate::rt_engine::reap_finished(mixer, graveyard);
    crate::rt_engine::reap_finished_streamed(mixer, streamed_graveyard);
}

/// Build an output stream of element type `T`, mixing into an f32 scratch bus and
/// converting each sample to `T`. The scratch bus is reused across callbacks.
fn build_typed_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    callback_state: Arc<Mutex<AudioCallbackState>>,
    xruns: Arc<AtomicU64>,
    error_flag: Arc<AtomicBool>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32> + Send + 'static,
{
    let mut scratch: Vec<f32> = Vec::new();
    device.build_output_stream(
        *config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            scratch.resize(data.len(), 0.0);
            run_mix_callback(&mut scratch, &callback_state);
            for (out, &s) in data.iter_mut().zip(scratch.iter()) {
                *out = T::from_sample(s);
            }
        },
        move |err| {
            tracing::error!("Audio stream error: {}", err);
            xruns.fetch_add(1, Ordering::Relaxed);
            // Signal the supervisor to rebuild; the cpal callback must not do it.
            if super::rebuild::requires_rebuild(err.kind()) {
                error_flag.store(true, Ordering::Relaxed);
            }
        },
        None,
    )
}

/// Dispatch over the device's native sample format and build the matching typed
/// output stream. The mixer always works in f32; only the device-facing
/// conversion differs, so non-f32 devices no longer crash at build time.
pub fn build_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    callback_state: Arc<Mutex<AudioCallbackState>>,
    xruns: Arc<AtomicU64>,
    error_flag: Arc<AtomicBool>,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    use cpal::SampleFormat;
    let stream = match sample_format {
        SampleFormat::F32 => {
            build_typed_output_stream::<f32>(device, config, callback_state, xruns, error_flag)?
        }
        SampleFormat::I16 => {
            build_typed_output_stream::<i16>(device, config, callback_state, xruns, error_flag)?
        }
        SampleFormat::U16 => {
            build_typed_output_stream::<u16>(device, config, callback_state, xruns, error_flag)?
        }
        SampleFormat::I32 => {
            build_typed_output_stream::<i32>(device, config, callback_state, xruns, error_flag)?
        }
        other => return Err(format!("unsupported device sample format: {:?}", other).into()),
    };
    Ok(stream)
}

/// Own and supervise the output stream on a dedicated thread. On a fatal stream
/// error the supervisor rebuilds it — re-resolving the device by name and reusing
/// the original negotiated config (so the mixer's channel count and resample
/// target stay valid) — with exponential backoff. If the device cannot be
/// rebuilt after the backoff is exhausted, the process exits so a service manager
/// (e.g. systemd) can restart with a fresh full setup. Returns when `shutdown`
/// is set.
pub fn spawn_output_supervisor(
    device_name: Option<String>,
    config: cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    callback_state: Arc<Mutex<AudioCallbackState>>,
    xruns: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    use crate::audio::rebuild::RebuildPolicy;
    use std::time::{Duration, Instant};

    std::thread::spawn(move || {
        let mut policy = RebuildPolicy::new();
        let mut ever_succeeded = false;
        loop {
            if shutdown.load(Ordering::Relaxed) {
                return;
            }

            let error_flag = Arc::new(AtomicBool::new(false));
            let built = (|| -> Result<cpal::Stream, Box<dyn std::error::Error>> {
                let device = find_output_device(device_name.as_deref())?;
                let stream = build_output_stream(
                    &device,
                    &config,
                    sample_format,
                    callback_state.clone(),
                    xruns.clone(),
                    error_flag.clone(),
                )?;
                stream.play()?;
                Ok(stream)
            })();

            match built {
                Ok(stream) => {
                    tracing::info!("Audio stream started ({:?})", sample_format);
                    ever_succeeded = true;
                    let started = Instant::now();
                    // Hold the stream alive until a fatal error or shutdown.
                    while !error_flag.load(Ordering::Relaxed) && !shutdown.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    drop(stream);
                    if shutdown.load(Ordering::Relaxed) {
                        return;
                    }
                    // A stable run that then errors is a transient blip: reset the
                    // backoff and rebuild immediately. Rapid failures fall through
                    // to the growing backoff below.
                    if started.elapsed() >= Duration::from_secs(5) {
                        policy.reset();
                        tracing::warn!(
                            "Audio stream error after a stable run; rebuilding output..."
                        );
                        continue;
                    }
                    tracing::warn!("Audio stream failed shortly after start; backing off...");
                }
                Err(e) => {
                    tracing::error!("Audio output unavailable: {}", e);
                    if !ever_succeeded {
                        // A startup misconfiguration won't self-heal; fail fast.
                        #[cfg(target_os = "linux")]
                        tracing::error!(
                            "On ALSA, a raw 'hw:' device may require its native format; try a \
                             'plughw:' or 'default' device, which converts formats automatically."
                        );
                        std::process::exit(1);
                    }
                }
            }

            match policy.next_delay() {
                Some(delay) => {
                    tracing::info!("Retrying output device in {:?}", delay);
                    std::thread::sleep(delay);
                }
                None => {
                    tracing::error!(
                        "Output device could not be (re)built after several attempts; \
                         exiting for a service-manager restart."
                    );
                    std::process::exit(1);
                }
            }
        }
    })
}
