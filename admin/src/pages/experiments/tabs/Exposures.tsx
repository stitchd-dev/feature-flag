/**
 * Exposures tab — Phase 9 Task 9.2.
 *
 * Renders two stacked sections:
 *   1. SRM panel — reads `results_by_context_type[activeContextType].srm` and
 *      surfaces per-variant assigned/expected/deviation/chi-sq counts.
 *      Health pill colour comes from the gateway's `health` field with a
 *      client-side override that flips to "red" whenever
 *      `overall_chi_sq_p < 0.001` (the spec's hard threshold).
 *   2. Paginated exposure table — reads the `exposures` prop populated by
 *      `listExperimentExposures()` upstream. URL-driven `?page=N` is
 *      delegated to the parent via `onPageChange`.
 *
 * Above both: a "Bound to: <label>" badge sourced from
 * `results.bound_target.label`.
 *
 * The component is intentionally presentational — fetching + URL handling
 * live on the page so this can be tested in isolation.
 */
import { I } from '../../../components/icons'
import type {
  ExperimentResults,
  PaginatedExposures,
} from '../../../lib/types'
import { pickContextTypeResult } from '../ExperimentDetail.helpers'

// ── Pure helpers (exported for direct unit testing) ─────────────────────────

export type SrmHealth = 'green' | 'yellow' | 'red'

/**
 * Computes the SRM health bucket from the overall chi-squared p-value.
 *
 * Thresholds (per spec §3 — SRM detection):
 *   • red    — chi_sq_p < 0.001
 *   • yellow — 0.001 ≤ chi_sq_p < 0.01
 *   • green  — chi_sq_p ≥ 0.01 (includes null / no-data)
 *
 * The gateway emits a `health` field too; we re-derive here so the UI is
 * resilient to stale snapshots (e.g. cached results returning a "green"
 * health for new data that's actually red).
 */
export function srmHealthClass(chiSqP: number | null): SrmHealth {
  if (chiSqP == null || !Number.isFinite(chiSqP)) return 'green'
  if (chiSqP < 0.001) return 'red'
  if (chiSqP < 0.01) return 'yellow'
  return 'green'
}

/**
 * Clamps a URL-supplied page parameter to [1, max(1, totalPages)]. Accepts
 * either a number or a string (URL params come in as strings); returns 1 on
 * NaN or zero totals.
 */
export function clampPage(raw: number | string, totalPages: number): number {
  const n = typeof raw === 'number' ? raw : Number.parseInt(raw, 10)
  if (!Number.isFinite(n) || Number.isNaN(n)) return 1
  const ceil = Math.max(1, totalPages)
  return Math.min(ceil, Math.max(1, Math.floor(n)))
}

// ── Formatting ───────────────────────────────────────────────────────────────

function fmtPct(fraction: number, opts: { signed?: boolean } = {}): string {
  const pct = fraction * 100
  if (opts.signed) {
    const sign = pct > 0 ? '+' : pct < 0 ? '−' : ''
    return `${sign}${Math.abs(pct).toFixed(2)}%`
  }
  return `${pct.toFixed(2)}%`
}

function fmtChiSq(p: number | null): string {
  if (p == null || !Number.isFinite(p)) return '—'
  if (p < 0.001) return '<0.001'
  return p.toFixed(3)
}

function fmtTimestamp(iso: string): string {
  try {
    return new Date(iso).toLocaleString()
  } catch {
    return iso
  }
}

// ── Health pill ──────────────────────────────────────────────────────────────

const PILL_STYLES: Record<
  SrmHealth,
  { background: string; color: string; border: string }
> = {
  green: {
    background: 'var(--success-bg)',
    color: 'var(--success)',
    border: 'rgba(17,122,61,0.3)',
  },
  yellow: {
    background: 'rgba(184,118,26,0.10)',
    color: '#B8761A',
    border: 'rgba(184,118,26,0.3)',
  },
  red: {
    background: 'var(--danger-bg)',
    color: 'var(--danger)',
    border: 'rgba(196,43,28,0.3)',
  },
}

function HealthPill({ health, label }: { health: SrmHealth; label: string }) {
  const s = PILL_STYLES[health]
  return (
    <span
      data-srm-health={health}
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 6,
        padding: '3px 10px',
        fontSize: 11,
        fontWeight: 600,
        textTransform: 'uppercase',
        letterSpacing: '0.05em',
        borderRadius: 12,
        background: s.background,
        color: s.color,
        border: `1px solid ${s.border}`,
      }}
    >
      {label}
    </span>
  )
}

// ── ExposuresPanel ──────────────────────────────────────────────────────────

interface Props {
  results: ExperimentResults | null
  activeContextType: string
  exposures: PaginatedExposures | null
  page: number
  onPageChange: (next: number) => void
  loading: boolean
  error: string | null
}

export function ExposuresPanel({
  results,
  activeContextType,
  exposures,
  page,
  onPageChange,
  loading,
  error,
}: Props) {
  const picked = pickContextTypeResult(results, activeContextType)
  const srm = picked?.block.srm ?? null
  const boundLabel = results?.bound_target.label ?? '—'
  const boundKind = results?.bound_target.kind

  const health = srmHealthClass(srm?.overall_chi_sq_p ?? null)
  const healthLabel =
    health === 'red'
      ? 'SRM detected'
      : health === 'yellow'
        ? 'SRM warning'
        : 'Healthy'

  return (
    <div>
      {/* Bound-target badge */}
      <div
        style={{
          marginBottom: 14,
          display: 'flex',
          alignItems: 'center',
          gap: 10,
          fontSize: 12,
          color: 'var(--fg-muted)',
        }}
      >
        <span>Bound to:</span>
        <span
          className="badge"
          style={{
            background:
              boundKind === 'default_rule'
                ? 'var(--info-bg, var(--bg-sunken))'
                : 'var(--bg-sunken)',
          }}
        >
          {boundKind === 'default_rule' ? '⤓ ' : ''}
          {boundLabel}
        </span>
      </div>

      {/* SRM panel */}
      <div className="card">
        <div className="card-header">
          <div className="card-title">
            Exposures ({activeContextType}) · SRM
          </div>
          <HealthPill health={health} label={healthLabel} />
        </div>
        {srm == null || srm.per_variant.length === 0 ? (
          <div
            style={{
              padding: 24,
              textAlign: 'center',
              color: 'var(--fg-muted)',
              fontSize: 13,
            }}
          >
            No SRM data yet for this context type.
          </div>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Variant</th>
                <th>Observed</th>
                <th>Expected</th>
                <th>Deviation</th>
                <th>χ² p-value</th>
              </tr>
            </thead>
            <tbody>
              {srm.per_variant.map((row) => (
                <tr key={row.variant_key}>
                  <td>
                    <span className="badge">{row.variant_key}</span>
                  </td>
                  <td style={{ fontFamily: 'var(--font-mono)' }}>
                    {row.observed_count.toLocaleString('en-US')}
                  </td>
                  <td style={{ fontFamily: 'var(--font-mono)' }}>
                    {row.expected_count.toLocaleString('en-US')}
                  </td>
                  <td
                    style={{
                      fontFamily: 'var(--font-mono)',
                      color:
                        Math.abs(row.deviation) > 0.1
                          ? 'var(--danger)'
                          : 'var(--fg-muted)',
                    }}
                  >
                    {fmtPct(row.deviation, { signed: true })}
                  </td>
                  <td
                    style={{
                      fontFamily: 'var(--font-mono)',
                      color:
                        health === 'red' ? 'var(--danger)' : 'var(--fg)',
                    }}
                  >
                    {fmtChiSq(srm.overall_chi_sq_p)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {/* Paginated exposure table */}
      <div className="card" style={{ marginTop: 18 }}>
        <div className="card-header">
          <div className="card-title">
            Exposures · per-context assignments
          </div>
          <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>
            {exposures
              ? `${exposures.total.toLocaleString('en-US')} total`
              : '—'}
          </span>
        </div>
        {error != null ? (
          <div
            style={{
              padding: '12px 16px',
              background: 'var(--danger-bg)',
              color: 'var(--danger)',
              fontSize: 13,
            }}
          >
            {error}
          </div>
        ) : loading && exposures == null ? (
          <div
            style={{
              padding: 24,
              textAlign: 'center',
              color: 'var(--fg-muted)',
              fontSize: 13,
            }}
          >
            Loading exposures…
          </div>
        ) : exposures == null || exposures.items.length === 0 ? (
          <div
            style={{
              padding: 24,
              textAlign: 'center',
              color: 'var(--fg-muted)',
              fontSize: 13,
            }}
          >
            No exposures recorded yet for this context type.
          </div>
        ) : (
          <>
            <table className="table">
              <thead>
                <tr>
                  <th>Context key</th>
                  <th>Variant</th>
                  <th>Assigned at</th>
                  <th>Matched rule</th>
                </tr>
              </thead>
              <tbody>
                {exposures.items.map((row) => (
                  <tr key={`${row.context_type}:${row.context_key}`}>
                    <td className="mono-key">{row.context_key}</td>
                    <td>
                      <span className="badge">{row.variant_key}</span>
                    </td>
                    <td style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                      {fmtTimestamp(row.assigned_at)}
                    </td>
                    <td
                      style={{
                        fontFamily: 'var(--font-mono)',
                        fontSize: 12,
                        color: row.matched_rule_id
                          ? 'var(--fg)'
                          : 'var(--fg-muted)',
                      }}
                    >
                      {row.matched_rule_id ?? '—'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            {(() => {
              // The exposures endpoint remains page/total-based (it is not one
              // of the cursor-migrated list endpoints), so this tab keeps an
              // inline page-numbered control rather than the shared cursor
              // <Pagination>. Styling mirrors the former shared component.
              const totalPages = Math.max(1, Math.ceil(exposures.total / exposures.per_page))
              const isFirst = page <= 1
              const isLast = page >= totalPages
              const from = Math.min((page - 1) * exposures.per_page + 1, exposures.total)
              const to = Math.min(page * exposures.per_page, exposures.total)
              return (
                <div
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'flex-end',
                    gap: 8,
                    padding: '10px 16px',
                    borderTop: '1px solid var(--border-faint)',
                    fontSize: 13,
                    color: 'var(--fg-muted)',
                  }}
                >
                  <span>{from}–{to} of {exposures.total}</span>
                  <button
                    className="icon-btn"
                    disabled={isFirst}
                    onClick={() => !isFirst && onPageChange(page - 1)}
                    aria-label="Previous page"
                    style={{ opacity: isFirst ? 0.35 : 1 }}
                  >
                    <I.chevronLeft size={14} />
                  </button>
                  <span style={{ fontVariantNumeric: 'tabular-nums' }}>
                    {page} / {totalPages}
                  </span>
                  <button
                    className="icon-btn"
                    disabled={isLast}
                    onClick={() => !isLast && onPageChange(page + 1)}
                    aria-label="Next page"
                    style={{ opacity: isLast ? 0.35 : 1 }}
                  >
                    <I.chevronRight size={14} />
                  </button>
                </div>
              )
            })()}
          </>
        )}
      </div>
    </div>
  )
}
