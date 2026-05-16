# 04 — Rule-based segment membership

Tests `InSegment` against a rule-based segment. The segment's own condition
tree is evaluated locally from the snapshot — no network lookup.

- `pro-users` segment matches contexts where `user.plan == "pro"`.
- Flag `premium-feature` returns `true` when the context is in `pro-users`,
  otherwise its default (`false`).
