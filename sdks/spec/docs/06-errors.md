# 06 — Errors

This document classifies every error condition the SDK can encounter and
specifies the canonical handling for each. Implementations MUST map these to
language-idiomatic error types but the **classification and behaviour** are
spec-mandated.

## Taxonomy

### Class A: Configuration Errors (`init`-time only)

Invalid input from the application embedding the SDK. The SDK MUST fail-fast
on these — never proceed with a malformed config.

| Error | Detected | Action |
|---|---|---|
| `gateway_url` is empty or malformed URL | `init` | Return error immediately; do not attempt connection |
| `sdk_key` is empty | `init` | Return error immediately |
| Any `Duration` config field is zero or negative | `init` | Return error with field name |
| `lru_max_entries` is zero | `init` | Return error |
| `event_batch_size` is zero | `init` | Return error |
| `event_flush_interval > event_batch_size * sane-bound` | `init` | (Soft warning, not error — odd config but legal) |

### Class B: Authentication Errors

The SDK key was rejected by the gateway.

| Error | Detected | Action |
|---|---|---|
| First definition sync returns `Unauthenticated` (gRPC 16 / HTTP 401) | `init` | Return error; SDK does NOT start background tasks |
| In-flight definition sync returns `Unauthenticated` | Poll loop | Log ERROR; DO NOT swap snapshot; continue retrying (admin may un-revoke). Application continues serving from last snapshot. |
| In-flight LRU refresh / event flush returns `Unauthenticated` | LRU / event loop | Same as above — log, do not panic, continue retrying |

The SDK **never** surfaces an auth error to an `evaluate()` caller after `init`
succeeds. Evaluations continue against the last-known snapshot.

### Class C: Transient Network Errors

Connection refused, timeout, gRPC `Unavailable`, DNS failure, etc.

| Error | Detected | Action |
|---|---|---|
| First sync transient failure | `init` | Return error (no fallback — caller can retry `init`) |
| Polling-loop transient failure | Background | Exponential backoff (see `04-polling.md`); never panic |
| LRU on-miss synchronous fetch transient failure | `evaluate()` | Treat list-segment membership as `false` for this eval; do NOT insert into LRU; emit event with `segment_evaluations[].source = "lru_miss_failed"` |
| Event flush transient failure | Background | Re-enqueue batch at head; retry next interval |

### Class D: Snapshot Inconsistency

The local snapshot references something that doesn't exist.

| Condition | Action |
|---|---|
| `evaluate()` requests an unknown `flag_key` | Return default-for-type variant; emit event with `outcome = "flag_not_found"`; do NOT log (this is application-driven, expected) |
| Rule references `segment_id` not present in snapshot | Treat condition as `false`; log WARN once per `(flag_key, segment_id)` (rate-limited); the segment may show up on next poll |
| Rule references `variant_key` not in flag's variants | Treat as misconfiguration: log ERROR; return the flag's `default_rule` variant; emit event |
| Snapshot has zero flags AND zero segments | Legal (newly-provisioned env); no error |

### Class E: Programming Errors

Caller misuse. SDK MAY panic / throw if invariants are violated, but SHOULD
prefer typed errors.

| Condition | Action |
|---|---|
| `evaluate()` called after `shutdown()` | Return error (or panic in languages where post-shutdown calls are clearly UB) |
| `init()` called twice on the same SDK instance | Return error (second call) |

## Error Surface — Public API

Every SDK MUST expose at least these error categories to its public API:

```
SdkError =
  | ConfigError(message)        // Class A
  | AuthError(message)          // Class B (init-time only — runtime auth failures stay internal)
  | NetworkError(message)       // Class C (init-time only)
  | StateError(message)         // Class E (post-shutdown, double-init)
```

These four are the minimum. Implementations MAY expose finer-grained variants
(e.g. separate `Timeout` from `ConnectionRefused`) but MUST NOT collapse them
into a single opaque error type that hides Class B vs Class C distinctions
from the caller.

## Logging Discipline

The SDK SHOULD use the language's idiomatic structured logger (slog / pino /
structlog / zap / ...) at these levels:

| Level | When |
|---|---|
| `ERROR` | First-time `Unauthenticated`; snapshot reference to unknown variant; flush buffer overflow |
| `WARN` | Backoff escalation past 2 consecutive failures; segment_id not found; event flush dropping oldest |
| `INFO` | Successful first sync; clean shutdown |
| `DEBUG` | Per-poll-tick fired (interval reached); cache miss → fetch; cache hit (extremely chatty — opt-in only) |
| `TRACE` | Per-evaluation flow; opt-in only |

The SDK MUST NOT log the SDK key value, even at TRACE level. It MAY log a hash
or last-4-chars suffix for debugging.

## Panic / Crash Discipline

The SDK MUST NOT panic or crash the embedding process in any of these scenarios:

- Gateway returns a malformed response
- Definition snapshot has a corrupt rule
- LRU is full
- Event buffer is full
- Network is completely down for hours

The only legitimate panics:

- `init` config validation failure (Class A) — but prefer returning an error
- Post-`shutdown()` `evaluate()` calls in languages where this is clearly UB
- Internal invariant violations that indicate an SDK bug (e.g. an atomic
  counter went negative)

## Reporting Bugs

When the SDK encounters an internal invariant violation (Class E "this should
be unreachable"), it SHOULD log at ERROR with enough context (flag_key,
context_type, snapshot_version if tracked) to file an issue. It SHOULD NOT
include the SDK key, full context parameters (may contain PII), or stack traces
containing user data.
