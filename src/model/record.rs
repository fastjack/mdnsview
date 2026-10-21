//! Source-independent representation of mDNS resource records.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::SystemTime;

use tokio::time::Instant;

/// Resource record type codes used by mDNS / DNS-SD.
pub mod rtype {
    pub const A: u16 = 1;
    pub const PTR: u16 = 12;
    pub const HINFO: u16 = 13;
    pub const TXT: u16 = 16;
    pub const AAAA: u16 = 28;
    pub const SRV: u16 = 33;
    pub const NSEC: u16 = 47;
    pub const ANY: u16 = 255;

    /// Mnemonic for a type code, or `TYPE<n>` (RFC 3597) for unknown codes.
    pub fn name(code: u16) -> String {
        match code {
            A => "A".into(),
            2 => "NS".into(),
            5 => "CNAME".into(),
            PTR => "PTR".into(),
            HINFO => "HINFO".into(),
            TXT => "TXT".into(),
            AAAA => "AAAA".into(),
            SRV => "SRV".into(),
            41 => "OPT".into(),
            NSEC => "NSEC".into(),
            64 => "SVCB".into(),
            65 => "HTTPS".into(),
            ANY => "ANY".into(),
            n => format!("TYPE{n}"),
        }
    }
}

/// A DNS name kept as individual labels, so instance names containing dots
/// ("Mr. Printer") survive. Comparison is ASCII case-insensitive.
#[derive(Clone)]
pub struct DnsName {
    labels: Vec<String>,
    key: String,
}

impl DnsName {
    pub fn from_labels<I, S>(labels: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let labels: Vec<String> = labels
            .into_iter()
            .map(Into::into)
            .filter(|l| !l.is_empty())
            .collect();
        let key = join_escaped(&labels).to_ascii_lowercase();
        Self { labels, key }
    }

    /// Parses a dotted name; `\.` and `\\` escape a literal dot or backslash.
    pub fn parse(text: &str) -> Self {
        let mut labels = Vec::new();
        let mut current = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                '.' => labels.push(std::mem::take(&mut current)),
                c => current.push(c),
            }
        }
        labels.push(current);
        Self::from_labels(labels)
    }

    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    /// Lowercased, escaped form used as identity.
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn first_label(&self) -> &str {
        self.labels.first().map(String::as_str).unwrap_or("")
    }

    /// The name with the first label removed.
    pub fn parent(&self) -> DnsName {
        Self::from_labels(self.labels.iter().skip(1).cloned())
    }

    fn label_is(&self, index: usize, value: &str) -> bool {
        self.labels
            .get(index)
            .is_some_and(|l| l.eq_ignore_ascii_case(value))
    }

    fn is_proto_label(&self, index: usize) -> bool {
        self.label_is(index, "_tcp") || self.label_is(index, "_udp")
    }

    /// `_svc._tcp.<domain>`: a service type name (owner of browse PTRs).
    pub fn is_service_type(&self) -> bool {
        self.labels.len() >= 3
            && self.labels[0].starts_with('_')
            && self.is_proto_label(1)
            && !self.is_meta_query()
    }

    /// `_services._dns-sd._udp.<domain>`.
    pub fn is_meta_query(&self) -> bool {
        self.label_is(0, "_services") && self.label_is(1, "_dns-sd") && self.label_is(2, "_udp")
    }

    /// `<Instance>._svc._tcp.<domain>`: a service instance name.
    pub fn is_service_instance(&self) -> bool {
        self.labels.len() >= 4
            && self.labels[1].starts_with('_')
            && self.is_proto_label(2)
            && !self.label_is(1, "_sub")
            && !self.is_meta_query()
    }

    /// For an instance name, its type as `_svc._tcp`.
    pub fn instance_type(&self) -> Option<String> {
        self.is_service_instance()
            .then(|| format!("{}.{}", self.labels[1], self.labels[2]).to_ascii_lowercase())
    }
}

fn join_escaped(labels: &[String]) -> String {
    let mut out = String::new();
    for (i, label) in labels.iter().enumerate() {
        if i > 0 {
            out.push('.');
        }
        for c in label.chars() {
            if c == '.' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out
}

impl fmt::Display for DnsName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&join_escaped(&self.labels))
    }
}

impl fmt::Debug for DnsName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DnsName({self})")
    }
}

impl PartialEq for DnsName {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for DnsName {}

impl Hash for DnsName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

impl PartialOrd for DnsName {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DnsName {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

/// Decoded record data.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RData {
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    Ptr(DnsName),
    Srv {
        priority: u16,
        weight: u16,
        port: u16,
        target: DnsName,
    },
    /// TXT character-strings, uninterpreted bytes.
    Txt(Vec<Vec<u8>>),
    Hinfo {
        cpu: String,
        os: String,
    },
    Nsec {
        next: DnsName,
        types: Vec<u16>,
    },
    /// A type we do not decode, with its raw bytes.
    Unknown { rtype: u16, data: Vec<u8> },
    /// A type the parser decoded but we do not model, as display text.
    Other { rtype: u16, text: String },
}

impl RData {
    pub fn rtype(&self) -> u16 {
        match self {
            RData::A(_) => rtype::A,
            RData::Aaaa(_) => rtype::AAAA,
            RData::Ptr(_) => rtype::PTR,
            RData::Srv { .. } => rtype::SRV,
            RData::Txt(_) => rtype::TXT,
            RData::Hinfo { .. } => rtype::HINFO,
            RData::Nsec { .. } => rtype::NSEC,
            RData::Unknown { rtype, .. } | RData::Other { rtype, .. } => *rtype,
        }
    }

    pub fn address(&self) -> Option<IpAddr> {
        match self {
            RData::A(a) => Some(IpAddr::V4(*a)),
            RData::Aaaa(a) => Some(IpAddr::V6(*a)),
            _ => None,
        }
    }
}

/// Identity of a record: owner name, type and data. Several records may
/// share a name and type (e.g. multiple PTR or A records).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RecordKey {
    pub name: DnsName,
    pub data: RData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// The operating system's mDNS daemon.
    System,
    /// Our own multicast socket.
    Wire,
}

/// Which sources have reported a record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sources {
    pub system: bool,
    pub wire: bool,
}

impl Sources {
    pub fn add(&mut self, source: Source) {
        match source {
            Source::System => self.system = true,
            Source::Wire => self.wire = true,
        }
    }

    pub fn label(&self) -> &'static str {
        match (self.system, self.wire) {
            (true, true) => "both",
            (true, false) => "system",
            (false, true) => "wire",
            (false, false) => "-",
        }
    }
}

/// One record as reported by a source.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedRecord {
    pub name: DnsName,
    pub data: RData,
    /// TTL in seconds from the wire; `None` for daemon-held records, which
    /// stay valid until the daemon reports them removed.
    pub ttl: Option<u32>,
    pub cache_flush: bool,
    pub source: Source,
    pub src_addr: Option<IpAddr>,
    pub iface: Option<String>,
    pub at: Instant,
    pub wall: SystemTime,
}

impl ObservedRecord {
    pub fn key(&self) -> RecordKey {
        RecordKey {
            name: self.name.clone(),
            data: self.data.clone(),
        }
    }
}

/// Reason the wire source reports when macOS's Local Network privacy
/// setting blocks its multicast traffic.
pub const LOCAL_NETWORK_BLOCKED: &str = "blocked by macOS Local Network permission: allow the app \
     that starts mdnsview in System Settings › Privacy & Security › Local Network";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceState {
    Starting,
    Active,
    Unavailable(String),
}

/// Which of the eligible interfaces the user wants discovery to use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum InterfaceSelection {
    /// Every eligible interface, including ones that appear later.
    #[default]
    All,
    /// Only the named interfaces, whether or not they currently exist.
    Only(BTreeSet<String>),
}

impl InterfaceSelection {
    pub fn includes(&self, name: &str) -> bool {
        match self {
            InterfaceSelection::All => true,
            InterfaceSelection::Only(names) => names.contains(name),
        }
    }
}

/// An interface as shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceInfo {
    pub name: String,
    pub addresses: Vec<IpAddr>,
}

/// Messages from discovery sources to the store.
#[derive(Debug, Clone, PartialEq)]
pub enum Observation {
    Record(ObservedRecord),
    /// The OS daemon no longer holds this service instance.
    SystemRemoved { instance: DnsName, at: Instant },
    /// The daemon still holds this instance, but only on interfaces no
    /// longer in use: stop holding it, without a goodbye.
    SystemReleased { instance: DnsName, at: Instant },
    SourceStatus { source: Source, state: SourceState },
    /// The eligible interfaces, the names of those in use, and the
    /// runtime selection that chose them.
    Interfaces {
        eligible: Vec<InterfaceInfo>,
        active: Vec<String>,
        selection: InterfaceSelection,
    },
    /// Whether the wire source hears nothing from other hosts although its
    /// queries go out (Windows only; see the wire backend).
    WireSilent(bool),
}

/// A request from the store for the wire backend to query a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRequest {
    pub name: DnsName,
    pub rtypes: Vec<u16>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compare_case_insensitively() {
        let a = DnsName::parse("Brother-A1B2.local");
        let b = DnsName::parse("brother-a1b2.LOCAL");
        assert_eq!(a, b);
        assert_eq!(a.key(), "brother-a1b2.local");
        assert_eq!(a.to_string(), "Brother-A1B2.local");
    }

    #[test]
    fn dots_inside_labels_are_escaped() {
        let n = DnsName::from_labels(["Mr. Printer", "_ipp", "_tcp", "local"]);
        assert_eq!(n.to_string(), "Mr\\. Printer._ipp._tcp.local");
        assert_eq!(DnsName::parse(&n.to_string()), n);
        assert_eq!(n.first_label(), "Mr. Printer");
    }

    #[test]
    fn classifies_service_names() {
        let inst = DnsName::parse("Office Printer._ipp._tcp.local");
        assert!(inst.is_service_instance());
        assert_eq!(inst.instance_type().as_deref(), Some("_ipp._tcp"));
        assert_eq!(inst.parent().key(), "_ipp._tcp.local");

        let ty = DnsName::parse("_ipp._tcp.local");
        assert!(ty.is_service_type());
        assert!(!ty.is_service_instance());

        let meta = DnsName::parse("_services._dns-sd._udp.local");
        assert!(meta.is_meta_query());
        assert!(!meta.is_service_type());
        assert!(!meta.is_service_instance());

        let sub = DnsName::parse("_printer._sub._http._tcp.local");
        assert!(!sub.is_service_instance());

        assert!(!DnsName::parse("host.local").is_service_instance());
    }

    #[test]
    fn rtype_names() {
        assert_eq!(rtype::name(rtype::SRV), "SRV");
        assert_eq!(rtype::name(65280), "TYPE65280");
    }
}
