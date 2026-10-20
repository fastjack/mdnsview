# Spec Delta

## ADDED Requirements

### Requirement: Cache-flush scope
A received record with the cache-flush bit SHALL expire, one second later, the other records of the same name and type that were received more than one second earlier (RFC 6762 section 10.2). For SRV and TXT records, it SHALL only expire records from the same host:
- an SRV record expires only SRV records with the same target
- a TXT record expires only TXT records received from the same source address

#### Scenario: Host changes its address
- **WHEN** `nas.local` announces a new A record with the cache-flush bit, and its old A record was received 10 seconds earlier
- **THEN** the old A record expires one second later

#### Scenario: Host changes its port
- **WHEN** `printer.local` announces a new SRV record for its instance with a new port and the cache-flush bit
- **THEN** the old SRV record with the same target expires one second later

#### Scenario: Two hosts announce the same instance name
- **WHEN** `DESKTOP-EAOAATI.local` and `DESKTOP-28SEPKJ-2.local` both announce SRV and TXT records for `0._teamviewer._tcp.local` with the cache-flush bit, a minute apart
- **THEN** neither announcement expires the other host's SRV or TXT record
