# 06 — List-segment LRU miss → on-demand fetch

Same flag/segment as scenario 05, but with an empty pre-seeded LRU. The runner
must wire up a mock HTTP endpoint for `POST /v1/sdk/segments/list:batch` that
returns the membership specified in `list_segment_memberships.json::on_miss_responses`.

Verifies the on-miss path:
1. `evaluate()` encounters list-segment InSegment(early-access-orgs).
2. LRU.get(("org", "startup")) returns None.
3. SDK fires a synchronous batch lookup against the mock endpoint.
4. Result is inserted into the LRU.
5. Membership read returns `true`, rule matches, returns `on`.
