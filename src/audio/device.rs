// ABOUTME: Platform-agnostic audio device information and categorization.
// ABOUTME: Provides structured device discovery with native capability detection.

use std::fmt;

/// Category of audio device, used for filtering and display
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeviceCategory {
    /// Recommended devices that are likely to work well
    Suggested,
    /// Direct hardware devices (hw: on ALSA)
    Hardware,
    /// Hardware with plugin layer (plughw: on ALSA)
    PluginHardware,
    /// User-defined virtual devices (from asound.conf)
    Virtual,
    /// System defaults and sound server redirects
    System,
    /// Surround/channel layout presets
    ChannelLayout,
    /// HDMI audio outputs
    Hdmi,
    /// Software mixing devices (dmix on ALSA)
    SoftwareMixer,
    /// Devices that failed to probe or are unavailable
    Unavailable,
}

impl fmt::Display for DeviceCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceCategory::Suggested => write!(f, "Suggested Devices"),
            DeviceCategory::Hardware => write!(f, "Hardware Devices"),
            DeviceCategory::PluginHardware => write!(f, "Hardware (with format conversion)"),
            DeviceCategory::Virtual => write!(f, "Virtual Devices"),
            DeviceCategory::System => write!(f, "System Devices"),
            DeviceCategory::ChannelLayout => write!(f, "Channel Layout Presets"),
            DeviceCategory::Hdmi => write!(f, "HDMI Outputs"),
            DeviceCategory::SoftwareMixer => write!(f, "Software Mixer Devices"),
            DeviceCategory::Unavailable => write!(f, "Unavailable"),
        }
    }
}

/// Native sample format supported by hardware
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum NativeSampleFormat {
    S16LE,
    S24LE,      // 24-bit in 4-byte container
    S24_3LE,    // 24-bit packed (3 bytes)
    S32LE,
    F32LE,
    F64LE,
}

impl fmt::Display for NativeSampleFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NativeSampleFormat::S16LE => write!(f, "S16LE"),
            NativeSampleFormat::S24LE => write!(f, "S24LE"),
            NativeSampleFormat::S24_3LE => write!(f, "S24_3LE"),
            NativeSampleFormat::S32LE => write!(f, "S32LE"),
            NativeSampleFormat::F32LE => write!(f, "F32LE"),
            NativeSampleFormat::F64LE => write!(f, "F64LE"),
        }
    }
}

/// Native hardware capabilities (from direct ALSA probe on Linux)
#[derive(Debug, Clone)]
pub struct NativeCapabilities {
    pub formats: Vec<NativeSampleFormat>,
    pub min_channels: u16,
    pub max_channels: u16,
    pub min_sample_rate: u32,
    pub max_sample_rate: u32,
    /// Specific supported rates if device doesn't support continuous range
    pub discrete_rates: Option<Vec<u32>>,
}

impl NativeCapabilities {
    pub fn format_rates(&self) -> String {
        if let Some(ref rates) = self.discrete_rates {
            rates.iter()
                .map(|r| r.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        } else if self.min_sample_rate == self.max_sample_rate {
            format!("{}", self.min_sample_rate)
        } else {
            format!("{}-{}", self.min_sample_rate, self.max_sample_rate)
        }
    }

    pub fn format_channels(&self) -> String {
        if self.min_channels == self.max_channels {
            format!("{}", self.max_channels)
        } else {
            format!("{}-{}", self.min_channels, self.max_channels)
        }
    }

    pub fn format_formats(&self) -> String {
        self.formats.iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Information about a discovered audio device
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Device name/identifier (used for --device argument)
    pub name: String,
    /// Human-readable description
    pub description: Option<String>,
    /// Device category for filtering and display
    pub category: DeviceCategory,
    /// Native hardware capabilities (if probed successfully)
    pub native_capabilities: Option<NativeCapabilities>,
    /// Channel count from cpal (may differ from native)
    pub cpal_channels: Option<u16>,
    /// Sample rate range from cpal
    pub cpal_sample_rate_min: Option<u32>,
    pub cpal_sample_rate_max: Option<u32>,
    /// Why this device is suggested (if category is Suggested)
    pub suggestion_reason: Option<String>,
    /// Why this device is unavailable (if category is Unavailable)
    pub unavailable_reason: Option<String>,
    /// Additional notes for display
    pub notes: Vec<String>,
}

impl DeviceInfo {
    pub fn new(name: String, category: DeviceCategory) -> Self {
        Self {
            name,
            description: None,
            category,
            native_capabilities: None,
            cpal_channels: None,
            cpal_sample_rate_min: None,
            cpal_sample_rate_max: None,
            suggestion_reason: None,
            unavailable_reason: None,
            notes: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    #[allow(dead_code)]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    #[allow(dead_code)]
    pub fn with_unavailable_reason(mut self, reason: impl Into<String>) -> Self {
        self.unavailable_reason = Some(reason.into());
        self.category = DeviceCategory::Unavailable;
        self
    }

    #[allow(dead_code)]
    pub fn with_suggestion_reason(mut self, reason: impl Into<String>) -> Self {
        self.suggestion_reason = Some(reason.into());
        self
    }
}

/// Result of device discovery
#[derive(Debug)]
pub struct DeviceList {
    pub devices: Vec<DeviceInfo>,
    /// Warnings or errors encountered during discovery (for optional display)
    #[allow(dead_code)]
    pub discovery_notes: Vec<String>,
}

impl DeviceList {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            discovery_notes: Vec::new(),
        }
    }

    /// Get devices filtered by category
    pub fn by_category(&self, category: DeviceCategory) -> Vec<&DeviceInfo> {
        self.devices.iter()
            .filter(|d| d.category == category)
            .collect()
    }

    /// Get all categories that have at least one device
    #[allow(dead_code)]
    pub fn categories(&self) -> Vec<DeviceCategory> {
        let mut cats: Vec<_> = self.devices.iter()
            .map(|d| d.category.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        cats.sort();
        cats
    }

    /// Get suggested devices (convenience method)
    #[allow(dead_code)]
    pub fn suggested(&self) -> Vec<&DeviceInfo> {
        self.by_category(DeviceCategory::Suggested)
    }
}

impl Default for DeviceList {
    fn default() -> Self {
        Self::new()
    }
}

/// Display formatted device list
pub fn format_device_list(list: &DeviceList) -> String {
    let mut output = String::new();
    output.push_str("Available audio output devices:\n");

    // Display order for categories
    let display_order = [
        DeviceCategory::Suggested,
        DeviceCategory::PluginHardware,
        DeviceCategory::Hardware,
        DeviceCategory::Virtual,
        DeviceCategory::System,
        DeviceCategory::ChannelLayout,
        DeviceCategory::Hdmi,
        DeviceCategory::SoftwareMixer,
        DeviceCategory::Unavailable,
    ];

    for category in display_order.iter() {
        let devices = list.by_category(category.clone());
        if devices.is_empty() {
            continue;
        }

        output.push_str(&format!("\n=== {} ===\n", category));

        for device in devices {
            output.push_str(&format!("  {}\n", device.name));

            if let Some(ref desc) = device.description {
                output.push_str(&format!("    {}\n", desc));
            }

            if let Some(ref reason) = device.suggestion_reason {
                output.push_str(&format!("    Why: {}\n", reason));
            }

            if let Some(ref caps) = device.native_capabilities {
                output.push_str(&format!(
                    "    Native: {} ch, {} Hz, {}\n",
                    caps.format_channels(),
                    caps.format_rates(),
                    caps.format_formats()
                ));
            }

            // Show cpal capabilities if no native caps or if they differ
            if device.native_capabilities.is_none() {
                if let (Some(ch), Some(min_r), Some(max_r)) =
                    (device.cpal_channels, device.cpal_sample_rate_min, device.cpal_sample_rate_max)
                {
                    let rate_str = if min_r == max_r {
                        format!("{} Hz", min_r)
                    } else {
                        format!("{}-{} Hz", min_r, max_r)
                    };
                    output.push_str(&format!("    Reported: {} ch, {}\n", ch, rate_str));
                }
            }

            if let Some(ref reason) = device.unavailable_reason {
                output.push_str(&format!("    Reason: {}\n", reason));
            }

            for note in &device.notes {
                output.push_str(&format!("    Note: {}\n", note));
            }
        }
    }

    output
}

/// Try to match two ALSA device names
/// Returns Some(true) if they match, Some(false) if same prefix but different card, None if not comparable
#[cfg(target_os = "linux")]
pub fn try_match_alsa_device(requested: &str, enumerated: &str) -> Option<bool> {
    // Get prefix (hw:, plughw:, sysdefault:, etc.)
    let prefixes = ["plughw:", "hw:", "sysdefault:", "dmix:", "front:", "surround"];

    for prefix in prefixes {
        if requested.starts_with(prefix) && enumerated.starts_with(prefix) {
            // Both have the same prefix, compare by card number
            let req_card = extract_alsa_card_from_name(requested);
            let enum_card = extract_alsa_card_from_name(enumerated);

            match (req_card, enum_card) {
                (Some(r), Some(e)) => return Some(r == e),
                _ => continue,
            }
        }
    }

    None
}

/// Extract card identifier from ALSA device name
/// Handles both numeric (hw:1,0) and named (hw:CARD=UMC1820,DEV=0) formats
#[cfg(target_os = "linux")]
fn extract_alsa_card_from_name(name: &str) -> Option<AlsaCardId> {
    // Find prefix end
    let prefixes = ["plughw:", "hw:", "sysdefault:", "dmix:", "front:", "surround"];

    for prefix in prefixes {
        if name.starts_with(prefix) {
            let rest = &name[prefix.len()..];

            // Try "CARD=name" format first
            if rest.starts_with("CARD=") {
                let card_part = &rest[5..];
                let card_name = if let Some(comma_pos) = card_part.find(',') {
                    &card_part[..comma_pos]
                } else {
                    card_part
                };
                return Some(AlsaCardId::Name(card_name.to_string()));
            }

            // Try numeric format "N,M" or just "N"
            let num_part = if let Some(comma_pos) = rest.find(',') {
                &rest[..comma_pos]
            } else {
                rest
            };

            if let Ok(num) = num_part.parse::<u32>() {
                return Some(AlsaCardId::Index(num));
            }
        }
    }

    None
}

/// ALSA card identifier - can be either index or name
#[cfg(target_os = "linux")]
#[derive(Debug)]
enum AlsaCardId {
    Index(u32),
    Name(String),
}

#[cfg(target_os = "linux")]
impl PartialEq for AlsaCardId {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (AlsaCardId::Index(a), AlsaCardId::Index(b)) => a == b,
            (AlsaCardId::Name(a), AlsaCardId::Name(b)) => a == b,
            // Cross-compare by looking up card index from name
            (AlsaCardId::Index(idx), AlsaCardId::Name(name)) |
            (AlsaCardId::Name(name), AlsaCardId::Index(idx)) => {
                // Try to match card name to index by checking /proc/asound/cards
                if let Ok(cards) = std::fs::read_to_string("/proc/asound/cards") {
                    for line in cards.lines() {
                        // Format: " 1 [UMC1820        ]: USB-Audio - UMC1820"
                        if let Some(bracket_start) = line.find('[') {
                            if let Some(bracket_end) = line.find(']') {
                                let card_id = line[bracket_start + 1..bracket_end].trim();
                                if card_id == name {
                                    // Found the card name, extract index
                                    let idx_str = line[..bracket_start].trim();
                                    if let Ok(card_idx) = idx_str.parse::<u32>() {
                                        return card_idx == *idx;
                                    }
                                }
                            }
                        }
                    }
                }
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_info_creation() {
        let device = DeviceInfo::new("hw:0,0".to_string(), DeviceCategory::Hardware)
            .with_description("Test Device")
            .with_note("This is a test");

        assert_eq!(device.name, "hw:0,0");
        assert_eq!(device.description, Some("Test Device".to_string()));
        assert_eq!(device.notes.len(), 1);
    }

    #[test]
    fn test_device_list_filtering() {
        let mut list = DeviceList::new();
        list.devices.push(DeviceInfo::new("hw:0".to_string(), DeviceCategory::Hardware));
        list.devices.push(DeviceInfo::new("plughw:0".to_string(), DeviceCategory::PluginHardware));
        list.devices.push(DeviceInfo::new("default".to_string(), DeviceCategory::System));

        assert_eq!(list.by_category(DeviceCategory::Hardware).len(), 1);
        assert_eq!(list.by_category(DeviceCategory::PluginHardware).len(), 1);
        assert_eq!(list.by_category(DeviceCategory::Suggested).len(), 0);
    }

    #[test]
    fn test_native_capabilities_formatting() {
        let caps = NativeCapabilities {
            formats: vec![NativeSampleFormat::S24_3LE],
            min_channels: 20,
            max_channels: 20,
            min_sample_rate: 44100,
            max_sample_rate: 96000,
            discrete_rates: Some(vec![44100, 48000, 88200, 96000]),
        };

        assert_eq!(caps.format_channels(), "20");
        assert_eq!(caps.format_rates(), "44100, 48000, 88200, 96000");
        assert_eq!(caps.format_formats(), "S24_3LE");
    }
}
