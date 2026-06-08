/**
 * Pure helpers for the schedule builder (flag_lifecycle_20260604, Phase 8.2).
 *
 * Kept framework-free so they're unit-testable without rendering: formatting
 * run/next/last timestamps, a human diff/summary of a pending mutation, and
 * translating the Formik form values into the gateway `CreateScheduleBody`.
 */
import type { CreateScheduleBody, ScheduledChange, ScheduleStatus } from '../../lib/types'
import { buildWeeklyRrule } from '../../lib/validation/lifecycle'
import type { ScheduleFormValues } from '../../lib/validation/lifecycle'

/** Format an epoch-ms instant for display; `0`/missing → an em-dash. */
export function formatInstant(ms: number | null | undefined): string {
  if (ms == null || ms === 0) return '—'
  const d = new Date(ms)
  if (Number.isNaN(d.getTime())) return '—'
  return d.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

/** A coarse status grouping used to drive list badges + which actions apply. */
export function statusGroup(
  status: ScheduleStatus,
): 'pending' | 'active' | 'paused' | 'terminal' {
  switch (status) {
    case 'pending':
      return 'pending'
    case 'active':
      return 'active'
    case 'paused':
      return 'paused'
    default:
      return 'terminal'
  }
}

/** Which lifecycle actions are valid for a change in its current state. */
export function availableActions(
  change: Pick<ScheduledChange, 'schedule_kind' | 'status'>,
): Array<'cancel' | 'pause' | 'resume'> {
  const g = statusGroup(change.status)
  if (change.schedule_kind === 'one_shot') {
    return g === 'pending' ? ['cancel'] : []
  }
  // recurring
  if (g === 'active') return ['pause']
  if (g === 'paused') return ['resume']
  return []
}

/**
 * Convert an HTML `datetime-local` string (e.g. "2026-06-10T09:00", interpreted
 * by the author in their chosen `tz`) into an epoch-ms instant. The browser
 * parses a bare datetime-local in the LOCAL zone; to honor the picked IANA tz
 * we compute the offset that `tz` had at that wall-clock time.
 */
export function datetimeLocalToEpochMs(value: string, tz: string): number {
  // Parse the wall-clock fields.
  const m = value.match(/^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?$/)
  if (!m) return NaN
  const [, y, mo, d, h, mi, s] = m
  const wall = Date.UTC(+y, +mo - 1, +d, +h, +mi, s ? +s : 0)
  // Determine the offset `tz` applied at that instant by formatting `wall`
  // (treated as UTC) into `tz` and measuring the drift.
  const offset = tzOffsetMs(wall, tz)
  return wall - offset
}

/**
 * The offset (ms) of IANA `tz` from UTC at the given UTC instant
 * (positive east of UTC). DST-aware via `Intl.DateTimeFormat`.
 */
export function tzOffsetMs(utcMs: number, tz: string): number {
  try {
    const dtf = new Intl.DateTimeFormat('en-US', {
      timeZone: tz,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      hour12: false,
    })
    const parts = dtf.formatToParts(new Date(utcMs))
    const map: Record<string, number> = {}
    for (const p of parts) {
      if (p.type !== 'literal') map[p.type] = +p.value
    }
    let hour = map.hour
    if (hour === 24) hour = 0
    const asUtc = Date.UTC(map.year, map.month - 1, map.day, hour, map.minute, map.second)
    return asUtc - utcMs
  } catch {
    return 0
  }
}

/**
 * Translate validated schedule-builder form values into the gateway
 * `CreateScheduleBody`. `mutation_payload` is parsed from the JSON text the
 * entity-specific editor produced (the schema already validated it parses).
 */
export function toCreateBody(values: ScheduleFormValues): CreateScheduleBody {
  const mutation_payload = JSON.parse(values.mutation_payload)
  if (values.schedule_kind === 'one_shot') {
    return {
      mutation_payload,
      schedule_kind: 'one_shot',
      scheduled_at_ms: datetimeLocalToEpochMs(values.scheduled_at ?? '', values.tz),
    }
  }
  return {
    mutation_payload,
    schedule_kind: 'recurring',
    rrule: buildWeeklyRrule(
      (values.weekdays ?? []) as Parameters<typeof buildWeeklyRrule>[0],
      values.hour ?? 0,
      values.minute ?? 0,
    ),
    tz: values.tz,
  }
}

/**
 * A short human-readable schedule line: when it fires.
 * One-shot → "Once at <instant>"; recurring → "Weekly · <rrule> (<tz>)".
 */
export function describeSchedule(change: ScheduledChange): string {
  if (change.schedule_kind === 'recurring') {
    const days = parseRruleByday(change.rrule)
    const time = parseRruleTime(change.rrule)
    const when = days.length ? days.join(', ') : 'weekly'
    return `${when}${time ? ` at ${time}` : ''}${change.tz ? ` (${change.tz})` : ''}`
  }
  return `Once at ${formatInstant(change.scheduled_at_ms)}`
}

/** Extract BYDAY tokens (e.g. ["MO","WE"]) from an RRULE string. */
export function parseRruleByday(rrule: string): string[] {
  const m = rrule.match(/BYDAY=([^;]+)/)
  return m ? m[1].split(',').filter(Boolean) : []
}

/** Extract a "HH:MM" time from an RRULE's BYHOUR/BYMINUTE, when present. */
export function parseRruleTime(rrule: string): string | null {
  const h = rrule.match(/BYHOUR=(\d+)/)
  const mi = rrule.match(/BYMINUTE=(\d+)/)
  if (!h) return null
  const hh = String(+h[1]).padStart(2, '0')
  const mm = String(mi ? +mi[1] : 0).padStart(2, '0')
  return `${hh}:${mm}`
}

/**
 * Render a pending mutation payload as a list of human-readable "field → value"
 * lines for the diff/summary preview. Falls back to pretty-printed JSON for
 * shapes we don't special-case.
 */
export function summarizeMutation(payload: unknown): string[] {
  if (payload == null || typeof payload !== 'object') {
    return [String(payload)]
  }
  const obj = payload as Record<string, unknown>

  // Experiment transitions carry a `transition` discriminator.
  if (typeof obj.transition === 'string') {
    return [`transition → ${obj.transition}`]
  }

  const lines: string[] = []
  for (const [k, v] of Object.entries(obj)) {
    lines.push(`${humanizeKey(k)} → ${formatValue(v)}`)
  }
  return lines.length ? lines : [JSON.stringify(payload, null, 2)]
}

function humanizeKey(k: string): string {
  switch (k) {
    case 'enabled_override':
      return 'enabled'
    case 'rollout_percentage':
      return 'rollout %'
    default:
      return k.replace(/_/g, ' ')
  }
}

function formatValue(v: unknown): string {
  if (v == null) return 'null'
  if (typeof v === 'boolean') return v ? 'on' : 'off'
  if (typeof v === 'object') return JSON.stringify(v)
  return String(v)
}
