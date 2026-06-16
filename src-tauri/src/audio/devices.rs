use cpal::traits::{DeviceTrait, HostTrait};

/// A discoverable audio output device, identified by its display name.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    pub name: String,
}

/// Lists the names of all available output devices on the default host.
pub fn list_output_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    match host.output_devices() {
        Ok(devices) => devices
            .filter_map(|d| d.name().ok().map(|name| DeviceInfo { name }))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Case-insensitive substring match of any hint against the device name.
/// Returns the first device whose name contains any of the hints.
pub fn find_virtual_output<'a>(
    devices: &'a [DeviceInfo],
    hints: &[&str],
) -> Option<&'a DeviceInfo> {
    devices.iter().find(|d| {
        let lower = d.name.to_lowercase();
        hints.iter().any(|h| lower.contains(&h.to_lowercase()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str) -> DeviceInfo {
        DeviceInfo {
            name: name.to_string(),
        }
    }

    #[test]
    fn finds_blackhole_on_macos() {
        let devices = vec![dev("MacBook Pro Speakers"), dev("BlackHole 2ch")];
        let found = find_virtual_output(&devices, &["blackhole", "vb-audio", "cable"]);
        assert_eq!(found, Some(&dev("BlackHole 2ch")));
    }

    #[test]
    fn finds_vbcable_on_windows() {
        let devices = vec![
            dev("Speakers (Realtek)"),
            dev("CABLE Input (VB-Audio Virtual Cable)"),
        ];
        let found = find_virtual_output(&devices, &["blackhole", "vb-audio", "cable"]);
        assert_eq!(found, Some(&dev("CABLE Input (VB-Audio Virtual Cable)")));
    }

    #[test]
    fn returns_none_when_no_virtual_device() {
        let devices = vec![dev("Speakers (Realtek)")];
        let found = find_virtual_output(&devices, &["blackhole", "vb-audio", "cable"]);
        assert_eq!(found, None);
    }
}
