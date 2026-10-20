# Design

## Context

See proposal.md for the problem. The required behavior is in the `mdns-discovery` delta.

How refinement works today (`src/model/store.rs`):

- `Store::ingest_record` calls `refinements(&data, at)` for every ingested record, from either source.
  - A PTR pointing at a service instance asks for `[SRV, TXT]` of the instance. It is satisfied by a wire **SRV**.
  - An SRV asks for `[A, AAAA]` of its target. It is satisfied by a wire A **or** AAAA.
- If the satisfying type is already on the wire, nothing is requested. Otherwise a `QueryRequest` goes out, throttled per name by `refine_requested` and `REFINE_INTERVAL` (30 s).
- The scheduler (`src/wire/scheduler.rs`) turns each request into immediate questions, with a 1 s dedup (`REFINE_DEDUP`).
- **Nothing retries.** A request is only repeated if the same PTR or SRV is ingested again more than 30 s later. For wire PTRs this rarely happens, because known-answer suppression keeps responders from re-sending them.

What probing the Brother DCP-L8400CDN (192.168.4.102) on the office LAN showed:

| Query | Answer |
|---|---|
| One PTR question for `_ipp._tcp.local` | 554 bytes: PTR, plus SRV, TXT and A as additional records |
| Five PTR questions for its five service types | 1224 bytes, TC bit clear: all five PTRs, but the additional records stop after the `_ipp` SRV. The `_ipp`, `_scanner` and `_http` TXT, and the `_scanner` and `_http` SRV, are missing |
| A lone TXT question | Answered in 0.03–0.06 s. On the very first probe it took 2.66 s, and a following SRV question went unanswered for 3 s, probably because the printer was waking up |
| 7 packets of 20 filler questions, then the TXT question | Answered in 0.11–0.16 s, three out of three. The burst alone loses nothing |

Wire TXT and mDNSResponder TXT data were byte-identical for every instance where both arrived, so merging already works once the wire record arrives.

## Goals / Non-Goals

**Goals:**
- Every instance keeps being asked for until both its SRV and its TXT are on the wire, or the bounded retries run out.
- Retries do not depend on re-ingesting the PTR or SRV.

**Non-Goals:**
- Changing how enumeration queries are batched (`QUESTIONS_PER_PACKET`), or pacing them.
- Retrying host address lookups differently from instances. They use the same mechanism, but with "A or AAAA" satisfaction.

## Decisions

### 1. Missing types instead of one satisfying type

`refinements` computes the **missing** types:
- **Instance:** SRV is missing unless a wire SRV for the instance exists, and the same for TXT, each checked on its own.
- **Host:** `[A, AAAA]` while neither is on the wire, and nothing once either is.

If nothing is missing, nothing is requested. Otherwise the `QueryRequest` carries only the missing types. An instance whose SRV arrived in a truncated answer then asks for `[TXT]` alone.

*Alternative considered:* making TXT the satisfying type instead of SRV. Rejected, because an instance whose TXT arrived but whose SRV did not would then have the same bug the other way round.

### 2. A refinement table with a retry schedule, replacing `refine_requested`

`refine_requested: HashMap<DnsName, Instant>` becomes `refinements: HashMap<DnsName, Refinement>`:

```rust
struct Refinement {
    kind: RefineKind,          // Instance or Host: decides the missing types
    requested: Instant,        // when the current round started
    retries: usize,            // retries sent in this round
    next_retry: Option<Instant>,
}
```

A round runs as follows:

- **Start.** When a PTR or SRV is ingested and something is missing, the store sends the first request immediately and starts a round, unless a round for that name started less than `REFINE_INTERVAL` ago. `next_retry` = `requested + RETRY_DELAYS[0]`.
- **Retry.** `Store::due_refinements(now) -> Vec<QueryRequest>` handles every entry whose `next_retry` is due. It recomputes the missing types.
  - If nothing is missing, the entry is removed.
  - Otherwise it emits a request for the missing types, increments `retries`, and sets `next_retry` to `now + RETRY_DELAYS[retries]`, or to `None` once the schedule is used up.
- **Cleanup.** Entries without `next_retry` remain only as the throttle. `gc` drops them after `REFINE_INTERVAL`, just as `refine_requested` is pruned today.

`RETRY_DELAYS = [2 s, 8 s, 30 s]`. That gives four requests in about 40 s per round. The first delay is longer than the scheduler's 1 s dedup, so retries are never swallowed by it. The total stays small: on the office LAN about 150 instances, and only those still missing something are retried.

*Alternative considered:* retrying in the wire scheduler. Rejected, because only the store knows which records have arrived; the scheduler sees questions, not satisfaction.

### 3. Driving retries from the store task's GC tick

The store task already ticks every second (`GC_INTERVAL`). On that tick it calls `store.due_refinements(now)` after `gc` and sends the requests on the existing query channel. Retries are therefore at most about 1 s late, which is irrelevant at these delays. No new timer or channel is needed.

## Risks / Trade-offs

- [A service that has no TXT record at all, which violates DNS-SD but happens, is asked for TXT four times per round] → This is bounded: one round per `REFINE_INTERVAL` at most, and only when its PTR or SRV is ingested again.
- [More queries right after startup] → Only instances still missing something are retried, and each retry is one small question. Probing showed bursts are tolerated.
- [The probe results come from one printer model] → The fix does not depend on why a record is missing. Truncated answers, sleeping devices and lost packets are all handled by asking for the missing types again.

## Migration Plan

None needed. Nothing is persisted. Roll back by reverting.
