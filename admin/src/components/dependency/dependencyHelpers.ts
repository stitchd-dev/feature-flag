/**
 * Pure helpers for dependency-graph visualization + the delete-blocked UX
 * (flag_lifecycle_20260604, Phase 8.4).
 */
import type { DependencyEdge, DependencyExistsError } from '../../lib/types'

/**
 * Extract a structured `409 dependency_exists` body from an axios error, or
 * null if the error is not a dependency-block. Mirrors the gateway envelope
 * `{ error: "dependency_exists", dependents: [...], message }`.
 */
export function parseDependencyExists(err: unknown): DependencyExistsError | null {
  const e = err as { response?: { status?: number; data?: unknown } } | undefined
  if (e?.response?.status !== 409) return null
  const data = e.response.data as Record<string, unknown> | undefined
  if (!data || data.error !== 'dependency_exists') return null
  const dependents = Array.isArray(data.dependents)
    ? (data.dependents as unknown[]).map(String)
    : []
  return {
    error: 'dependency_exists',
    dependents,
    message:
      typeof data.message === 'string'
        ? data.message
        : 'This entity is still referenced by other entities. Remove the references first.',
  }
}

/** Human label for an edge relationship kind. */
export function edgeKindLabel(kind: string): string {
  switch (kind) {
    case 'prerequisite_flag':
      return 'prerequisite flag'
    case 'segment_ref':
      return 'segment reference'
    case 'dependent_flag':
      return 'dependent flag'
    case 'prerequisite_flag_variant':
      return 'flag-variant start prerequisite'
    case 'prerequisite_experiment_done':
      return 'experiment-done start prerequisite'
    default:
      return kind.replace(/_/g, ' ')
  }
}

/** Best display label for an edge: its key, else its (possibly-truncated) id. */
export function edgeLabel(edge: DependencyEdge): string {
  if (edge.key) return edge.key
  return edge.id.length > 12 ? `${edge.id.slice(0, 8)}…` : edge.id
}

/** Summary counts for the recharts overview bar. */
export function graphCounts(upstream: DependencyEdge[], downstream: DependencyEdge[]): {
  upstream: number
  downstream: number
} {
  return { upstream: upstream.length, downstream: downstream.length }
}
