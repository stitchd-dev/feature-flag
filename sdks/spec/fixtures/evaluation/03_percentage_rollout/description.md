# 03 — Percentage rollout (50/50)

A `checkout-flow` flag with a 50/50 percentage allocation on `user.Key`.
Uses verified hash inputs from `fixtures/hashing/reference_vectors.json`:

- `alice` hashes to bucket **511** → cumulative weight of `control` (500) is
  not > 511, so `treatment` (cumulative 1000) wins.
- `bob` hashes to bucket **233** → cumulative weight of `control` (500) > 233,
  so `control` wins.

If this scenario fails, your SDK's hash function does not match the reference
implementation in `stitchd-core/src/hashing.rs::calculate_allocation`.
