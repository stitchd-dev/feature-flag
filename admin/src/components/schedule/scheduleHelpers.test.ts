/**
 * Unit tests for the schedule-builder pure helpers
 * (flag_lifecycle_20260604, Phase 8.2). Environment is `node`, so these test
 * the framework-free logic (no rendering).
 */
import { describe, it, expect } from 'vitest'
import type { ScheduledChange } from '../../lib/types'
import type { ScheduleFormValues } from '../../lib/validation/lifecycle'
import {
  availableActions,
  datetimeLocalToEpochMs,
  describeSchedule,
  formatInstant,
  parseRruleByday,
  parseRruleTime,
  statusGroup,
  summarizeMutation,
  toCreateBody,
  tzOffsetMs,
} from './scheduleHelpers'

function change(over: Partial<ScheduledChange> = {}): ScheduledChange {
  return {
    id: 's1',
    entity_type: 'flag',
    entity_id: 'f',
    env_id: 'e',
    mutation_payload: { enabled_override: false },
    schedule_kind: 'one_shot',
    scheduled_at_ms: 0,
    rrule: '',
    tz: '',
    status: 'pending',
    next_run_at_ms: 0,
    last_run_at_ms: 0,
    created_at_ms: 0,
    updated_at_ms: 0,
    version: 0,
    runs: [],
    ...over,
  }
}

describe('formatInstant', () => {
  it('renders a dash for 0 / null / missing', () => {
    expect(formatInstant(0)).toBe('—')
    expect(formatInstant(null)).toBe('—')
    expect(formatInstant(undefined)).toBe('—')
  })
  it('renders a real instant', () => {
    expect(formatInstant(Date.UTC(2026, 5, 10, 9, 0))).not.toBe('—')
  })
})

describe('statusGroup', () => {
  it('maps statuses to coarse groups', () => {
    expect(statusGroup('pending')).toBe('pending')
    expect(statusGroup('active')).toBe('active')
    expect(statusGroup('paused')).toBe('paused')
    expect(statusGroup('applied')).toBe('terminal')
    expect(statusGroup('failed')).toBe('terminal')
    expect(statusGroup('cancelled')).toBe('terminal')
  })
})

describe('availableActions', () => {
  it('offers cancel for a pending one-shot', () => {
    expect(availableActions(change({ schedule_kind: 'one_shot', status: 'pending' }))).toEqual([
      'cancel',
    ])
  })
  it('offers nothing for an applied one-shot', () => {
    expect(availableActions(change({ schedule_kind: 'one_shot', status: 'applied' }))).toEqual([])
  })
  it('offers pause for an active recurring, resume for a paused one', () => {
    expect(availableActions(change({ schedule_kind: 'recurring', status: 'active' }))).toEqual([
      'pause',
    ])
    expect(availableActions(change({ schedule_kind: 'recurring', status: 'paused' }))).toEqual([
      'resume',
    ])
  })
})

describe('parseRruleByday / parseRruleTime', () => {
  it('extracts BYDAY tokens', () => {
    expect(parseRruleByday('FREQ=WEEKLY;BYDAY=MO,FR;BYHOUR=9')).toEqual(['MO', 'FR'])
    expect(parseRruleByday('FREQ=WEEKLY')).toEqual([])
  })
  it('extracts HH:MM from BYHOUR/BYMINUTE', () => {
    expect(parseRruleTime('FREQ=WEEKLY;BYDAY=MO;BYHOUR=9;BYMINUTE=5')).toBe('09:05')
    expect(parseRruleTime('FREQ=WEEKLY;BYDAY=MO;BYHOUR=17')).toBe('17:00')
    expect(parseRruleTime('FREQ=WEEKLY;BYDAY=MO')).toBeNull()
  })
})

describe('describeSchedule', () => {
  it('describes a one-shot', () => {
    expect(describeSchedule(change({ schedule_kind: 'one_shot', scheduled_at_ms: 0 }))).toMatch(
      /^Once at/,
    )
  })
  it('describes a recurring window with days + time + tz', () => {
    const c = change({
      schedule_kind: 'recurring',
      rrule: 'FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=0',
      tz: 'America/New_York',
    })
    expect(describeSchedule(c)).toBe('MO, WE at 09:00 (America/New_York)')
  })
})

describe('tzOffsetMs', () => {
  it('reports +0 for UTC', () => {
    expect(tzOffsetMs(Date.UTC(2026, 0, 1, 12, 0), 'UTC')).toBe(0)
  })
  it('reports a positive offset east of UTC (Asia/Kolkata = +5:30)', () => {
    const off = tzOffsetMs(Date.UTC(2026, 0, 1, 12, 0), 'Asia/Kolkata')
    expect(off).toBe((5 * 60 + 30) * 60 * 1000)
  })
})

describe('datetimeLocalToEpochMs', () => {
  it('interprets a wall-clock value in the picked tz (UTC ⇒ identity)', () => {
    expect(datetimeLocalToEpochMs('2026-06-10T09:00', 'UTC')).toBe(Date.UTC(2026, 5, 10, 9, 0))
  })
  it('shifts by the tz offset (Asia/Kolkata 09:00 ⇒ 03:30 UTC)', () => {
    expect(datetimeLocalToEpochMs('2026-06-10T09:00', 'Asia/Kolkata')).toBe(
      Date.UTC(2026, 5, 10, 3, 30),
    )
  })
  it('returns NaN for malformed input', () => {
    expect(Number.isNaN(datetimeLocalToEpochMs('nope', 'UTC'))).toBe(true)
  })
})

describe('toCreateBody', () => {
  it('builds a one-shot body with the epoch instant', () => {
    const values: ScheduleFormValues = {
      schedule_kind: 'one_shot',
      scheduled_at: '2026-06-10T09:00',
      tz: 'UTC',
      weekdays: [],
      hour: 0,
      minute: 0,
      mutation_payload: '{"enabled_override":false}',
    }
    const body = toCreateBody(values)
    expect(body.schedule_kind).toBe('one_shot')
    expect(body.scheduled_at_ms).toBe(Date.UTC(2026, 5, 10, 9, 0))
    expect(body.mutation_payload).toEqual({ enabled_override: false })
  })

  it('builds a recurring body with an RRULE + tz', () => {
    const values: ScheduleFormValues = {
      schedule_kind: 'recurring',
      scheduled_at: '',
      tz: 'Europe/Berlin',
      weekdays: ['MO', 'WE', 'FR'],
      hour: 9,
      minute: 30,
      mutation_payload: '{"enabled_override":true}',
    }
    const body = toCreateBody(values)
    expect(body.schedule_kind).toBe('recurring')
    expect(body.tz).toBe('Europe/Berlin')
    expect(body.rrule).toBe('FREQ=WEEKLY;BYDAY=MO,WE,FR;BYHOUR=9;BYMINUTE=30;BYSECOND=0')
    expect(body.mutation_payload).toEqual({ enabled_override: true })
  })
})

describe('summarizeMutation', () => {
  it('humanizes a flag enable/disable payload', () => {
    expect(summarizeMutation({ enabled_override: false })).toEqual(['enabled → off'])
    expect(summarizeMutation({ enabled_override: true })).toEqual(['enabled → on'])
  })
  it('summarizes an experiment transition', () => {
    expect(summarizeMutation({ transition: 'start' })).toEqual(['transition → start'])
  })
  it('lists multiple fields', () => {
    expect(summarizeMutation({ default_variant_key: 'v2', rollout_percentage: 25 })).toEqual([
      'default variant key → v2',
      'rollout % → 25',
    ])
  })
})
