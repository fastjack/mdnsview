# Spec Delta

## MODIFIED Requirements

### Requirement: Refinement of daemon results
For each service instance or host reported by the OS daemon, the system SHALL send its own queries for that instance's SRV and TXT records and the host's A/AAAA records, so that wire-level metadata is captured for it. The same applies to instances discovered on the wire. An instance is refined once its SRV and TXT records have both been received on the wire, a host once an A or AAAA record has; each query asks only for what is missing.

#### Scenario: Instance known only from avahi
- **WHEN** avahi reports an instance that has not yet been seen on the wire
- **THEN** the system queries for its records, and the responses are captured with wire metadata

#### Scenario: SRV received without TXT
- **WHEN** a responder's answer contains the SRV record of an instance but leaves out its TXT record, for example because the answer was too large to carry all additional records
- **THEN** the system sends a query for that instance's TXT record only, and the TXT record received in response is captured with wire metadata

#### Scenario: Host with one address family
- **WHEN** a host's A record has been received on the wire and it has no AAAA record
- **THEN** the host counts as refined and no further address queries are sent for it

## ADDED Requirements

### Requirement: Refinement retries
Refinement queries that remain unanswered SHALL be retried a bounded number of times with increasing delays, without waiting for the instance or host to be reported again.

#### Scenario: Unanswered refinement is retried
- **WHEN** a refinement query for an instance's TXT record gets no answer
- **THEN** the system sends the query again after increasing delays, up to its retry limit, even if the instance is not reported again in the meantime

#### Scenario: Retries stop once answered
- **WHEN** the TXT record arrives on the wire after the first retry
- **THEN** no further refinement queries are sent for that instance
