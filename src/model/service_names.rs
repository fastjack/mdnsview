//! Human-readable names for common DNS-SD service types.

/// `(type, readable name)`, types lowercased as `_svc._proto`.
const NAMES: &[(&str, &str)] = &[
    ("_adisk._tcp", "Time Machine disk"),
    ("_afpovertcp._tcp", "AFP file sharing"),
    ("_airplay._tcp", "AirPlay"),
    ("_airport._tcp", "AirPort base station"),
    ("_androidtvremote2._tcp", "Android TV remote"),
    ("_apple-mobdev2._tcp", "Apple device sync"),
    ("_companion-link._tcp", "Apple companion link"),
    ("_daap._tcp", "iTunes / DAAP music sharing"),
    ("_device-info._tcp", "Device info"),
    ("_dpap._tcp", "iPhoto sharing"),
    ("_esphomelib._tcp", "ESPHome"),
    ("_ftp._tcp", "FTP"),
    ("_googlecast._tcp", "Google Cast"),
    ("_hap._tcp", "HomeKit accessory"),
    ("_hap._udp", "HomeKit accessory (Thread)"),
    ("_home-assistant._tcp", "Home Assistant"),
    ("_homekit._tcp", "HomeKit"),
    ("_http._tcp", "Web server (HTTP)"),
    ("_https._tcp", "Web server (HTTPS)"),
    ("_ipp._tcp", "Printer (IPP)"),
    ("_ipps._tcp", "Printer (IPP over TLS)"),
    ("_matter._tcp", "Matter"),
    ("_matterc._udp", "Matter commissioning"),
    ("_meshcop._udp", "Thread border router"),
    ("_mqtt._tcp", "MQTT broker"),
    ("_nfs._tcp", "NFS file sharing"),
    ("_pdl-datastream._tcp", "Printer (raw socket)"),
    ("_printer._tcp", "Printer (LPD)"),
    ("_raop._tcp", "AirPlay audio (RAOP)"),
    ("_rdlink._tcp", "Apple remote desktop link"),
    ("_rfb._tcp", "Screen sharing (VNC)"),
    ("_scanner._tcp", "Scanner"),
    ("_sftp-ssh._tcp", "SFTP"),
    ("_sleep-proxy._udp", "Bonjour sleep proxy"),
    ("_smb._tcp", "SMB file sharing"),
    ("_spotify-connect._tcp", "Spotify Connect"),
    ("_ssh._tcp", "SSH"),
    ("_touch-able._tcp", "Apple TV remote"),
    ("_uscan._tcp", "Scanner (eSCL)"),
    ("_uscans._tcp", "Scanner (eSCL over TLS)"),
    ("_workstation._tcp", "Workstation"),
];

/// Readable name for a service type such as `_ipp._tcp`, if known.
pub fn readable(service_type: &str) -> Option<&'static str> {
    let lower = service_type.to_ascii_lowercase();
    NAMES
        .binary_search_by(|(t, _)| (*t).cmp(lower.as_str()))
        .ok()
        .map(|i| NAMES[i].1)
}

/// Readable name, falling back to the raw type string.
pub fn label(service_type: &str) -> String {
    readable(service_type)
        .map(str::to_string)
        .unwrap_or_else(|| service_type.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_for_binary_search() {
        assert!(NAMES.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn known_type() {
        assert_eq!(label("_ipp._tcp"), "Printer (IPP)");
        assert_eq!(label("_airplay._tcp"), "AirPlay");
        assert_eq!(label("_IPP._TCP"), "Printer (IPP)");
    }

    #[test]
    fn unknown_type_keeps_raw_string() {
        assert_eq!(label("_foo-bar._tcp"), "_foo-bar._tcp");
        assert_eq!(readable("_foo-bar._tcp"), None);
    }
}
