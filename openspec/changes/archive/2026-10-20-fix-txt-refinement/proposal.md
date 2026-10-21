# Proposal

## Why

Some services never get a wire TXT record. Their TXT shows `via system` permanently, without source address, TTL or cache-flush bit. This is the Brother DCP-L8400CDN `_ipp` service on the office LAN.

Probing that printer showed the cause:
- **It answers a lone question in full.** A question about `_ipp._tcp` alone gets PTR, SRV, TXT and A.
- **It drops extra records from combined answers.** When one query asks about several of its service types, as mdnsview's enumeration does, it fills the answer to about 1.2 KB. It then leaves out the remaining extra records without setting the truncation bit, which mDNS allows. The `_ipp` SRV made it in, but the `_ipp` TXT did not.
- **mdnsview then stops asking.** The store counts an instance as refined once any wire SRV exists, so it never asks for the TXT.
- **Nothing retries.** A refinement request that goes unanswered, for example by a slow or sleeping device, is not repeated either.

## What Changes

- Refinement treats SRV and TXT separately. An instance keeps being refined until **both** its SRV and its TXT record have been received on the wire, and each request asks only for the types still missing.
- Unanswered refinement requests are retried a bounded number of times, with increasing delays. Retries no longer depend on another PTR arriving.
- Host refinement is unchanged: one wire A or AAAA record still satisfies it, since many hosts have only one address family.
- No user-visible configuration, UI or wire-format changes. The only visible effect is that affected TXT records turn from `via system` to `via both`, with wire metadata.

Out of scope: pacing or splitting the startup enumeration queries, and per-responder answer-size workarounds. Probing showed a burst of 160 questions does not by itself lose answers. The truncated additional records are handled by asking for what is missing.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `mdns-discovery`: the *Refinement of daemon results* requirement now covers SRV and TXT separately, including instances whose SRV arrived without its TXT. A new *Refinement retries* requirement adds bounded retries. These were split at archive time to keep each requirement under the 500-character limit.

## Impact

- **Code:**
  - `src/model/store.rs`: refinement bookkeeping (per-type satisfaction, retry schedule), and a retry hook on the existing 1 s GC tick in the store task.
  - `src/model/tests.rs`: refinement tests.
- **Wire traffic:** a few extra small queries for instances that are missing a record, at most a handful per instance. There is no change for instances that are fully answered.
- **Dependencies:** none.
- **Platforms:** the fix applies to both Linux (avahi) and macOS (mDNSResponder), and to instances discovered on the wire alone.
