//! DNS name helpers shared by the daemon backends that report names as
//! text (mDNSResponder, Windows DNS).

use crate::model::record::DnsName;

#[cfg_attr(windows, allow(dead_code))]
const DEFAULT_DOMAIN: &str = "local";

/// `<instance>.<regtype>.<domain>`, with `name` as one label even if it
/// contains dots.
#[cfg_attr(windows, allow(dead_code))]
pub fn instance_name(name: &str, regtype: &str, domain: &str) -> DnsName {
    let mut labels = vec![name.to_string()];
    labels.extend(DnsName::parse(regtype).labels().iter().cloned());
    labels.extend(domain_or_local(domain).labels().iter().cloned());
    DnsName::from_labels(labels)
}

#[cfg_attr(windows, allow(dead_code))]
pub fn domain_or_local(domain: &str) -> DnsName {
    let parsed = DnsName::parse(domain);
    if parsed.labels().is_empty() {
        DnsName::parse(DEFAULT_DOMAIN)
    } else {
        parsed
    }
}

/// Parses a name in DNS presentation format as the daemon returns it:
/// `\DDD` is a decimal byte and `\c` a literal character.
pub fn parse_presentation(text: &str) -> DnsName {
    DnsName::from_labels(decode(text, true))
}

/// Unescapes text in presentation format as one label: unescaped dots are
/// kept, for daemons that do not escape dots inside instance names.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn unescape_label(text: &str) -> String {
    decode(text, false).concat()
}

/// Splits presentation text into unescaped labels (one label unless
/// `split_dots`).
fn decode(text: &str, split_dots: bool) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut labels = Vec::new();
    let mut current = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                let digits = bytes.get(i + 1..i + 4).filter(|d| d.iter().all(u8::is_ascii_digit));
                let decimal = digits.and_then(|d| std::str::from_utf8(d).ok()?.parse::<u8>().ok());
                if let Some(byte) = decimal {
                    current.push(byte);
                    i += 4;
                } else {
                    if let Some(&c) = bytes.get(i + 1) {
                        current.push(c);
                    }
                    i += 2;
                }
            }
            b'.' if split_dots => {
                labels.push(String::from_utf8_lossy(&current).into_owned());
                current.clear();
                i += 1;
            }
            c => {
                current.push(c);
                i += 1;
            }
        }
    }
    labels.push(String::from_utf8_lossy(&current).into_owned());
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_names_keep_dots_in_the_instance_label() {
        let n = instance_name("Mr. Printer", "_ipp._tcp.", "local.");
        assert_eq!(n.labels(), ["Mr. Printer", "_ipp", "_tcp", "local"]);
        assert!(n.is_service_instance());
    }

    #[test]
    fn presentation_names_are_unescaped() {
        let n = parse_presentation("Martins\\032MacBook\\.Pro.local.");
        assert_eq!(n.labels(), ["Martins MacBook.Pro", "local"]);
        assert_eq!(parse_presentation("M\\195\\188ller.local.").first_label(), "Müller");
        assert_eq!(parse_presentation("BRN30055C1A4C01.local.").to_string(), "BRN30055C1A4C01.local");
    }

    #[test]
    fn labels_keep_unescaped_dots() {
        assert_eq!(unescape_label("Mr. Printer"), "Mr. Printer");
        assert_eq!(unescape_label("Mr\\. Printer\\032\\195\\188"), "Mr. Printer ü");
    }
}
