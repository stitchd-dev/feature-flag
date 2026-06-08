import * as Yup from 'yup'

// ── Flag-lifecycle form schemas (flag_lifecycle_20260604, Phase 8) ────────────
//
// Yup schemas for the schedule builder + prerequisites editor. Backed by the
// gateway contracts in `crates/stitchd-gateway/src/routes/{schedules,flags}.rs`.

export const SCHEDULE_KINDS = ['one_shot', 'recurring'] as const
export type ScheduleKindValue = (typeof SCHEDULE_KINDS)[number]

/** Weekday tokens used by the recurring weekly-window builder → RRULE BYDAY. */
export const WEEKDAYS = [
  { token: 'MO', label: 'Mon' },
  { token: 'TU', label: 'Tue' },
  { token: 'WE', label: 'Wed' },
  { token: 'TH', label: 'Thu' },
  { token: 'FR', label: 'Fri' },
  { token: 'SA', label: 'Sat' },
  { token: 'SU', label: 'Sun' },
] as const

export type WeekdayToken = (typeof WEEKDAYS)[number]['token']

/**
 * The list of IANA timezones for the tz picker. Uses the runtime
 * `Intl.supportedValuesOf('timeZone')` when available (all evergreen
 * browsers), falling back to a small common set otherwise.
 */
export function ianaTimezones(): string[] {
  const intl = Intl as unknown as { supportedValuesOf?: (k: string) => string[] }
  if (typeof intl.supportedValuesOf === 'function') {
    try {
      const zones = intl.supportedValuesOf('timeZone')
      // `supportedValuesOf` yields `Etc/UTC` rather than the canonical `UTC`;
      // surface a bare `UTC` first so it's the obvious default in the picker.
      return zones.includes('UTC') ? zones : ['UTC', ...zones]
    } catch {
      /* fall through */
    }
  }
  return [
    'UTC',
    'America/New_York',
    'America/Chicago',
    'America/Denver',
    'America/Los_Angeles',
    'Europe/London',
    'Europe/Paris',
    'Europe/Berlin',
    'Asia/Kolkata',
    'Asia/Tokyo',
    'Australia/Sydney',
  ]
}

/** The caller's best-guess local IANA timezone (for the picker default). */
export function guessLocalTimezone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
  } catch {
    return 'UTC'
  }
}

/**
 * Build an RFC-5545 RRULE for a weekly window: WEEKLY recurrence on the chosen
 * weekdays at `hour:minute`. The IANA tz is carried separately (the gateway's
 * `tz` field), so the RRULE itself is tz-naive (BYHOUR/BYMINUTE are evaluated
 * in `tz` server-side).
 */
export function buildWeeklyRrule(days: WeekdayToken[], hour: number, minute: number): string {
  const byday = days.join(',')
  return `FREQ=WEEKLY;BYDAY=${byday};BYHOUR=${hour};BYMINUTE=${minute};BYSECOND=0`
}

/**
 * Schedule-builder form. Discriminated by `schedule_kind`:
 *   - `one_shot`   ⇒ `scheduled_at` (a datetime-local string) is required.
 *   - `recurring`  ⇒ `weekdays` (≥1), `hour`, `minute`, and `tz` are required.
 * `mutation_payload` is a JSON string the entity-specific editor produces; it
 * must parse as JSON.
 */
export const scheduleSchema = Yup.object({
  schedule_kind: Yup.string()
    .oneOf(SCHEDULE_KINDS as unknown as string[], 'Invalid schedule kind')
    .required('Schedule kind is required'),

  // One-shot: an HTML datetime-local value (interpreted in `tz`).
  scheduled_at: Yup.string().when('schedule_kind', {
    is: 'one_shot',
    then: (s) => s.trim().min(1, 'Pick a date and time').required('Pick a date and time'),
    otherwise: (s) => s.optional(),
  }),

  // Timezone is required for both kinds (one-shot interprets `scheduled_at` in it).
  tz: Yup.string().trim().min(1, 'Timezone is required').required('Timezone is required'),

  // Recurring: weekly window.
  weekdays: Yup.array()
    .of(Yup.string().oneOf(WEEKDAYS.map((d) => d.token) as unknown as string[]))
    .when('schedule_kind', {
      is: 'recurring',
      then: (s) => s.min(1, 'Pick at least one weekday').required('Pick at least one weekday'),
      otherwise: (s) => s.optional(),
    }),
  hour: Yup.number()
    .when('schedule_kind', {
      is: 'recurring',
      then: (s) =>
        s
          .typeError('Hour must be a number')
          .min(0, 'Hour 0–23')
          .max(23, 'Hour 0–23')
          .required('Hour is required'),
      otherwise: (s) => s.optional(),
    }),
  minute: Yup.number()
    .when('schedule_kind', {
      is: 'recurring',
      then: (s) =>
        s
          .typeError('Minute must be a number')
          .min(0, 'Minute 0–59')
          .max(59, 'Minute 0–59')
          .required('Minute is required'),
      otherwise: (s) => s.optional(),
    }),

  // Entity-specific mutation, edited as JSON text. Must parse.
  mutation_payload: Yup.string()
    .trim()
    .min(1, 'A mutation payload is required')
    .required('A mutation payload is required')
    .test('is-json', 'Mutation payload must be valid JSON', (v) => {
      if (v == null || v === '') return false
      try {
        JSON.parse(v)
        return true
      } catch {
        return false
      }
    }),
}).defined()

export type ScheduleFormValues = Yup.InferType<typeof scheduleSchema>

/**
 * Prerequisites-editor form. Each row pairs a prerequisite flag key with the
 * required variant key; both are required and the prerequisite flag may not be
 * the flag being edited (a self-edge is always a cycle). `fallback_variant_key`
 * empty ⇒ the flag's off/disabled variant.
 */
export const prerequisitesSchema = Yup.object({
  prerequisites: Yup.array()
    .of(
      Yup.object({
        prerequisite_flag_key: Yup.string()
          .trim()
          .min(1, 'Pick a prerequisite flag')
          .required('Pick a prerequisite flag'),
        required_variant_key: Yup.string()
          .trim()
          .min(1, 'Pick the required variant')
          .required('Pick the required variant'),
      }),
    )
    .required()
    .test('no-duplicate-prereq', 'A flag may not be listed as a prerequisite twice', (rows) => {
      if (!rows) return true
      const keys = rows.map((r) => r.prerequisite_flag_key).filter(Boolean)
      return new Set(keys).size === keys.length
    }),
  fallback_variant_key: Yup.string().defined(),
}).defined()

export type PrerequisitesFormValues = Yup.InferType<typeof prerequisitesSchema>
