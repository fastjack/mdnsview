//! Conversion between DNS messages on the wire and our record types.

use std::net::{Ipv4Addr, Ipv6Addr};

use anyhow::{Result, anyhow};
use simple_dns::rdata::{self, RData as DnsRData};
use simple_dns::{CLASS, Label, Name, Packet, PacketFlag, QCLASS, QTYPE, Question, ResourceRecord, TYPE};

use crate::model::record::{DnsName, RData, rtype};

/// A record decoded from a message, before source metadata is attached.
#[derive(Debug, Clone, PartialEq)]
pub struct WireRecord {
    pub name: DnsName,
    pub data: RData,
    pub ttl: u32,
    pub cache_flush: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub is_response: bool,
    pub questions: Vec<(DnsName, u16)>,
    /// Answer, authority and additional records, in that order.
    pub records: Vec<WireRecord>,
}

pub fn decode(bytes: &[u8]) -> Result<Decoded> {
    let packet = Packet::parse(bytes).map_err(|e| anyhow!("malformed mDNS message: {e}"))?;
    let questions = packet
        .questions
        .iter()
        .map(|q| (name(&q.qname), qtype_code(q.qtype)))
        .collect();
    let records = packet
        .answers
        .iter()
        .chain(&packet.name_servers)
        .chain(&packet.additional_records)
        .filter(|rr| rr.class == CLASS::IN)
        .map(|rr| WireRecord {
            name: name(&rr.name),
            data: rdata(&rr.rdata),
            ttl: rr.ttl,
            cache_flush: rr.cache_flush,
        })
        .collect();
    Ok(Decoded {
        is_response: packet.has_flags(PacketFlag::RESPONSE),
        questions,
        records,
    })
}

/// Builds a multicast (QM) query. Known answers are included so responders
/// can suppress answers we already have (RFC 6762 §7.1); only PTR known
/// answers are supported, which is what enumeration needs.
pub fn encode_query(questions: &[(DnsName, u16)], known_answers: &[WireRecord]) -> Result<Vec<u8>> {
    let owned_names: Vec<Name<'static>> = questions.iter().map(|(n, _)| to_name(n)).collect();
    let mut packet = Packet::new_query(0);
    for ((_, qtype), qname) in questions.iter().zip(&owned_names) {
        packet.questions.push(Question::new(
            qname.clone(),
            QTYPE::TYPE(TYPE::from(*qtype)),
            QCLASS::CLASS(CLASS::IN),
            false,
        ));
    }
    for known in known_answers {
        if let RData::Ptr(target) = &known.data {
            packet.answers.push(ResourceRecord::new(
                to_name(&known.name),
                CLASS::IN,
                known.ttl,
                DnsRData::PTR(rdata::PTR(to_name(target))),
            ));
        }
    }
    packet
        .build_bytes_vec_compressed()
        .map_err(|e| anyhow!("cannot encode query: {e}"))
}

fn qtype_code(q: QTYPE) -> u16 {
    match q {
        QTYPE::TYPE(t) => t.into(),
        QTYPE::ANY => rtype::ANY,
        QTYPE::IXFR => 251,
        QTYPE::AXFR => 252,
        QTYPE::MAILB => 253,
        QTYPE::MAILA => 254,
    }
}

fn name(n: &Name<'_>) -> DnsName {
    DnsName::from_labels(
        n.get_labels()
            .iter()
            .map(|l| String::from_utf8_lossy(l.as_ref()).into_owned()),
    )
}

fn to_name(n: &DnsName) -> Name<'static> {
    let labels: Vec<Label<'static>> = n
        .labels()
        .iter()
        .map(|l| Label::new_unchecked(l.as_bytes().to_vec()))
        .collect();
    Name::new_with_labels(&labels)
}

fn char_string(s: &simple_dns::CharacterString<'_>) -> String {
    String::try_from(s.clone()).unwrap_or_else(|_| s.to_string())
}

fn rdata(d: &DnsRData<'_>) -> RData {
    match d {
        DnsRData::A(a) => RData::A(Ipv4Addr::from(a.address)),
        DnsRData::AAAA(a) => RData::Aaaa(Ipv6Addr::from(a.address)),
        DnsRData::PTR(p) => RData::Ptr(name(&p.0)),
        DnsRData::SRV(s) => RData::Srv {
            priority: s.priority,
            weight: s.weight,
            port: s.port,
            target: name(&s.target),
        },
        DnsRData::TXT(t) => RData::Txt(
            t.iter_raw()
                .map(|(key, value)| {
                    let mut s = key.to_vec();
                    if let Some(v) = value {
                        s.push(b'=');
                        s.extend_from_slice(v);
                    }
                    s
                })
                .collect(),
        ),
        DnsRData::HINFO(h) => RData::Hinfo {
            cpu: char_string(&h.cpu),
            os: char_string(&h.os),
        },
        DnsRData::NSEC(n) => RData::Nsec {
            next: name(&n.next_name),
            types: n
                .type_bit_maps
                .iter()
                .flat_map(|m| {
                    m.bitmap.iter().enumerate().flat_map(move |(byte, bits)| {
                        (0..8u16)
                            .filter(move |bit| bits & (0x80 >> bit) != 0)
                            .map(move |bit| u16::from(m.window_block) * 256 + byte as u16 * 8 + bit)
                    })
                })
                .collect(),
        },
        DnsRData::NULL(code, data) => RData::Unknown {
            rtype: *code,
            data: data.get_data().to_vec(),
        },
        DnsRData::Empty(t) => RData::Unknown {
            rtype: (*t).into(),
            data: Vec::new(),
        },
        other => RData::Other {
            rtype: other.type_code().into(),
            text: format!("{other:?}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Decoded {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        decode(&std::fs::read(path).unwrap()).unwrap()
    }

    fn find<'a>(d: &'a Decoded, owner: &str, rt: u16) -> &'a WireRecord {
        d.records
            .iter()
            .find(|r| r.name == DnsName::parse(owner) && r.data.rtype() == rt)
            .unwrap_or_else(|| panic!("no {} record for {owner}", rtype::name(rt)))
    }

    #[test]
    fn avahi_response() {
        let d = fixture("avahi_response.bin");
        assert!(d.is_response);
        let instance = "mdnsview fixture._mdnsview-test._tcp.local";
        let srv = find(&d, instance, rtype::SRV);
        assert_eq!(srv.ttl, 120);
        assert!(srv.cache_flush);
        assert!(matches!(&srv.data, RData::Srv { port: 4242, target, .. } if target.key() == "omarchy.local"));
        let ptr = find(&d, "_mdnsview-test._tcp.local", rtype::PTR);
        assert!(!ptr.cache_flush, "shared records have no cache-flush bit");
        assert_eq!(ptr.data, RData::Ptr(DnsName::parse(instance)));
        let txt = find(&d, instance, rtype::TXT);
        let RData::Txt(strings) = &txt.data else { panic!() };
        assert!(strings.contains(&b"k=v".to_vec()));
        find(&d, "omarchy.local", rtype::A);
        find(&d, "omarchy.local", rtype::AAAA);
    }

    #[test]
    fn apple_response_with_device_info_and_nsec() {
        let d = fixture("apple_device_info.bin");
        let info = find(&d, "Hyperion._device-info._tcp.local", rtype::TXT);
        let RData::Txt(strings) = &info.data else { panic!() };
        assert!(strings.iter().any(|s| s.starts_with(b"model=")));
        let nsec = find(&d, "Hyperion.local", rtype::NSEC);
        let RData::Nsec { types, .. } = &nsec.data else { panic!() };
        assert!(types.contains(&rtype::A) && types.contains(&rtype::AAAA), "{types:?}");
        find(&d, "Hyperion._companion-link._tcp.local", rtype::SRV);
    }

    #[test]
    fn goodbye_has_zero_ttl() {
        let d = fixture("avahi_goodbye.bin");
        let srv = find(&d, "mdnsview fixture._mdnsview-test._tcp.local", rtype::SRV);
        assert_eq!(srv.ttl, 0);
        assert_eq!(find(&d, "_mdnsview-test._tcp.local", rtype::PTR).ttl, 0);
    }

    #[test]
    fn null_record_kept_raw() {
        let d = fixture("sleep_proxy_null.bin");
        let r = &d.records[0];
        assert!(matches!(&r.data, RData::Unknown { rtype: 10, data } if !data.is_empty()));
    }

    #[test]
    fn opt_pseudo_record_is_not_a_record() {
        let d = fixture("airplay_with_opt.bin");
        assert!(d.records.iter().all(|r| r.data.rtype() != 41));
        find(&d, "Besprechungsraum._airplay._tcp.local", rtype::SRV);
    }

    #[test]
    fn unknown_type_kept_with_raw_bytes() {
        // Response with one answer: "x.local" TYPE65280 IN, ttl 120, rdata de ad.
        let mut msg = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0];
        msg.extend_from_slice(b"\x01x\x05local\x00");
        msg.extend_from_slice(&[0xff, 0x00, 0x00, 0x01, 0, 0, 0, 120, 0, 2, 0xde, 0xad]);
        let d = decode(&msg).unwrap();
        assert_eq!(
            d.records[0].data,
            RData::Unknown {
                rtype: 65280,
                data: vec![0xde, 0xad]
            }
        );
    }

    #[test]
    fn query_roundtrip() {
        let questions = vec![
            (DnsName::parse("_services._dns-sd._udp.local"), rtype::PTR),
            (DnsName::from_labels(["Mr. P", "_ipp", "_tcp", "local"]), rtype::SRV),
        ];
        let known = vec![WireRecord {
            name: DnsName::parse("_services._dns-sd._udp.local"),
            data: RData::Ptr(DnsName::parse("_ipp._tcp.local")),
            ttl: 4000,
            cache_flush: false,
        }];
        let bytes = encode_query(&questions, &known).unwrap();
        let d = decode(&bytes).unwrap();
        assert!(!d.is_response);
        assert_eq!(d.questions, questions);
        assert_eq!(d.records, known);
        // QM: the unicast-response bit (top bit of qclass) is clear.
        let parsed = Packet::parse(&bytes).unwrap();
        assert!(parsed.questions.iter().all(|q| !q.unicast_response));
    }
}
