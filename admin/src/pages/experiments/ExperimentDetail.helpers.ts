/**
 * Pure-logic helpers for ExperimentDetail.tsx.
 *
 * Phase 8 cutover: ExperimentDetail no longer reads from the `EXPERIMENTS` mock
 * data; everything derives from the live `getExperiment(...)` + `getExperimentResults(...)`
 * API. These helpers exist so the derivation logic is testable independent of
 * the page render path.
 *
 * Conventions:
 *   • All inputs are nullable to mirror the page's loading + partial-fetch states.
 *   • Outputs that feed JSX are pre-formatted strings or numbers — the page
 *     itself only handles layout, not formatting.
 */
import type {
  ExperimentResults,
  ContextTypeResultJson,
  VariantResultJson,
} from '../../lib/types'
import type { ExperimentSummary } from '../../lib/api'

// ── Display model ────────────────────────────────────────────────────────────
//
// Phase 9 will replace these mock-flavoured strings with real numbers + units;
// for now the helpers keep parity with the EXPERIMENTS mock shape so existing
// viz components don't need a refactor pass.

export interface ExperimentDisplay {
  /** Number of variants (control + treatments). Min 1, typically 2-5. */
  variants: number
  /** Model label as displayed in the badge — preserves API casing. */
  model: string
  /** Formatted "+X.X%" or "—" when no data. */
  lift: string
  /** Bayesian: P(beats control) as integer percent. Frequentist: confidence as integer percent. 0 when no data. */
  confidence: number
  /** Total samples across all variants, formatted with thousands separators or "0" when none. */
  samples: string
  /** Coarse remaining-time label. "ready" when min samples reached, "—" when no data, else "running". */
  remaining: string
  /** Bound flag key (from experiment summary). */
  flag: string
  /** First metric_id (used by the Metrics tab — Phase 9 surfaces the resolved name). */
  primary: string
  /** Started-at as a locale date string, or "—" when draft. */
  started: string
}

// ── Pickers ──────────────────────────────────────────────────────────────────

/**
 * Pick the active context-type block from a results envelope.
 *
 * Falls back to the first available context type when the requested one is
 * missing — this keeps the page render-safe during transitions (e.g. when the
 * persisted active type was removed from the experiment between visits).
 *
 * Returns `null` when the results object has no context types at all.
 */
export function pickContextTypeResult(
  results: ExperimentResults | null,
  activeContextType: string | null,
): { contextType: string; block: ContextTypeResultJson } | null {
  if (!results) return null
  const keys = Object.keys(results.results_by_context_type)
  if (keys.length === 0) return null
  const key =
    activeContextType && results.results_by_context_type[activeContextType]
      ? activeContextType
      : keys[0]
  return { contextType: key, block: results.results_by_context_type[key] }
}

/**
 * Returns the canonical control row (or null when the results don't include
 * one — should never happen for a well-formed iteration).
 */
export function findControlRow(
  variants: VariantResultJson[],
): VariantResultJson | null {
  // Frequentist convention: control row has `p_value === null` and
  // `expected_lift === null` and `prob_to_beat_control === null`. Use that as
  // the primary signal; fall back to first row when no row matches.
  const ctrl = variants.find(
    (v) => v.p_value === null && v.expected_lift === null,
  )
  return ctrl ?? variants[0] ?? null
}

/** Returns the row flagged `is_winner`, or null when none exists yet. */
export function findWinnerRow(
  variants: VariantResultJson[],
): VariantResultJson | null {
  return variants.find((v) => v.is_winner) ?? null
}

// ── Formatters ───────────────────────────────────────────────────────────────

/** "+4.7%" / "−1.2%" / "—" — formats a lift fraction as a signed percent string. */
export function formatLift(liftFraction: number | null): string {
  if (liftFraction == null || !Number.isFinite(liftFraction)) return '—'
  const pct = liftFraction * 100
  const sign = pct > 0 ? '+' : pct < 0 ? '−' : ''
  return `${sign}${Math.abs(pct).toFixed(1)}%`
}

/** Integer percent in [0, 100]; clamps + rounds. Returns 0 when the input is null. */
export function asConfidencePercent(p: number | null): number {
  if (p == null || !Number.isFinite(p)) return 0
  return Math.max(0, Math.min(100, Math.round(p * 100)))
}

/** Thousands-separated sample-count string, or "0" when empty. */
export function formatSamples(total: number | null): string {
  if (total == null || !Number.isFinite(total)) return '0'
  return Math.max(0, Math.floor(total)).toLocaleString('en-US')
}

// ── Display model builder ────────────────────────────────────────────────────

/**
 * Builds the page's display model from the API responses + the active context
 * type. Phase 9 will replace the mock-flavoured remaining/lift/etc. with real
 * iteration-aware values; for Phase 8 we extract what we can from results and
 * fall back to "—" / 0 / "running" placeholders.
 *
 * Pass `useBayesian` so the confidence field reflects the correct
 * frequentist-vs-bayesian view (matches the existing toggle behaviour).
 */
export function buildExperimentDisplay(
  exp: ExperimentSummary | null,
  results: ExperimentResults | null,
  activeContextType: string | null,
  useBayesian: boolean,
): ExperimentDisplay {
  const picked = pickContextTypeResult(results, activeContextType)
  const variantRows = picked?.block.variants ?? []
  const winner = findWinnerRow(variantRows)

  // Confidence: bayesian = P(beats control) of the winner, frequentist = 1 - p-value.
  let confidence = 0
  if (winner) {
    if (useBayesian) {
      confidence = asConfidencePercent(winner.prob_to_beat_control)
    } else if (winner.p_value != null) {
      confidence = asConfidencePercent(1 - winner.p_value)
    }
  }

  const totalSamples = variantRows.reduce(
    (acc, v) => acc + (v.sample_size ?? 0),
    0,
  )

  // The remaining-time label is iteration-aware (Phase 9 will compute it from
  // iteration.started_at + duration). For now: "ready" when we have a winner
  // declared, "running" when there's data but no winner yet, "—" otherwise.
  let remaining = '—'
  if (variantRows.length > 0) {
    remaining = winner ? 'ready' : 'running'
  }

  return {
    variants: exp?.variants ?? (variantRows.length > 0 ? variantRows.length : 2),
    model: exp?.model ?? 'frequentist',
    lift: formatLift(winner?.expected_lift ?? null),
    confidence,
    samples: formatSamples(totalSamples > 0 ? totalSamples : null),
    remaining,
    flag: exp?.flag_key ?? '—',
    primary: exp?.metric_ids?.[0] ?? '—',
    started: exp?.started_at
      ? new Date(exp.started_at).toLocaleDateString()
      : '—',
  }
}

// ── Bound-target label ───────────────────────────────────────────────────────

/**
 * Returns the user-facing label for the experiment's bound target — used in
 * the page header subtitle. Falls back to "—" when results aren't loaded yet.
 */
export function boundTargetLabel(
  results: ExperimentResults | null,
): string {
  if (!results) return '—'
  return results.bound_target.label
}

// ── Context type list ────────────────────────────────────────────────────────

/**
 * Returns the list of context types for the experiment, in a stable order.
 *
 * Source of truth (in priority order):
 *   1. results.results_by_context_type keys — the most authoritative since
 *      they reflect what the stats service has computed.
 *   2. Empty array — the page falls back to ['user'] in that case.
 *
 * Phase 9 will lift this from the experiment's `unit_context_types` field
 * directly once the experiment summary surfaces it; for Phase 8 the results
 * envelope is the only source available to the admin UI.
 */
export function listContextTypes(results: ExperimentResults | null): string[] {
  if (!results) return []
  return Object.keys(results.results_by_context_type).sort()
}
