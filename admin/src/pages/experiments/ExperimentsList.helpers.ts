/**
 * Pure helpers for `ExperimentsList.tsx`.
 *
 * Phase 10 enhancements add:
 *   - Status + flag filters (URL-driven).
 *   - Days-remaining display derived from `scheduled_end_at`.
 *   - Primary metric resolution (id → name) via a lookup table built from
 *     `api.metrics.list(envId)`.
 *   - Exposed-context count (derived from `listExperimentExposures(...)`
 *     with `per_page=1` reading the `total` field).
 *
 * The helpers below are intentionally pure so they can be exhaustively
 * unit-tested without spinning up the React tree.
 */
import type { ExperimentResponse } from '../../lib/types'

export type StatusFilter = 'all' | 'running' | 'draft' | 'stopped' | 'completed' | 'paused'
export const STATUS_FILTERS: StatusFilter[] = [
  'all',
  'running',
  'paused',
  'draft',
  'stopped',
  'completed',
]

export const STATUS_FILTER_LABELS: Record<StatusFilter, string> = {
  all: 'All',
  running: 'Running',
  paused: 'Paused',
  draft: 'Draft',
  stopped: 'Stopped',
  completed: 'Completed',
}

/** Map a status string to the visual badge classname used in the list. */
export function statusBadgeClass(status: string): string {
  switch (status) {
    case 'running':
      return 'info'
    case 'completed':
      return 'success'
    case 'stopped':
      return 'danger'
    case 'paused':
      return 'warning'
    default:
      return ''
  }
}

/** True when an experiment matches the active status filter. */
export function matchesStatus(exp: ExperimentResponse, filter: StatusFilter): boolean {
  if (filter === 'all') return true
  return exp.status.toLowerCase() === filter
}

/** True when an experiment is bound to a flag whose key OR id matches. */
export function matchesFlag(exp: ExperimentResponse, flagFilter: string | null): boolean {
  if (!flagFilter) return true
  return exp.flag_key === flagFilter || exp.flag_id === flagFilter
}

/**
 * Compute "days remaining" given an ISO-8601 `scheduled_end_at`.
 *
 *   - Returns `null` when the experiment has no scheduled end.
 *   - Returns `0` when the scheduled end is in the past.
 *   - Otherwise returns the ceiling number of whole days until end.
 *
 * `now` is injected so tests don't need to monkeypatch `Date.now`.
 */
export function computeDaysRemaining(
  scheduledEndAt: string | null | undefined,
  now: Date = new Date(),
): number | null {
  if (!scheduledEndAt) return null
  const end = new Date(scheduledEndAt)
  if (Number.isNaN(end.getTime())) return null
  const diffMs = end.getTime() - now.getTime()
  if (diffMs <= 0) return 0
  // Ceiling so the UI shows "1 day" right up until midnight.
  return Math.ceil(diffMs / (1000 * 60 * 60 * 24))
}

/**
 * Pretty-print "days remaining" for the list row. `null` → "—",
 * `0` → "Ended", else "N days".
 */
export function formatDaysRemaining(d: number | null): string {
  if (d == null) return '—'
  if (d === 0) return 'Ended'
  return d === 1 ? '1 day' : `${d} days`
}

/** Pick the first context type for an experiment (used as the default exposure
 *  scope when fetching the exposure count). Defaults to "user" when none is
 *  declared. */
export function pickExposureContextType(exp: ExperimentResponse): string {
  const types = exp.unit_context_types ?? []
  return types[0] ?? 'user'
}

/**
 * Builds a quick lookup from metric ID → display label. The label is the
 * metric's `name`, falling back to the `key`.
 */
export function buildMetricNameLookup(
  metrics: { id: string; name: string; key: string }[],
): Map<string, string> {
  const lookup = new Map<string, string>()
  for (const m of metrics) {
    lookup.set(m.id, m.name?.trim() ? m.name : m.key)
  }
  return lookup
}

/** Returns the resolved primary-metric label for an experiment, or "—" when
 *  not in the lookup yet. */
export function resolvePrimaryMetricLabel(
  exp: ExperimentResponse,
  lookup: Map<string, string>,
): string {
  const id = exp.metric_ids?.[0]
  if (!id) return '—'
  return lookup.get(id) ?? id
}
