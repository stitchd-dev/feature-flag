import type { AdminFlagResponse } from '../../lib/types'

/**
 * Lifecycle badges for the flags list (flag_lifecycle_20260604, Phase 8.5).
 *
 * Computed purely from the list DTO (which now carries `prerequisites`):
 *   - "has prerequisites" — the flag is gated on other flags.
 *   - "is a prerequisite" — another flag in the list gates on this flag.
 * ("has schedule" is env-scoped and shown on the flag's Schedule tab, not the
 *  project-scoped list, to avoid an N-call fan-out.)
 */

/** Build the set of flag keys that are referenced as a prerequisite by some flag. */
export function buildPrerequisiteOfSet(flags: AdminFlagResponse[]): Set<string> {
  const set = new Set<string>()
  for (const f of flags) {
    for (const p of f.prerequisites ?? []) {
      if (p.prerequisite_flag_key) set.add(p.prerequisite_flag_key)
    }
  }
  return set
}

/** Whether a flag has its own prerequisite gate configured. */
export function hasPrerequisites(flag: AdminFlagResponse): boolean {
  return (flag.prerequisites?.length ?? 0) > 0
}

const PILL_STYLE: React.CSSProperties = {
  fontSize: 10,
  padding: '1px 6px',
  borderRadius: 8,
  fontWeight: 600,
  whiteSpace: 'nowrap',
}

/** Render the lifecycle badges for a flag. Renders nothing when none apply. */
export function FlagBadges({
  flag,
  isPrerequisiteOf,
}: {
  flag: AdminFlagResponse
  isPrerequisiteOf: boolean
}) {
  const badges: React.ReactNode[] = []
  if (hasPrerequisites(flag)) {
    badges.push(
      <span
        key="has-prereq"
        title="This flag is gated on other flags"
        style={{ ...PILL_STYLE, background: 'var(--accent-subtle, #eff6ff)', color: 'var(--accent, #3b82f6)' }}
      >
        has prerequisites
      </span>,
    )
  }
  if (isPrerequisiteOf) {
    badges.push(
      <span
        key="is-prereq"
        title="Another flag depends on this flag as a prerequisite"
        style={{ ...PILL_STYLE, background: 'var(--success-bg, #f0fdf4)', color: 'var(--success-fg, #166534)' }}
      >
        is a prerequisite
      </span>,
    )
  }
  if (badges.length === 0) return null
  return <span style={{ display: 'inline-flex', gap: 4, flexWrap: 'wrap' }}>{badges}</span>
}
