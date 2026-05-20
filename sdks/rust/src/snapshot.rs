//! Definition snapshot — the SDK's in-memory copy of flag and segment data.
//!
//! See `sdks/spec/docs/03-caching.md` "Cache 1 — Definition Snapshot" for the
//! canonical lifecycle and concurrency rules.
//!
//! ## Invariants
//!
//! - **Lock-free reads.** `evaluate()` reads the snapshot on the hot path and
//!   must never block on the background polling task's swap. We use
//!   `arc_swap::ArcSwap<Arc<DefinitionSnapshot>>` so readers grab an `Arc` of
//!   the current snapshot in a single atomic load.
//! - **Full snapshots only.** Every `SyncDefinitions` poll produces a fresh
//!   snapshot — there are no deltas, no patches. The swap is unconditional.
//! - **Snapshots are immutable.** Once handed to `ArcSwap::store`, a snapshot
//!   is read-only. Subsequent writes replace it.
//!
//! ## Wire-format alignment
//!
//! The snapshot stores the proto types from `stitchd-proto` directly
//! (`FeatureFlag`, `RuleSegment`, `ListSegmentMeta`) — they carry everything
//! `evaluate()` needs and converting them now would be premature work that
//! [Phase 5 Task 8] handles just-in-time.

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};

use stitchd_proto::flags::v1::FeatureFlag;
use stitchd_proto::sdk::v1::SyncDefinitionsResponse;
use stitchd_proto::segments::v1::{ListSegmentMeta, RuleSegment};

use crate::event_buffer::TypedValue;

/// The metric value-type an event definition accepts. Mirrors
/// `stitchd_core::event::EventValueType` but is duplicated here so the
/// SDK crate has no dependency on the server-side `stitchd-core::event`
/// module. The `snake_case` serde rename matches the wire shape used by
/// `event_definitions` rows in the gateway / analytics-service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventValueType {
    /// Boolean metric (e.g. conversion flag).
    Bool,
    /// 64-bit integer metric (e.g. click count).
    Int,
    /// 64-bit floating-point metric (e.g. revenue).
    Double,
}

impl EventValueType {
    /// Return the `EventValueType` that corresponds to a [`TypedValue`]
    /// variant. Used by `Client::track()` to check whether a caller-supplied
    /// value matches the registered metric type.
    #[must_use]
    pub fn of(value: &TypedValue) -> Self {
        match value {
            TypedValue::Bool(_) => Self::Bool,
            TypedValue::Int(_) => Self::Int,
            TypedValue::Double(_) => Self::Double,
        }
    }

    /// Parse the wire-form `value_type` string used by
    /// `EventDefinitionMeta.value_type` in `SyncDefinitions`. Returns
    /// `None` for unknown variants — the polling layer drops those entries
    /// silently rather than failing the whole sync. This keeps the SDK
    /// forward-compatible if the gateway introduces a new value type the
    /// SDK doesn't yet understand.
    #[must_use]
    pub fn from_wire_str(s: &str) -> Option<Self> {
        match s {
            "bool" => Some(Self::Bool),
            "int" => Some(Self::Int),
            "double" => Some(Self::Double),
            _ => None,
        }
    }
}

/// Immutable in-memory snapshot of the SDK's environment.
///
/// Constructed via [`DefinitionSnapshot::from_proto`] and replaced wholesale
/// by the definition polling task (Phase 5 Task 5). Reads use the indexed
/// accessors below — never iterate the underlying maps on the hot path.
#[derive(Debug, Default)]
pub struct DefinitionSnapshot {
    /// Flag definitions keyed by `flag.key`.
    flags: HashMap<String, FeatureFlag>,
    /// Rule-based segment definitions keyed by segment id (UUID string).
    /// `RuleSegment.rule_payload` holds the serialised condition tree.
    rule_segments: HashMap<String, RuleSegment>,
    /// List-segment metadata keyed by segment id (UUID string).
    /// Entries (members) are NOT included — those live in the LRU cache.
    list_segments: HashMap<String, ListSegmentMeta>,
    /// Pre-registered event definitions keyed by `event_key`.
    /// Used by `Client::track()` for client-side validation (unknown
    /// event_keys + value-type mismatches are warned + skipped).
    ///
    /// Populated automatically by [`Self::from_proto`] from the
    /// `SyncDefinitions.event_definitions` field on every poll. Tests and
    /// the conformance runner may seed entries directly via
    /// [`Self::with_event_definitions`].
    event_definitions: HashMap<String, EventValueType>,
    /// Server clock at snapshot construction (`SyncDefinitionsResponse.server_timestamp_ms`).
    /// Diagnostic only — useful for "how stale is my snapshot" logging.
    server_timestamp_ms: i64,
    /// Resolved environment id from the gateway. Diagnostic only — backend
    /// services authoritatively resolve env from the SDK key.
    environment_id: String,
}

impl DefinitionSnapshot {
    /// Build a snapshot from a `SyncDefinitions` response.
    ///
    /// Duplicate flag keys (proto allows them; spec doesn't) keep the
    /// **last** occurrence and log nothing — duplicate keys at this layer
    /// are a backend data integrity bug that surfaces in the eval-side
    /// outcomes downstream, not here.
    #[must_use]
    pub fn from_proto(resp: SyncDefinitionsResponse) -> Self {
        let mut flags = HashMap::with_capacity(resp.flags.len());
        for f in resp.flags {
            flags.insert(f.key.clone(), f);
        }
        let mut rule_segments = HashMap::with_capacity(resp.rule_segments.len());
        for s in resp.rule_segments {
            rule_segments.insert(s.id.clone(), s);
        }
        // Note: list_segments can contain duplicate ids when a single
        // list-segment targets multiple context types (one ListSegmentMeta
        // per (id, context_type) pair). Keying by id alone means the
        // snapshot only retains the LAST context_type for that id. That's
        // acceptable for now — the SDK uses list_segments only for the
        // "is this segment referenced by any flag" filter and to know the
        // segment's context_type for the membership lookup. Multi-context
        // list-segments are spec-deferred (see sdks/spec/docs/02 §Segment Resolution).
        let mut list_segments = HashMap::with_capacity(resp.list_segments.len());
        for s in resp.list_segments {
            list_segments.insert(s.id.clone(), s);
        }
        // Event definitions: filter unparseable value_type strings rather
        // than failing the whole snapshot — keeps the SDK forward-compatible
        // with a gateway that ships a new value-type variant. Dropped
        // entries effectively become "unregistered" on this SDK build,
        // which is the safest fallback (warn + skip on track()).
        let mut event_definitions = HashMap::with_capacity(resp.event_definitions.len());
        for d in resp.event_definitions {
            if let Some(vt) = EventValueType::from_wire_str(&d.value_type) {
                event_definitions.insert(d.event_key, vt);
            }
        }
        Self {
            flags,
            rule_segments,
            list_segments,
            event_definitions,
            server_timestamp_ms: resp.server_timestamp_ms,
            environment_id: resp.environment_id,
        }
    }

    /// Replace the `event_definitions` cache with the supplied map.
    ///
    /// In production [`Self::from_proto`] populates this automatically from
    /// the polled `SyncDefinitions` response. This builder remains useful
    /// for tests and the conformance runner that don't go through the
    /// polling layer.
    #[must_use]
    pub fn with_event_definitions(mut self, defs: HashMap<String, EventValueType>) -> Self {
        self.event_definitions = defs;
        self
    }

    /// Look up an event definition by `event_key`. Returns the registered
    /// [`EventValueType`] if present.
    #[must_use]
    pub fn event_definition(&self, event_key: &str) -> Option<EventValueType> {
        self.event_definitions.get(event_key).copied()
    }

    /// Total event-definition count — diagnostic.
    #[must_use]
    pub fn event_definition_count(&self) -> usize {
        self.event_definitions.len()
    }

    /// Look up a flag by key. Returns `None` if no flag with this key exists
    /// in the current snapshot.
    #[must_use]
    pub fn flag(&self, key: &str) -> Option<&FeatureFlag> {
        self.flags.get(key)
    }

    /// Look up a rule-based segment by UUID. Returns `None` if absent.
    #[must_use]
    pub fn rule_segment(&self, segment_id: &str) -> Option<&RuleSegment> {
        self.rule_segments.get(segment_id)
    }

    /// Look up list-segment metadata by UUID. Returns `None` if absent
    /// (either unknown or the segment is rule-based, not list-based).
    #[must_use]
    pub fn list_segment(&self, segment_id: &str) -> Option<&ListSegmentMeta> {
        self.list_segments.get(segment_id)
    }

    /// Iterate every flag key in the snapshot. Used by Phase 5 Task 6 to
    /// compute the "segments referenced by any flag rule" filter set.
    pub fn flag_keys(&self) -> impl Iterator<Item = &str> {
        self.flags.keys().map(String::as_str)
    }

    /// Iterate every flag in the snapshot.
    pub fn flags(&self) -> impl Iterator<Item = &FeatureFlag> {
        self.flags.values()
    }

    /// Iterate every list-segment id in the snapshot. Used by the LRU
    /// refresh task (Phase 5 Task 6) when building the batch request.
    pub fn list_segment_ids(&self) -> impl Iterator<Item = &str> {
        self.list_segments.keys().map(String::as_str)
    }

    /// Total flag count — diagnostic.
    #[must_use]
    pub fn flag_count(&self) -> usize {
        self.flags.len()
    }

    /// Total rule-segment count — diagnostic.
    #[must_use]
    pub fn rule_segment_count(&self) -> usize {
        self.rule_segments.len()
    }

    /// Total list-segment count — diagnostic.
    #[must_use]
    pub fn list_segment_count(&self) -> usize {
        self.list_segments.len()
    }

    /// Server clock at snapshot construction (epoch milliseconds).
    /// 0 if the gateway didn't supply one (e.g. test fixtures).
    #[must_use]
    pub const fn server_timestamp_ms(&self) -> i64 {
        self.server_timestamp_ms
    }

    /// Resolved environment id from the gateway. Empty when constructed
    /// outside a real sync (e.g. tests, conformance runner).
    #[must_use]
    pub fn environment_id(&self) -> &str {
        &self.environment_id
    }
}

/// Concurrent snapshot store — read by `evaluate()`, written by the
/// definition polling task. Lock-free reads via `arc_swap::ArcSwap`.
///
/// Clones cheaply (it wraps an `Arc`).
#[derive(Clone)]
pub struct DefinitionStore {
    inner: Arc<ArcSwap<DefinitionSnapshot>>,
}

impl DefinitionStore {
    /// Build a store seeded with an empty snapshot. The first call to
    /// `SdkClient::init` then immediately replaces this with a real one
    /// from the gateway. We never expose the empty state to `evaluate()`
    /// (init blocks on the first sync per spec).
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ArcSwap::from_pointee(DefinitionSnapshot::default())),
        }
    }

    /// Build a store seeded with the supplied snapshot. Used by tests
    /// and the conformance runner.
    #[must_use]
    pub fn from_snapshot(snapshot: DefinitionSnapshot) -> Self {
        Self {
            inner: Arc::new(ArcSwap::from_pointee(snapshot)),
        }
    }

    /// Get an `Arc` pointing at the current snapshot. Lock-free, never
    /// blocks. The returned `Arc` is stable — even if the store swaps to
    /// a new snapshot mid-evaluation, the caller continues operating on
    /// the version it grabbed.
    #[must_use]
    pub fn load(&self) -> Arc<DefinitionSnapshot> {
        self.inner.load_full()
    }

    /// Atomically replace the current snapshot. Concurrent readers see
    /// either the old or new snapshot, never a torn state.
    pub fn store(&self, snapshot: DefinitionSnapshot) {
        self.inner.store(Arc::new(snapshot));
    }
}

impl Default for DefinitionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for DefinitionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.load();
        f.debug_struct("DefinitionStore")
            .field("flag_count", &s.flag_count())
            .field("rule_segment_count", &s.rule_segment_count())
            .field("list_segment_count", &s.list_segment_count())
            .finish()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(key: &str) -> FeatureFlag {
        FeatureFlag {
            key: key.to_string(),
            ..Default::default()
        }
    }

    fn rule_seg(id: &str, key: &str) -> RuleSegment {
        RuleSegment {
            id: id.to_string(),
            key: key.to_string(),
            context_type: String::new(),
            rule_payload: vec![],
        }
    }

    fn list_seg(id: &str, key: &str, ctx_type: &str) -> ListSegmentMeta {
        ListSegmentMeta {
            id: id.to_string(),
            key: key.to_string(),
            context_type: ctx_type.to_string(),
        }
    }

    fn snapshot_with(
        flags: Vec<FeatureFlag>,
        rule_segments: Vec<RuleSegment>,
        list_segments: Vec<ListSegmentMeta>,
    ) -> DefinitionSnapshot {
        DefinitionSnapshot::from_proto(SyncDefinitionsResponse {
            flags,
            rule_segments,
            list_segments,
            server_timestamp_ms: 1_700_000_000_000,
            environment_id: "env-1".to_string(),
            event_definitions: vec![],
        })
    }

    // ── DefinitionSnapshot construction ─────────────────────────────────────

    #[test]
    fn default_is_empty() {
        let s = DefinitionSnapshot::default();
        assert_eq!(s.flag_count(), 0);
        assert_eq!(s.rule_segment_count(), 0);
        assert_eq!(s.list_segment_count(), 0);
        assert_eq!(s.server_timestamp_ms(), 0);
        assert!(s.environment_id().is_empty());
    }

    #[test]
    fn from_proto_populates_all_indexes() {
        let s = snapshot_with(
            vec![flag("a"), flag("b")],
            vec![rule_seg("seg-1", "pro-users")],
            vec![list_seg("seg-2", "beta-orgs", "org")],
        );
        assert_eq!(s.flag_count(), 2);
        assert_eq!(s.rule_segment_count(), 1);
        assert_eq!(s.list_segment_count(), 1);
        assert_eq!(s.server_timestamp_ms(), 1_700_000_000_000);
        assert_eq!(s.environment_id(), "env-1");
    }

    // ── Accessors ───────────────────────────────────────────────────────────

    #[test]
    fn flag_lookup_hits_existing_key() {
        let s = snapshot_with(vec![flag("checkout")], vec![], vec![]);
        assert!(s.flag("checkout").is_some());
    }

    #[test]
    fn flag_lookup_misses_unknown_key() {
        let s = snapshot_with(vec![flag("checkout")], vec![], vec![]);
        assert!(s.flag("nonexistent").is_none());
    }

    #[test]
    fn segment_lookups_differentiate_rule_and_list() {
        // Same id namespace, but a rule_segment doesn't appear in
        // list_segment() and vice-versa.
        let s = snapshot_with(
            vec![],
            vec![rule_seg("rule-1", "k")],
            vec![list_seg("list-1", "k", "user")],
        );
        assert!(s.rule_segment("rule-1").is_some());
        assert!(s.list_segment("rule-1").is_none());
        assert!(s.list_segment("list-1").is_some());
        assert!(s.rule_segment("list-1").is_none());
    }

    // ── Iterators ───────────────────────────────────────────────────────────

    #[test]
    fn flag_keys_iter_visits_every_flag() {
        let s = snapshot_with(vec![flag("a"), flag("b"), flag("c")], vec![], vec![]);
        let mut keys: Vec<&str> = s.flag_keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[test]
    fn list_segment_ids_iter_visits_every_list_segment() {
        let s = snapshot_with(
            vec![],
            vec![],
            vec![list_seg("x", "k1", "user"), list_seg("y", "k2", "org")],
        );
        let mut ids: Vec<&str> = s.list_segment_ids().collect();
        ids.sort();
        assert_eq!(ids, vec!["x", "y"]);
    }

    // ── Duplicate handling ──────────────────────────────────────────────────

    #[test]
    fn duplicate_flag_keys_keep_last() {
        let mut f1 = flag("dup");
        f1.enabled = false;
        let mut f2 = flag("dup");
        f2.enabled = true;
        let s = snapshot_with(vec![f1, f2], vec![], vec![]);
        assert_eq!(s.flag_count(), 1);
        assert!(s.flag("dup").unwrap().enabled);
    }

    // ── DefinitionStore: lock-free swap ─────────────────────────────────────

    #[test]
    fn store_load_returns_stored_snapshot() {
        let store = DefinitionStore::from_snapshot(snapshot_with(vec![flag("a")], vec![], vec![]));
        let snap = store.load();
        assert_eq!(snap.flag_count(), 1);
        assert!(snap.flag("a").is_some());
    }

    #[test]
    fn store_swap_replaces_snapshot_atomically() {
        let store = DefinitionStore::new();
        // Initial empty.
        assert_eq!(store.load().flag_count(), 0);
        // Swap in a populated snapshot.
        store.store(snapshot_with(vec![flag("a"), flag("b")], vec![], vec![]));
        assert_eq!(store.load().flag_count(), 2);
        // Swap in a smaller snapshot — old data fully replaced (not merged).
        store.store(snapshot_with(vec![flag("c")], vec![], vec![]));
        let s = store.load();
        assert_eq!(s.flag_count(), 1);
        assert!(s.flag("a").is_none());
        assert!(s.flag("c").is_some());
    }

    #[test]
    fn store_load_returns_stable_arc_across_swap() {
        // Critical invariant — a reader that loaded an Arc continues to
        // see its snapshot even after the store has swapped to a new one.
        let store =
            DefinitionStore::from_snapshot(snapshot_with(vec![flag("original")], vec![], vec![]));
        let reader_snap = store.load();
        // Swap on a different thread (via the same store handle).
        let store_clone = store.clone();
        let handle = std::thread::spawn(move || {
            store_clone.store(snapshot_with(vec![flag("replaced")], vec![], vec![]));
        });
        handle.join().unwrap();
        // The reader still sees "original", even though the store now has "replaced".
        assert!(reader_snap.flag("original").is_some());
        assert!(reader_snap.flag("replaced").is_none());
        // A fresh load picks up the swap.
        assert!(store.load().flag("replaced").is_some());
    }

    #[test]
    fn store_is_send_and_sync() {
        // Required so SdkClient can be Arc-shared across handlers.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DefinitionStore>();
        assert_send_sync::<DefinitionSnapshot>();
    }

    #[test]
    fn store_clones_share_underlying_state() {
        // DefinitionStore::clone() returns a handle to the SAME ArcSwap,
        // so a swap via one handle is visible to the other.
        let a = DefinitionStore::new();
        let b = a.clone();
        a.store(snapshot_with(vec![flag("via-a")], vec![], vec![]));
        assert!(b.load().flag("via-a").is_some());
    }

    // ── EventValueType + event_definitions cache ────────────────────────────

    #[test]
    fn event_value_type_serde_round_trips_snake_case() {
        // Must match `stitchd_core::event::EventValueType` so a future
        // polling-layer extension that ships event_definitions via
        // SyncDefinitions can wire straight in without renames.
        let cases = [
            (EventValueType::Bool, r#""bool""#),
            (EventValueType::Int, r#""int""#),
            (EventValueType::Double, r#""double""#),
        ];
        for (val, expected) in cases {
            let s = serde_json::to_string(&val).unwrap();
            assert_eq!(s, expected, "serialise");
            let parsed: EventValueType = serde_json::from_str(&s).unwrap();
            assert_eq!(parsed, val, "round-trip");
        }
    }

    #[test]
    fn event_value_type_of_typed_value() {
        use crate::event_buffer::TypedValue;
        assert_eq!(EventValueType::of(&TypedValue::Bool(true)), EventValueType::Bool);
        assert_eq!(EventValueType::of(&TypedValue::Int(7)), EventValueType::Int);
        assert_eq!(EventValueType::of(&TypedValue::Double(1.5)), EventValueType::Double);
    }

    #[test]
    fn event_definitions_default_is_empty() {
        let s = snapshot_with(vec![], vec![], vec![]);
        assert_eq!(s.event_definition_count(), 0);
        assert!(s.event_definition("anything").is_none());
    }

    #[test]
    fn with_event_definitions_populates_cache() {
        let mut defs = HashMap::new();
        defs.insert("checkout_completed".to_string(), EventValueType::Bool);
        defs.insert("revenue".to_string(), EventValueType::Double);
        defs.insert("clicks".to_string(), EventValueType::Int);
        let s = snapshot_with(vec![], vec![], vec![]).with_event_definitions(defs);
        assert_eq!(s.event_definition_count(), 3);
        assert_eq!(s.event_definition("checkout_completed"), Some(EventValueType::Bool));
        assert_eq!(s.event_definition("revenue"), Some(EventValueType::Double));
        assert_eq!(s.event_definition("clicks"), Some(EventValueType::Int));
        assert!(s.event_definition("not_registered").is_none());
    }

    // ── from_proto event_definitions wiring ─────────────────────────────────

    #[test]
    fn from_proto_populates_event_definitions_cache() {
        use stitchd_proto::sdk::v1::EventDefinitionMeta;
        let resp = SyncDefinitionsResponse {
            flags: vec![],
            rule_segments: vec![],
            list_segments: vec![],
            server_timestamp_ms: 0,
            environment_id: String::new(),
            event_definitions: vec![
                EventDefinitionMeta {
                    event_key: "checkout_completed".into(),
                    value_type: "bool".into(),
                },
                EventDefinitionMeta {
                    event_key: "revenue".into(),
                    value_type: "double".into(),
                },
                EventDefinitionMeta {
                    event_key: "clicks".into(),
                    value_type: "int".into(),
                },
            ],
        };
        let s = DefinitionSnapshot::from_proto(resp);
        assert_eq!(s.event_definition_count(), 3);
        assert_eq!(
            s.event_definition("checkout_completed"),
            Some(EventValueType::Bool)
        );
        assert_eq!(s.event_definition("revenue"), Some(EventValueType::Double));
        assert_eq!(s.event_definition("clicks"), Some(EventValueType::Int));
    }

    #[test]
    fn from_proto_handles_unknown_value_type_gracefully() {
        // A gateway running a newer schema might ship a value_type the SDK
        // doesn't yet understand. Drop that entry but keep the rest — never
        // fail the whole snapshot.
        use stitchd_proto::sdk::v1::EventDefinitionMeta;
        let resp = SyncDefinitionsResponse {
            flags: vec![],
            rule_segments: vec![],
            list_segments: vec![],
            server_timestamp_ms: 0,
            environment_id: String::new(),
            event_definitions: vec![
                EventDefinitionMeta {
                    event_key: "checkout_completed".into(),
                    value_type: "bool".into(),
                },
                EventDefinitionMeta {
                    event_key: "future_thing".into(),
                    value_type: "garbage".into(),
                },
                EventDefinitionMeta {
                    event_key: "revenue".into(),
                    value_type: "double".into(),
                },
            ],
        };
        let s = DefinitionSnapshot::from_proto(resp);
        assert_eq!(s.event_definition_count(), 2);
        assert_eq!(
            s.event_definition("checkout_completed"),
            Some(EventValueType::Bool)
        );
        assert_eq!(s.event_definition("revenue"), Some(EventValueType::Double));
        assert!(s.event_definition("future_thing").is_none());
    }

    #[test]
    fn from_wire_str_round_trips_known_variants() {
        assert_eq!(EventValueType::from_wire_str("bool"), Some(EventValueType::Bool));
        assert_eq!(EventValueType::from_wire_str("int"), Some(EventValueType::Int));
        assert_eq!(
            EventValueType::from_wire_str("double"),
            Some(EventValueType::Double)
        );
        assert_eq!(EventValueType::from_wire_str(""), None);
        assert_eq!(EventValueType::from_wire_str("Bool"), None); // case sensitive
        assert_eq!(EventValueType::from_wire_str("string"), None);
    }

    #[test]
    fn debug_includes_counts_not_full_data() {
        let store = DefinitionStore::from_snapshot(snapshot_with(
            vec![flag("a"), flag("b")],
            vec![rule_seg("r", "k")],
            vec![list_seg("l", "k", "user")],
        ));
        let s = format!("{store:?}");
        assert!(s.contains("flag_count: 2"));
        assert!(s.contains("rule_segment_count: 1"));
        assert!(s.contains("list_segment_count: 1"));
        // Should NOT dump every flag/segment (those Debug outputs would be huge).
        assert!(!s.contains("key:"), "Debug must not dump flag bodies: {s}");
    }
}
