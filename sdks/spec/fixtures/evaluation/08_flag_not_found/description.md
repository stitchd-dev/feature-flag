# 08 — Flag not in snapshot

The requested `flag_key` does not exist in the SDK's current snapshot. The SDK
must return an `EvalResult` with:
- `variant_key` = empty string
- `value` = `null` (SDK cannot infer a type-default without the flag type)
- `outcome` = `"flag_not_found"`

It must also still emit a `FlagEvaluationEvent` so observability can flag the
misconfiguration. Event emission is not asserted by this fixture — it's
asserted by the integration test in Phase 6.
