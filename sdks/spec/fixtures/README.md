# Conformance Fixtures

Test vectors every Stitchd SDK implementation MUST pass. Format is JSON so any
language can consume them directly.

## Layout

```
fixtures/
├── README.md                                this file
├── hashing/
│   └── reference_vectors.json               murmur3 → bucket reference outputs
└── evaluation/
    ├── 01_bool_default_rule/                simplest: bool flag, no rules, returns default variant
    ├── 02_string_eq_rule/                   string flag, one Eq leaf condition
    ├── 03_percentage_rollout/               percentage allocation; bucket from reference_vectors
    ├── 04_rule_segment_member/              rule references a rule-based segment
    ├── 05_list_segment_hit/                 rule references a list-based segment; membership pre-cached
    ├── 06_list_segment_miss/                rule references a list-based segment; LRU miss → on-demand fetch
    ├── 07_reasoning_trace/                  asserts the shape of reasoning trace output
    └── 08_flag_not_found/                   asserts default-for-type + outcome="flag_not_found"
```

## Scenario File Format

Every `fixtures/evaluation/<scenario>/` directory contains exactly these files:

| File | Schema | Contents |
|---|---|---|
| `flag_definitions.json` | (see *Definition Shapes* below) | The flag definitions the SDK's snapshot is pre-seeded with for this scenario |
| `segment_definitions.json` | (see *Definition Shapes* below) | The segment definitions (rule + list) the snapshot is pre-seeded with |
| `list_segment_memberships.json` | (optional, only for list-segment scenarios) | Pre-seeded LRU contents OR responses the mock gateway should return for batch membership lookups |
| `requests.json` | `eval_request.schema.json` (as JSON array) | Array of EvalRequest objects |
| `expected.json` | `eval_result.schema.json` OR `eval_result_with_reasoning.schema.json` (as JSON array) | Expected evaluation outputs, in same order as `requests.json` |
| `description.md` | (free text) | What this scenario verifies and why |

## Conformance Runner Contract

A conformance runner for any language SDK MUST:

1. Walk every directory under `fixtures/evaluation/`.
2. For each scenario:
   - Load `flag_definitions.json` + `segment_definitions.json` into the SDK's
     in-memory snapshot (bypassing the network — use whatever test seam the
     SDK exposes).
   - If `list_segment_memberships.json` is present:
     - Pre-seed the LRU with its `preseed_lru` entries (if any).
     - Configure a mock HTTP server to return the file's `on_miss_responses`
       (if any) for `POST /v1/sdk/segments/list:batch`. If a scenario expects
       a miss-fetch and no mock response is provided, that's a fixture error.
   - Decide whether to call `evaluate()` or `evaluate_with_reasoning()` based
     on whether `expected.json[].reasoning` is present.
   - Pass each entry of `requests.json` (in order) and collect results.
   - Assert `results[i]` matches `expected.json[i]` field-by-field. For
     reasoning traces, `evaluated_at` is the only field allowed to differ
     (SDK fills its own clock).

A runner is **conformant** when all scenarios pass with zero deviations.

## Definition Shapes

`flag_definitions.json` and `segment_definitions.json` use a JSON shape derived
from the proto definitions in `sdks/spec/proto/sdk/v1/service.proto`. The
informal grammar:

```jsonc
// flag_definitions.json
{
  "flags": [
    {
      "key": "checkout-flow",
      "value_type": "string",                          // bool|int|double|string|json
      "enabled": true,
      "environment_id": "00000000-0000-0000-0000-000000000001",
      "variants": [
        {"key": "control", "value": "v1"},
        {"key": "treatment", "value": "v2"}
      ],
      "default_rule": {
        "variant_assignment": {"specific_variant": "control"}
      },
      "rules": [
        {
          "rule_id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
          "rule_name": "beta-users",
          "condition": { /* ConditionTree from condition_tree.schema.json */ },
          "variant_assignment": {
            "specific_variant": "treatment"
            // OR
            // "percentage_rollout": {
            //   "targets": [{"context_type": "user", "field": "Key"}],
            //   "weights": [{"variant_key": "control", "weight": 500},
            //               {"variant_key": "treatment", "weight": 500}]
            // }
          }
        }
      ]
    }
  ]
}
```

```jsonc
// segment_definitions.json
{
  "rule_segments": [
    {
      "id": "11111111-1111-1111-1111-111111111111",
      "key": "pro-users",
      "context_type": "user",
      "condition": { /* ConditionTree */ }
    }
  ],
  "list_segments": [
    {
      "id": "22222222-2222-2222-2222-222222222222",
      "key": "early-access-orgs",
      "context_type": "org"
    }
  ]
}
```

```jsonc
// list_segment_memberships.json
{
  "preseed_lru": [
    {
      "context_type": "user",
      "context_key": "alice",
      "memberships": {
        "11111111-1111-1111-1111-111111111111": true,
        "22222222-2222-2222-2222-222222222222": false
      }
    }
  ],
  "on_miss_responses": [
    {
      "match_query": {
        "context_type": "user",
        "context_key": "alice",
        "segment_ids_contains": "22222222-2222-2222-2222-222222222222"
      },
      "return_memberships": {
        "22222222-2222-2222-2222-222222222222": true
      }
    }
  ]
}
```

## Why Scenarios Cap at 08

This is the **initial** conformance suite. Phase 6 (`Integration + Conformance`)
adds the runner. Future tracks will append scenarios as new behavioural
contracts are added (streaming, MFA-keyed SDKs, etc.). The numbering is left
sparse-able (e.g. `09_…`, `10_…`) so insertions don't renumber existing files.
