# Segmentation Service (`stitchd-segmentation-service`)

## Responsibility

Stores and evaluates segment definitions. Segments are reusable targeting criteria:

- **Rule segments** — dynamic: membership is evaluated at request time from context attributes and rules
- **List segments** — static: membership is a pre-populated set of context keys

Responsibilities include:

- Segment CRUD (create, update, delete)
- Membership evaluation for a given context (used by the flag service during flag evaluation)
- Serving segment definitions to the gateway REST API
- SDK list-check endpoint (membership check without full flag evaluation)

## Port

| Transport | Default Port |
|-----------|-------------|
| gRPC | `50053` |

## Service: `SegmentationService`

**Package:** `stitchd.segments.v1`

### `GetSegment`

```
rpc GetSegment(GetSegmentRequest) returns (SegmentBundle)
```

Fetch a single segment by key. Returns a `SegmentBundle` containing rule segments and list segments with that key.

**`GetSegmentRequest` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `environment_id` | string | Environment scope |
| `segment_key` | string | Segment identifier |

### `ListSegments`

```
rpc ListSegments(ListSegmentsRequest) returns (ListSegmentsResponse)
```

List all segments for an environment, returning separate lists for rule segments and list segments.

**`ListSegmentsResponse` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `rule_segments` | repeated `RuleSegment` | Dynamic rule-based segments |
| `list_segments` | repeated `ListSegmentMeta` | Static list-based segments (metadata only) |

### `EvaluateMembership`

```
rpc EvaluateMembership(EvaluateMembershipRequest) returns (EvaluateMembershipResponse)
```

Evaluate whether a specific context key is a member of a segment. Handles both rule-based evaluation (attribute matching) and list-based lookup. Called by `stitchd-flag-service` during flag evaluation when a flag has a segment targeting rule.

**`EvaluateMembershipRequest` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `environment_id` | string | Environment scope |
| `segment_key` | string | Segment to check |
| `context_key` | string | The context value to test for membership |
| `context_type` | string | The context type (e.g., `user`, `device`) |

**`EvaluateMembershipResponse` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `is_member` | bool | `true` if the context key is a member of the segment |

### `MutateSegment`

```
rpc MutateSegment(MutateSegmentRequest) returns (MutateSegmentResponse)
```

Create, update, or delete a segment. Exactly one of `rule_segment` or `list_segment` must be set in the request.

**`MutateSegmentRequest` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `environment_id` | string | Environment scope |
| `kind` | `SegmentMutationKind` | `CREATE`, `UPDATE`, or `DELETE` |
| `rule_segment` | `RuleSegment` (oneof) | Rule-based segment definition |
| `list_segment` | `ListSegment` (oneof) | List-based segment definition |
| `version` | uint64 | Optimistic-locking token |

## Auth Requirements

Internal service — called by the gateway REST layer and by `stitchd-flag-service`. RBAC context is injected by the gateway; the segmentation service does not re-validate credentials.
