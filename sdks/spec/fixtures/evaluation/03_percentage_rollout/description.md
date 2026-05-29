# 03 — Percentage rollout (50/50)

A `checkout-flow` flag with a 50/50 percentage allocation on `user.Key`.
Weights are basis points (0–10000 = 100%): `control` 5000, `treatment` 5000.
Uses verified hash inputs from `fixtures/hashing/reference_vectors.json`:

- `alice` hashes to bucket **5110** → cumulative weight of `control` (5000) is
  not > 5110, so `treatment` (cumulative 10000) wins.
- `bob` hashes to bucket **2334** → cumulative weight of `control` (5000) > 2334,
  so `control` wins.

If this scenario fails, your SDK's hash function does not match the reference
implementation in `stitchd-core/src/hashing.rs::calculate_allocation`.
