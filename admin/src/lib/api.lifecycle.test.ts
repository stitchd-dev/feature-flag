/**
 * Unit tests for the flag-lifecycle API wrappers (schedules, prerequisites,
 * dependency graph) in `admin/src/lib/api.ts` and the Yup schemas in
 * `admin/src/lib/validation/lifecycle.ts` (flag_lifecycle_20260604, Phase 8.1).
 *
 * Mirrors `api.experiments.test.ts`: `axios.create` is mocked before importing
 * `api.ts` so the wrappers record calls into a shared spy client.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'

interface SpyClient {
  get: ReturnType<typeof vi.fn>
  post: ReturnType<typeof vi.fn>
  put: ReturnType<typeof vi.fn>
  patch: ReturnType<typeof vi.fn>
  delete: ReturnType<typeof vi.fn>
  interceptors: { request: { use: () => void }; response: { use: () => void } }
  defaults: { baseURL: string }
}

const spyClient: SpyClient = {
  get: vi.fn(),
  post: vi.fn(),
  put: vi.fn(),
  patch: vi.fn(),
  delete: vi.fn(),
  interceptors: { request: { use: () => undefined }, response: { use: () => undefined } },
  defaults: { baseURL: '/api' },
}

vi.mock('axios', () => ({ default: { create: () => spyClient, post: vi.fn() } }))

const {
  listSchedules,
  getSchedule,
  createSchedule,
  cancelSchedule,
  pauseSchedule,
  resumeSchedule,
  getPrerequisites,
  setPrerequisites,
  getDependencies,
} = await import('./api')

import {
  scheduleSchema,
  prerequisitesSchema,
  buildWeeklyRrule,
  ianaTimezones,
  guessLocalTimezone,
} from './validation/lifecycle'

beforeEach(() => {
  spyClient.get.mockReset()
  spyClient.post.mockReset()
  spyClient.put.mockReset()
  spyClient.patch.mockReset()
  spyClient.delete.mockReset()
})

// ─── Schedules ────────────────────────────────────────────────────────────────

describe('schedule API wrappers', () => {
  it('listSchedules GETs the env-scoped entity URL', async () => {
    spyClient.get.mockResolvedValueOnce({ data: [] })
    const out = await listSchedules('env-1', 'flags', 'flag-key')
    expect(spyClient.get.mock.calls[0][0]).toBe('/v1/environments/env-1/flags/flag-key/schedules')
    expect(out).toEqual([])
  })

  it('getSchedule GETs the env-scoped schedule URL', async () => {
    const fixture = { id: 's1', runs: [] }
    spyClient.get.mockResolvedValueOnce({ data: fixture })
    const out = await getSchedule('env-1', 's1')
    expect(spyClient.get.mock.calls[0][0]).toBe('/v1/environments/env-1/schedules/s1')
    expect(out).toEqual(fixture)
  })

  it('createSchedule POSTs the create body to the entity URL', async () => {
    spyClient.post.mockResolvedValueOnce({ data: { id: 's2' } })
    const body = {
      mutation_payload: { enabled_override: false },
      schedule_kind: 'one_shot' as const,
      scheduled_at_ms: 123,
      tz: 'UTC',
    }
    await createSchedule('env-1', 'experiments', 'exp-1', body)
    expect(spyClient.post.mock.calls[0][0]).toBe(
      '/v1/environments/env-1/experiments/exp-1/schedules',
    )
    expect(spyClient.post.mock.calls[0][1]).toEqual(body)
  })

  it('cancel/pause/resume POST the version body to the lifecycle URLs', async () => {
    spyClient.post.mockResolvedValue({ data: { id: 's1' } })
    await cancelSchedule('env-1', 's1', 3)
    await pauseSchedule('env-1', 's1', 4)
    await resumeSchedule('env-1', 's1', 5)
    expect(spyClient.post.mock.calls[0]).toEqual([
      '/v1/environments/env-1/schedules/s1/cancel',
      { version: 3 },
    ])
    expect(spyClient.post.mock.calls[1]).toEqual([
      '/v1/environments/env-1/schedules/s1/pause',
      { version: 4 },
    ])
    expect(spyClient.post.mock.calls[2]).toEqual([
      '/v1/environments/env-1/schedules/s1/resume',
      { version: 5 },
    ])
  })

  it('propagates axios errors', async () => {
    spyClient.get.mockRejectedValueOnce({ response: { status: 502, data: { error: 'down' } } })
    await expect(listSchedules('env-1', 'flags', 'f')).rejects.toMatchObject({
      response: { status: 502 },
    })
  })
})

// ─── Prerequisites ──────────────────────────────────────────────────────────

describe('prerequisite API wrappers', () => {
  it('getPrerequisites GETs the project-scoped flag URL', async () => {
    spyClient.get.mockResolvedValueOnce({ data: { prerequisites: [], fallback_variant_key: '' } })
    const out = await getPrerequisites('proj-1', 'flag-key')
    expect(spyClient.get.mock.calls[0][0]).toBe('/v1/projects/proj-1/flags/flag-key/prerequisites')
    expect(out.fallback_variant_key).toBe('')
  })

  it('setPrerequisites PUTs the gate body', async () => {
    spyClient.put.mockResolvedValueOnce({ data: { prerequisites: [], fallback_variant_key: 'off' } })
    const body = {
      prerequisites: [{ prerequisite_flag_key: 'a', required_variant_key: 'on' }],
      fallback_variant_key: 'off',
      version: 7,
    }
    await setPrerequisites('proj-1', 'flag-key', body)
    expect(spyClient.put.mock.calls[0][0]).toBe('/v1/projects/proj-1/flags/flag-key/prerequisites')
    expect(spyClient.put.mock.calls[0][1]).toEqual(body)
  })

  it('surfaces a 400 cycle response to the caller', async () => {
    spyClient.put.mockRejectedValueOnce({
      response: { status: 400, data: { error: 'prerequisite cycle detected: a -> b -> a' } },
    })
    await expect(
      setPrerequisites('proj-1', 'flag-key', {
        prerequisites: [],
        fallback_variant_key: '',
        version: 1,
      }),
    ).rejects.toMatchObject({ response: { status: 400 } })
  })
})

// ─── Dependency graph ─────────────────────────────────────────────────────────

describe('getDependencies', () => {
  it('GETs the project-scoped dependencies URL', async () => {
    spyClient.get.mockResolvedValueOnce({
      data: { entity_kind: 'flag', entity_id: 'f', upstream: [], downstream: [] },
    })
    const out = await getDependencies('proj-1', 'flags', 'f')
    expect(spyClient.get.mock.calls[0][0]).toBe('/v1/projects/proj-1/flags/f/dependencies')
    expect(out.entity_kind).toBe('flag')
  })
})

// ─── Schemas + helpers ────────────────────────────────────────────────────────

describe('buildWeeklyRrule', () => {
  it('emits a WEEKLY RRULE with BYDAY/BYHOUR/BYMINUTE', () => {
    expect(buildWeeklyRrule(['MO', 'WE', 'FR'], 9, 30)).toBe(
      'FREQ=WEEKLY;BYDAY=MO,WE,FR;BYHOUR=9;BYMINUTE=30;BYSECOND=0',
    )
  })
})

describe('ianaTimezones / guessLocalTimezone', () => {
  it('returns a non-empty timezone list including UTC', () => {
    const tzs = ianaTimezones()
    expect(tzs.length).toBeGreaterThan(0)
    expect(tzs).toContain('UTC')
  })
  it('guesses a local timezone string', () => {
    expect(typeof guessLocalTimezone()).toBe('string')
    expect(guessLocalTimezone().length).toBeGreaterThan(0)
  })
})

describe('scheduleSchema', () => {
  const base = { tz: 'UTC', mutation_payload: '{"enabled_override":false}' }

  it('accepts a valid one-shot', async () => {
    await expect(
      scheduleSchema.validate({ ...base, schedule_kind: 'one_shot', scheduled_at: '2026-06-10T09:00' }),
    ).resolves.toBeTruthy()
  })

  it('rejects a one-shot missing the datetime', async () => {
    await expect(
      scheduleSchema.validate({ ...base, schedule_kind: 'one_shot', scheduled_at: '' }),
    ).rejects.toThrow(/date and time/i)
  })

  it('accepts a valid recurring weekly window', async () => {
    await expect(
      scheduleSchema.validate({
        ...base,
        schedule_kind: 'recurring',
        weekdays: ['MO', 'FR'],
        hour: 9,
        minute: 0,
      }),
    ).resolves.toBeTruthy()
  })

  it('rejects a recurring window with no weekdays', async () => {
    await expect(
      scheduleSchema.validate({
        ...base,
        schedule_kind: 'recurring',
        weekdays: [],
        hour: 9,
        minute: 0,
      }),
    ).rejects.toThrow(/at least one weekday/i)
  })

  it('rejects an out-of-range hour', async () => {
    await expect(
      scheduleSchema.validate({
        ...base,
        schedule_kind: 'recurring',
        weekdays: ['MO'],
        hour: 24,
        minute: 0,
      }),
    ).rejects.toThrow(/hour/i)
  })

  it('rejects an invalid mutation-payload JSON', async () => {
    await expect(
      scheduleSchema.validate({
        schedule_kind: 'one_shot',
        scheduled_at: '2026-06-10T09:00',
        tz: 'UTC',
        mutation_payload: '{not json',
      }),
    ).rejects.toThrow(/valid JSON/i)
  })
})

describe('prerequisitesSchema', () => {
  it('accepts well-formed prerequisite rows', async () => {
    await expect(
      prerequisitesSchema.validate({
        prerequisites: [{ prerequisite_flag_key: 'a', required_variant_key: 'on' }],
        fallback_variant_key: 'off',
      }),
    ).resolves.toBeTruthy()
  })

  it('rejects a row missing the required variant', async () => {
    await expect(
      prerequisitesSchema.validate({
        prerequisites: [{ prerequisite_flag_key: 'a', required_variant_key: '' }],
        fallback_variant_key: '',
      }),
    ).rejects.toThrow(/required variant/i)
  })

  it('rejects duplicate prerequisite flags', async () => {
    await expect(
      prerequisitesSchema.validate({
        prerequisites: [
          { prerequisite_flag_key: 'a', required_variant_key: 'on' },
          { prerequisite_flag_key: 'a', required_variant_key: 'off' },
        ],
        fallback_variant_key: '',
      }),
    ).rejects.toThrow(/twice/i)
  })

  it('accepts an empty prerequisite set (clears the gate)', async () => {
    await expect(
      prerequisitesSchema.validate({ prerequisites: [], fallback_variant_key: '' }),
    ).resolves.toBeTruthy()
  })
})
