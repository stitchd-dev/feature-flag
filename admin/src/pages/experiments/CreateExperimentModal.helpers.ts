/**
 * Pure helpers for CreateExperimentModal — kept separate from the React
 * component so they can be unit-tested in isolation (see
 * `CreateExperimentModal.test.ts`).
 */

export type MetricKind = 'aggregation' | 'ratio' | 'funnel'

export interface MetricPickerOption {
  id: string
  key: string
  name: string
  kind: MetricKind
}

/** Shape of the form values for the experiment-create modal. */
export interface ExperimentCreateFormValues {
  name: string
  key: string
  flag_key: string
  description?: string
  model: string
  metric_ids: string[]
  duration_days: number
  variants: { key: string; allocation: number }[]
}

// ─── Picker styling ─────────────────────────────────────────────────────────

/**
 * Kind-chip colour map — mirrors `MetricsList.tsx::kindChipStyle` so the picker
 * looks consistent with the metrics list page.
 */
export function metricKindChipStyle(kind: MetricKind): {
  background: string
  color: string
  border: string
} {
  switch (kind) {
    case 'aggregation':
      return {
        background: 'rgba(59,130,246,0.1)',
        color: '#2563eb',
        border: '1px solid rgba(59,130,246,0.25)',
      }
    case 'ratio':
      return {
        background: 'rgba(168,85,247,0.1)',
        color: '#9333ea',
        border: '1px solid rgba(168,85,247,0.25)',
      }
    case 'funnel':
      return {
        background: 'rgba(34,197,94,0.1)',
        color: '#16a34a',
        border: '1px solid rgba(34,197,94,0.25)',
      }
  }
}

// ─── Picker search ──────────────────────────────────────────────────────────

/**
 * Filter the metric options for the picker dropdown.
 *
 * - Hides options whose `id` is in `selected` (already picked metrics
 *   shouldn't appear as picks).
 * - Performs a case-insensitive substring match on `name` or `key` when
 *   `query` is non-empty.
 * - Empty query returns all non-selected options.
 */
export function filterMetricOptions(
  options: MetricPickerOption[],
  query: string,
  selected: Set<string> = new Set(),
): MetricPickerOption[] {
  const q = query.trim().toLowerCase()
  return options.filter((opt) => {
    if (selected.has(opt.id)) return false
    if (!q) return true
    return (
      opt.name.toLowerCase().includes(q) || opt.key.toLowerCase().includes(q)
    )
  })
}

// ─── Submit body ────────────────────────────────────────────────────────────

/**
 * Build the JSON body for `POST /v1/environments/{env}/experiments`.
 *
 * The Phase 7 cutover replaces the legacy `primary_metric: string` field
 * with `metric_ids: string[]` (UUIDs referencing `metric_definitions` rows).
 */
export function buildExperimentCreateBody(
  values: ExperimentCreateFormValues,
  environmentId: string,
): Record<string, unknown> {
  return {
    key: values.key.trim(),
    name: values.name.trim(),
    description: values.description?.trim() || undefined,
    flag_key: values.flag_key.trim(),
    model: values.model,
    metric_ids: values.metric_ids,
    duration_days: Number(values.duration_days),
    environment_id: environmentId,
    variants: values.variants.map((v) => ({
      key: v.key.trim(),
      allocation: v.allocation / 100,
    })),
  }
}
