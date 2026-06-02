/**
 * Capacity bar for an exclusion group (Phase 8 — xexp).
 *
 * Renders a horizontal bar visualising allocated vs free basis points, plus a
 * compact "allocated% · N members" caption. Pure-render — driven entirely by
 * the group it's handed, so it's exercised via `renderToString` in tests.
 */
import type { ExclusionGroup } from '../../../lib/api/exclusionGroups'
import { allocatedPercent, formatBpPercent } from './capacity'

interface Props {
  group: Pick<ExclusionGroup, 'allocated_bp' | 'free_bp'>
  /** Number of experiments assigned to this group. */
  memberCount: number
}

export function CapacityBar({ group, memberCount }: Props) {
  const allocPct = allocatedPercent(group)
  const nearFull = allocPct >= 90

  return (
    <div data-testid="capacity-bar">
      <div
        role="progressbar"
        aria-valuenow={Math.round(allocPct)}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label="Allocated capacity"
        style={{
          height: 8,
          borderRadius: 4,
          background: 'var(--bg-sunken)',
          border: '1px solid var(--border)',
          overflow: 'hidden',
        }}
      >
        <div
          style={{
            width: `${allocPct}%`,
            height: '100%',
            background: nearFull ? 'var(--danger)' : 'var(--accent)',
            transition: 'width 0.2s',
          }}
        />
      </div>
      <div
        style={{
          display: 'flex',
          justifyContent: 'space-between',
          marginTop: 4,
          fontSize: 11,
          color: 'var(--fg-muted)',
          fontFamily: 'var(--font-mono)',
        }}
      >
        <span>
          {`${formatBpPercent(group.allocated_bp)} allocated · ${formatBpPercent(group.free_bp)} free`}
        </span>
        <span>{`${memberCount} ${memberCount === 1 ? 'member' : 'members'}`}</span>
      </div>
    </div>
  )
}
