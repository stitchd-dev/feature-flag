# Experimentation Service (`stitchd-experimentation-service`)

## Responsibility

Manages the full lifecycle of A/B experiments and computes statistical results:

- Experiment CRUD (create, update, delete)
- Experiment lifecycle management (Draft → Active → Paused → Concluded)
- Iteration tracking (each continuous run of an experiment is a separate iteration)
- Statistical results computation (participant counts, metric averages, p-values)

## Port

| Transport | Default Port |
|-----------|-------------|
| gRPC | `50055` |

## Service: `ExperimentationService`

**Package:** `stitchd.experiments.v1`

### CRUD RPCs

| RPC | Request | Response | Description |
|-----|---------|----------|-------------|
| `CreateExperiment` | `CreateExperimentRequest` | `Experiment` | Create a new experiment |
| `GetExperiment` | `GetExperimentRequest` | `Experiment` | Fetch a single experiment |
| `ListExperiments` | `ListExperimentsRequest` | `ListExperimentsResponse` | List all experiments for an environment |
| `UpdateExperiment` | `UpdateExperimentRequest` | `Experiment` | Update experiment metadata |
| `DeleteExperiment` | `DeleteExperimentRequest` | `Experiment` | Delete an experiment (returns the deleted record) |

### `TransitionExperiment`

```
rpc TransitionExperiment(TransitionExperimentRequest) returns (Experiment)
```

Move an experiment through its lifecycle state machine.

Valid transitions:

| From | To | Meaning |
|------|----|---------|
| `DRAFT` | `ACTIVE` | Start collecting data |
| `ACTIVE` | `PAUSED` | Temporarily pause data collection |
| `PAUSED` | `ACTIVE` | Resume data collection |
| `ACTIVE` or `PAUSED` | `CONCLUDED` | Finalise the experiment |

**`TransitionExperimentRequest` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `environment_id` | string | Environment scope |
| `experiment_id` | string | Experiment to transition |
| `new_status` | `ExperimentStatus` | Target status |
| `reason` | string | Optional human-readable reason for the transition |

### `ListIterations`

```
rpc ListIterations(ListIterationsRequest) returns (ListIterationsResponse)
```

List all iterations for an experiment. An iteration is created each time the experiment transitions to `ACTIVE`; a new iteration starts if the experiment is paused and then reactivated.

**`ExperimentIteration` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Iteration ID |
| `experiment_id` | string | Parent experiment |
| `iteration_number` | int32 | Sequential counter starting at 1 |
| `started_at_ms` | int64 | Start time (epoch ms) |
| `ended_at_ms` | int64 | End time (0 while still running) |
| `metric_keys` | repeated string | Metric keys collected in this iteration |
| `traffic_allocation` | double | Fraction of traffic routed to the experiment (0.0–1.0) |

### `GetResults`

```
rpc GetResults(GetResultsRequest) returns (ExperimentResults)
```

Compute and return statistical results for an experiment across all concluded or active iterations.

**`ExperimentResults` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `experiment_id` | string | Experiment ID |
| `variant_results` | repeated `VariantResult` | Per-variant statistical breakdown |
| `computed_at_ms` | int64 | Timestamp of the computation |

**`VariantResult` fields:**

| Field | Type | Description |
|-------|------|-------------|
| `variant_key` | string | Variant identifier (e.g., `control`, `treatment`) |
| `participant_count` | uint64 | Number of unique participants in this variant |
| `metric_values` | map&lt;string, double&gt; | Average value per metric key |
| `p_value` | double | Statistical significance p-value (if computable) |
| `p_value_present` | bool | `false` if insufficient data to compute p-value |

## `Experiment` Message Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Unique experiment ID |
| `environment_id` | string | Environment scope |
| `name` | string | Display name |
| `description` | string | Human-readable description |
| `flag_key` | string | Feature flag driving variant assignment |
| `status` | `ExperimentStatus` | `DRAFT`, `ACTIVE`, `PAUSED`, or `CONCLUDED` |
| `variant_keys` | repeated string | Variant keys from the linked flag |
| `created_at_ms` | int64 | Creation time (epoch ms) |
| `updated_at_ms` | int64 | Last update time (epoch ms) |
| `version` | uint64 | Optimistic-locking version |

## Auth Requirements

All RPCs require Bearer JWT. The RBAC context injected by the gateway must include a role with the relevant flag/experiment management permissions for the target environment.
