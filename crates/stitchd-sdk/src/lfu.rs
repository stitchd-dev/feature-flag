//! LFU frequency tracker and pre-computed membership cache for list segments.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Tracks evaluation frequency per `(context_type, context_key)` within a tumbling window.
pub struct LfuTracker {
    counts: HashMap<(String, String), u64>,
    capacity: usize,
    window: Duration,
    window_start: Instant,
}

impl LfuTracker {
    pub fn new(capacity: usize, window: Duration) -> Self {
        Self {
            counts: HashMap::new(),
            capacity,
            window,
            window_start: Instant::now(),
        }
    }

    /// Record one evaluation for this context. Resets the window if it has expired.
    pub fn record(&mut self, context_type: &str, context_key: &str) {
        let now = Instant::now();
        if now.duration_since(self.window_start) >= self.window {
            self.counts.clear();
            self.window_start = now;
        }
        *self
            .counts
            .entry((context_type.to_owned(), context_key.to_owned()))
            .or_insert(0) += 1;
    }

    /// Return at most `capacity` (context_type, context_key) pairs sorted by descending frequency.
    pub fn hot_set(&self) -> Vec<(String, String)> {
        let mut entries: Vec<_> = self.counts.iter().collect();
        entries.sort_by(|a, b| b.1.cmp(a.1));
        entries
            .into_iter()
            .take(self.capacity)
            .map(|(k, _)| k.clone())
            .collect()
    }
}

/// Pre-computed list-segment membership for hot contexts.
#[derive(Default)]
pub struct LfuCache {
    // (context_type, context_key, segment_key) → is_member
    entries: HashMap<(String, String, String), bool>,
}

impl LfuCache {
    pub fn get(&self, ctx_type: &str, ctx_key: &str, seg_key: &str) -> Option<bool> {
        self.entries
            .get(&(ctx_type.to_owned(), ctx_key.to_owned(), seg_key.to_owned()))
            .copied()
    }

    /// Atomically replace all entries (called after each batch refresh).
    pub fn replace(&mut self, new: HashMap<(String, String, String), bool>) {
        self.entries = new;
    }
}

/// Combined LFU state — held behind a `Mutex` in `SdkClient`.
pub struct LfuState {
    pub tracker: LfuTracker,
    pub cache: LfuCache,
}

impl LfuState {
    pub fn new(capacity: usize, window: Duration) -> Self {
        Self {
            tracker: LfuTracker::new(capacity, window),
            cache: LfuCache::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn tracker_records_and_returns_hot_set() {
        let mut t = LfuTracker::new(2, Duration::from_secs(60));
        t.record("user", "u1");
        t.record("user", "u1");
        t.record("user", "u2");
        t.record("user", "u3");

        let hot = t.hot_set();
        assert_eq!(hot.len(), 2);
        assert_eq!(hot[0], ("user".to_string(), "u1".to_string()));
    }

    #[test]
    fn tracker_resets_on_window_expiry() {
        let mut t = LfuTracker::new(10, Duration::from_nanos(1));
        t.record("user", "u1");
        std::thread::sleep(Duration::from_millis(1));
        // Force window expiry by recording again
        t.record("user", "u2");
        let hot = t.hot_set();
        // After reset only u2 was recorded this window
        assert_eq!(hot.len(), 1);
        assert_eq!(hot[0].1, "u2");
    }

    #[test]
    fn lfu_cache_get_and_replace() {
        let mut cache = LfuCache::default();
        let mut entries = HashMap::new();
        entries.insert(("user".to_string(), "u1".to_string(), "beta".to_string()), true);
        cache.replace(entries);

        assert_eq!(cache.get("user", "u1", "beta"), Some(true));
        assert_eq!(cache.get("user", "u1", "other"), None);
        assert_eq!(cache.get("org", "u1", "beta"), None);
    }
}
