# 05 — List-segment membership via LRU hit

Tests `InSegment` against a list-based segment when the membership is already
in the LRU cache. Runner pre-seeds the LRU via `list_segment_memberships.json::preseed_lru`.
No network calls should occur.
