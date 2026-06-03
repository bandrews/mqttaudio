// ABOUTME: Linux-specific ALSA device probing for native hardware capabilities.
// ABOUTME: Provides detailed device information beyond what cpal exposes.

use alsa::pcm::{Format, HwParams, PCM};
use alsa::{Card, Direction};
use std::collections::HashSet;

use super::device::{
    DeviceCategory, DeviceInfo, DeviceList, NativeCapabilities, NativeSampleFormat,
};

/// Known ALSA device name patterns and their categories
fn classify_device_name(name: &str) -> DeviceCategory {
    if name.starts_with("hw:") {
        DeviceCategory::Hardware
    } else if name.starts_with("plughw:") {
        DeviceCategory::PluginHardware
    } else if name.starts_with("dmix:") {
        DeviceCategory::SoftwareMixer
    } else if name.starts_with("hdmi:") || name.contains("HDMI") {
        DeviceCategory::Hdmi
    } else if name.starts_with("front:")
        || name.starts_with("surround")
        || name.starts_with("iec958")
    {
        DeviceCategory::ChannelLayout
    } else if name.starts_with("sysdefault:")
        || name == "default"
        || name == "pulse"
        || name == "jack"
        || name == "oss"
    {
        DeviceCategory::System
    } else if name == "null" {
        DeviceCategory::Unavailable
    } else if is_plugin_device(name) {
        // Rate converters, upmixers, etc.
        DeviceCategory::System
    } else {
        // Likely a custom device from asound.conf
        DeviceCategory::Virtual
    }
}

/// Check if device name matches common ALSA plugin patterns
fn is_plugin_device(name: &str) -> bool {
    let plugins = [
        "lavrate",
        "samplerate",
        "speexrate",
        "speex",
        "upmix",
        "vdownmix",
    ];
    plugins.contains(&name)
}

/// Check if a device is likely to be useful for mqttaudio
fn should_suggest_device(name: &str, category: &DeviceCategory) -> bool {
    match category {
        // plughw devices are ideal - hardware with format flexibility
        DeviceCategory::PluginHardware => true,
        // Custom virtual devices are user-configured, likely intentional
        DeviceCategory::Virtual => {
            // But filter out common plugin names
            !is_plugin_device(name)
        }
        // sysdefault for specific cards is often good
        DeviceCategory::System if name.starts_with("sysdefault:CARD=") => true,
        _ => false,
    }
}

/// Convert ALSA format to our NativeSampleFormat
fn alsa_format_to_native(fmt: Format) -> Option<NativeSampleFormat> {
    match fmt {
        Format::S16LE => Some(NativeSampleFormat::S16LE),
        Format::S24LE => Some(NativeSampleFormat::S24LE),
        Format::S243LE => Some(NativeSampleFormat::S24_3LE),
        Format::S32LE => Some(NativeSampleFormat::S32LE),
        Format::FloatLE => Some(NativeSampleFormat::F32LE),
        Format::Float64LE => Some(NativeSampleFormat::F64LE),
        _ => None,
    }
}

/// Probe native hardware capabilities for an ALSA device
fn probe_device_capabilities(name: &str) -> Result<NativeCapabilities, String> {
    let pcm =
        PCM::new(name, Direction::Playback, false).map_err(|e| format!("Failed to open: {}", e))?;

    let hwp = HwParams::any(&pcm).map_err(|e| format!("Failed to get hw params: {}", e))?;

    // Probe supported formats
    let formats_to_test = [
        Format::S16LE,
        Format::S24LE,
        Format::S243LE,
        Format::S32LE,
        Format::FloatLE,
        Format::Float64LE,
    ];

    let mut formats = Vec::new();
    for fmt in formats_to_test.iter() {
        if hwp.test_format(*fmt).is_ok() {
            if let Some(native_fmt) = alsa_format_to_native(*fmt) {
                formats.push(native_fmt);
            }
        }
    }

    // Get channel range (cap at reasonable values for display)
    let min_channels = hwp
        .get_channels_min()
        .map_err(|e| format!("Failed to get min channels: {}", e))? as u16;
    let max_channels_raw =
        hwp.get_channels_max()
            .map_err(|e| format!("Failed to get max channels: {}", e))? as u16;
    // Plugin devices may report absurd values (10000+), cap for sanity
    let max_channels = max_channels_raw.min(128);

    // Get sample rate range (cap at reasonable values)
    let min_rate = hwp
        .get_rate_min()
        .map_err(|e| format!("Failed to get min rate: {}", e))?;
    let max_rate_raw = hwp
        .get_rate_max()
        .map_err(|e| format!("Failed to get max rate: {}", e))?;
    // Plugin devices may report absurd rates, cap at 384kHz
    let max_rate = max_rate_raw.min(384000);

    // Check if device supports continuous rates or only discrete ones
    let discrete_rates = if min_rate != max_rate && hwp.test_rate(min_rate + 1).is_err() {
        // Device only supports specific rates, probe common ones
        let common_rates = [44100, 48000, 88200, 96000, 176400, 192000];
        let mut supported: Vec<u32> = common_rates
            .iter()
            .filter(|&&r| hwp.test_rate(r).is_ok())
            .copied()
            .collect();
        supported.sort();
        if supported.is_empty() {
            None
        } else {
            Some(supported)
        }
    } else {
        None
    };

    Ok(NativeCapabilities {
        formats,
        min_channels,
        max_channels,
        min_sample_rate: min_rate,
        max_sample_rate: max_rate,
        discrete_rates,
    })
}

/// Get card ID (short name) from ALSA
#[allow(dead_code)]
fn get_card_id(card_index: i32) -> Option<String> {
    let card = Card::new(card_index);
    card.get_name().ok()
}

/// Check if a card index is valid by trying to get its name
fn card_exists(card_index: i32) -> bool {
    let card = Card::new(card_index);
    card.get_name().is_ok()
}

/// Extract card name from device string like "hw:CARD=UMC1820,DEV=0"
#[allow(dead_code)]
fn extract_card_name_from_device(name: &str) -> Option<String> {
    let prefix = if name.starts_with("hw:") {
        "hw:"
    } else if name.starts_with("plughw:") {
        "plughw:"
    } else {
        return None;
    };

    let rest = &name[prefix.len()..];

    // Try "CARD=name" format
    if let Some(card_part) = rest.strip_prefix("CARD=") {
        let card_name = if let Some(comma_pos) = card_part.find(',') {
            &card_part[..comma_pos]
        } else {
            card_part
        };
        return Some(card_name.to_string());
    }

    // Try numeric format "N,M"
    if let Some(comma_pos) = rest.find(',') {
        if let Ok(idx) = rest[..comma_pos].parse::<i32>() {
            return get_card_id(idx);
        }
    }

    None
}

/// Extract card index from ALSA device name like "hw:CARD=UMC1820,DEV=0" or "plughw:1,0"
fn extract_card_index_from_name(name: &str) -> Option<i32> {
    // Try to extract card index from CARD=name format
    if let Some(card_pos) = name.find("CARD=") {
        let rest = &name[card_pos + 5..];
        let card_name = if let Some(comma_pos) = rest.find(',') {
            &rest[..comma_pos]
        } else {
            rest
        };

        // Look up card name in /proc/asound/cards
        if let Ok(cards) = std::fs::read_to_string("/proc/asound/cards") {
            for line in cards.lines() {
                // Format: " 1 [UMC1820        ]: USB-Audio - UMC1820"
                if let Some(bracket_start) = line.find('[') {
                    if let Some(bracket_end) = line.find(']') {
                        let card_id = line[bracket_start + 1..bracket_end].trim();
                        if card_id == card_name {
                            let idx_str = line[..bracket_start].trim();
                            if let Ok(idx) = idx_str.parse::<i32>() {
                                return Some(idx);
                            }
                        }
                    }
                }
            }
        }
    }

    // Try numeric format (hw:1,0)
    let prefixes = ["hw:", "plughw:", "sysdefault:", "dmix:", "front:", "hdmi:"];
    for prefix in prefixes {
        if let Some(rest) = name.strip_prefix(prefix) {
            let num_part = if let Some(comma_pos) = rest.find(',') {
                &rest[..comma_pos]
            } else {
                rest
            };
            if let Ok(idx) = num_part.parse::<i32>() {
                return Some(idx);
            }
        }
    }

    None
}

/// Probe all ALSA devices and return categorized device list
pub fn probe_alsa_devices() -> DeviceList {
    let mut list = DeviceList::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut card_capabilities: std::collections::HashMap<i32, NativeCapabilities> =
        std::collections::HashMap::new();

    // First, probe hardware capabilities for each card using hw:N,0 format
    for card_idx in 0..32 {
        if !card_exists(card_idx) {
            continue;
        }
        let hw_name = format!("hw:{},0", card_idx);
        if let Ok(caps) = probe_device_capabilities(&hw_name) {
            card_capabilities.insert(card_idx, caps);
        }
    }

    // Enumerate all PCM devices from ALSA hints - these are the actual names cpal uses
    if let Ok(hints) = alsa::device_name::HintIter::new_str(None, "pcm") {
        for hint in hints {
            let name = match hint.name {
                Some(n) => n,
                None => continue,
            };

            if name == "null" || seen_names.contains(&name) {
                continue;
            }
            seen_names.insert(name.clone());

            let category = classify_device_name(&name);
            let mut device = DeviceInfo::new(name.clone(), category.clone());

            if let Some(desc) = hint.desc {
                let first_line = desc.lines().next().unwrap_or(&desc);
                device.description = Some(first_line.to_string());
            }

            // For hardware-related devices, use our pre-probed capabilities
            if let Some(card_idx) = extract_card_index_from_name(&name) {
                if let Some(caps) = card_capabilities.get(&card_idx) {
                    // For hw: devices, check format compatibility
                    if category == DeviceCategory::Hardware {
                        let cpal_compatible = caps.formats.iter().any(|f| {
                            matches!(
                                f,
                                NativeSampleFormat::S16LE
                                    | NativeSampleFormat::S32LE
                                    | NativeSampleFormat::F32LE
                                    | NativeSampleFormat::F64LE
                            )
                        });

                        if !cpal_compatible {
                            device.notes.push(format!(
                                "Requires {} format (use plughw: instead)",
                                caps.format_formats()
                            ));
                        }
                    }
                    device.native_capabilities = Some(caps.clone());
                }
            } else if !is_plugin_device(&name) && category != DeviceCategory::System {
                // For other devices, probe directly
                match probe_device_capabilities(&name) {
                    Ok(caps) => {
                        device.native_capabilities = Some(caps);
                    }
                    Err(e) => {
                        if e.contains("Device or resource busy") {
                            device = device.with_unavailable_reason("Device is busy");
                        } else if e.contains("Cannot connect to server")
                            || e.contains("not running")
                        {
                            device = device.with_unavailable_reason("Sound server not running");
                        }
                    }
                }
            }

            // Check for sound servers that aren't running
            if name == "jack" && PCM::new(&name, Direction::Playback, false).is_err() {
                device = device.with_unavailable_reason("JACK server not running");
            } else if name == "pulse" && PCM::new(&name, Direction::Playback, false).is_err() {
                device = device.with_unavailable_reason("PulseAudio not available");
            }

            // Mark as suggested if appropriate
            if should_suggest_device(&device.name, &device.category)
                && device.category != DeviceCategory::Unavailable
            {
                let mut suggested = device.clone();
                suggested.category = DeviceCategory::Suggested;

                if name.starts_with("plughw:") {
                    suggested.suggestion_reason =
                        Some("Hardware device with automatic format conversion".to_string());
                } else if name.starts_with("sysdefault:") {
                    suggested.suggestion_reason =
                        Some("System default for this sound card".to_string());
                } else {
                    suggested.suggestion_reason =
                        Some("Custom virtual device (from asound.conf)".to_string());
                }

                list.devices.push(suggested);
            }

            list.devices.push(device);
        }
    }

    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_device_name() {
        assert_eq!(
            classify_device_name("hw:CARD=UMC1820,DEV=0"),
            DeviceCategory::Hardware
        );
        assert_eq!(
            classify_device_name("plughw:CARD=UMC1820,DEV=0"),
            DeviceCategory::PluginHardware
        );
        assert_eq!(
            classify_device_name("dmix:CARD=PCH,DEV=0"),
            DeviceCategory::SoftwareMixer
        );
        assert_eq!(classify_device_name("default"), DeviceCategory::System);
        assert_eq!(classify_device_name("pulse"), DeviceCategory::System);
        assert_eq!(
            classify_device_name("surround71:CARD=PCH,DEV=0"),
            DeviceCategory::ChannelLayout
        );
        assert_eq!(
            classify_device_name("hdmi:CARD=PCH,DEV=0"),
            DeviceCategory::Hdmi
        );
        assert_eq!(classify_device_name("ch1"), DeviceCategory::Virtual);
        assert_eq!(classify_device_name("room"), DeviceCategory::Virtual);
        assert_eq!(classify_device_name("lavrate"), DeviceCategory::System);
    }

    #[test]
    fn test_should_suggest_device() {
        assert!(should_suggest_device(
            "plughw:CARD=UMC1820,DEV=0",
            &DeviceCategory::PluginHardware
        ));
        assert!(should_suggest_device("ch1", &DeviceCategory::Virtual));
        assert!(should_suggest_device("room", &DeviceCategory::Virtual));
        assert!(!should_suggest_device("lavrate", &DeviceCategory::Virtual));
        assert!(!should_suggest_device("default", &DeviceCategory::System));
        assert!(should_suggest_device(
            "sysdefault:CARD=UMC1820",
            &DeviceCategory::System
        ));
    }

    #[test]
    fn test_extract_card_name() {
        assert_eq!(
            extract_card_name_from_device("hw:CARD=UMC1820,DEV=0"),
            Some("UMC1820".to_string())
        );
        assert_eq!(
            extract_card_name_from_device("plughw:CARD=PCH,DEV=0"),
            Some("PCH".to_string())
        );
        assert_eq!(extract_card_name_from_device("default"), None);
    }
}
