import { useCallback, useEffect, useState } from 'react'
import { Formik, Form, useFormikContext } from 'formik'
import type { LifecycleEntityKind, ScheduledChange } from '../../lib/types'
import {
  listSchedules,
  createSchedule,
  cancelSchedule,
  pauseSchedule,
  resumeSchedule,
} from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'
import {
  scheduleSchema,
  ianaTimezones,
  guessLocalTimezone,
  WEEKDAYS,
} from '../../lib/validation/lifecycle'
import type { ScheduleFormValues, WeekdayToken } from '../../lib/validation/lifecycle'
import { FormSelect } from '../form/FormSelect'
import { FormField } from '../form/FormField'
import { FormErrorBanner } from '../form/FormErrorBanner'
import { ConfirmDialog } from '../ConfirmDialog'
import {
  availableActions,
  describeSchedule,
  formatInstant,
  statusGroup,
  summarizeMutation,
  toCreateBody,
} from './scheduleHelpers'

/** A named mutation/transition preset the entity page supplies. */
export interface MutationPreset {
  /** Stable id (used for the <select>). */
  id: string
  /** Human label shown in the picker. */
  label: string
  /** The JSON mutation payload this preset applies (object). */
  payload: unknown
}

export interface ScheduleBuilderProps {
  /** Environment the schedule is scoped to. */
  envId: string
  /** `flags` | `segments` | `experiments`. */
  entityKind: LifecycleEntityKind
  /** The targeted entity id (flag key / segment id / experiment id). */
  entityId: string
  /**
   * Entity-appropriate mutation/transition presets. The first is the default.
   * When omitted, the builder shows a free-form JSON editor only.
   */
  presets?: MutationPreset[]
  /** Allow editing the raw JSON in addition to the presets (default true). */
  allowRawJson?: boolean
}

// ─── Mutation/transition editor (inside Formik) ──────────────────────────────

function MutationEditor({
  presets,
  allowRawJson,
}: {
  presets: MutationPreset[]
  allowRawJson: boolean
}) {
  const { values, setFieldValue, errors, touched } = useFormikContext<ScheduleFormValues>()
  const [mode, setMode] = useState<'preset' | 'json'>(presets.length > 0 ? 'preset' : 'json')
  const [presetId, setPresetId] = useState<string>(presets[0]?.id ?? '')

  const err = touched.mutation_payload && errors.mutation_payload

  function applyPreset(id: string) {
    setPresetId(id)
    const p = presets.find((x) => x.id === id)
    if (p) void setFieldValue('mutation_payload', JSON.stringify(p.payload, null, 2))
  }

  return (
    <div>
      <div className="label" style={{ marginBottom: 4 }}>
        {label(values)}
      </div>
      {presets.length > 0 && allowRawJson && (
        <div style={{ display: 'flex', gap: 6, marginBottom: 8 }}>
          <button
            type="button"
            className={`btn sm ${mode === 'preset' ? 'primary' : ''}`}
            onClick={() => setMode('preset')}
          >
            Preset
          </button>
          <button
            type="button"
            className={`btn sm ${mode === 'json' ? 'primary' : ''}`}
            onClick={() => setMode('json')}
          >
            JSON
          </button>
        </div>
      )}

      {mode === 'preset' && presets.length > 0 ? (
        <select
          className="input"
          style={{ width: '100%' }}
          value={presetId}
          onChange={(e) => applyPreset(e.target.value)}
        >
          {presets.map((p) => (
            <option key={p.id} value={p.id}>
              {p.label}
            </option>
          ))}
        </select>
      ) : (
        <textarea
          className="input"
          style={{ width: '100%', minHeight: 100, fontFamily: 'monospace', fontSize: 12 }}
          value={values.mutation_payload}
          spellCheck={false}
          onChange={(e) => void setFieldValue('mutation_payload', e.target.value)}
        />
      )}
      {err && (
        <div role="alert" style={{ fontSize: 12, color: 'var(--danger)', marginTop: 4 }}>
          {err}
        </div>
      )}
    </div>
  )
}

function label(values: ScheduleFormValues): string {
  return values.schedule_kind === 'recurring'
    ? 'Recurring change to apply'
    : 'Change to apply at fire time'
}

// ─── Weekday picker (recurring) ──────────────────────────────────────────────

function WeekdayPicker() {
  const { values, setFieldValue, errors, touched } = useFormikContext<ScheduleFormValues>()
  const selected = (values.weekdays ?? []) as WeekdayToken[]
  const err = touched.weekdays && (errors.weekdays as string | undefined)

  function toggle(token: WeekdayToken) {
    const next = selected.includes(token)
      ? selected.filter((d) => d !== token)
      : [...selected, token]
    void setFieldValue('weekdays', next)
  }

  return (
    <div>
      <div className="label" style={{ marginBottom: 4 }}>
        Weekdays
      </div>
      <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
        {WEEKDAYS.map((d) => (
          <button
            key={d.token}
            type="button"
            className={`btn sm ${selected.includes(d.token) ? 'primary' : ''}`}
            onClick={() => toggle(d.token)}
            aria-pressed={selected.includes(d.token)}
          >
            {d.label}
          </button>
        ))}
      </div>
      {err && (
        <div role="alert" style={{ fontSize: 12, color: 'var(--danger)', marginTop: 4 }}>
          {err}
        </div>
      )}
    </div>
  )
}

// ─── Create form ─────────────────────────────────────────────────────────────

function ScheduleFields() {
  const { values } = useFormikContext<ScheduleFormValues>()
  const tzOptions = ianaTimezones().map((tz) => ({ value: tz, label: tz }))

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
      <FormSelect
        name="schedule_kind"
        label="Schedule"
        options={[
          { value: 'one_shot', label: 'One-shot (a single time)' },
          { value: 'recurring', label: 'Recurring (weekly window)' },
        ]}
      />

      {values.schedule_kind === 'one_shot' ? (
        <FormField
          name="scheduled_at"
          label="Fires at"
          type="datetime-local"
          hint="Interpreted in the timezone below."
        />
      ) : (
        <>
          <WeekdayPicker />
          <div style={{ display: 'flex', gap: 10 }}>
            <div style={{ flex: 1 }}>
              <FormField name="hour" label="Hour (0–23)" type="number" min={0} max={23} />
            </div>
            <div style={{ flex: 1 }}>
              <FormField name="minute" label="Minute (0–59)" type="number" min={0} max={59} />
            </div>
          </div>
        </>
      )}

      <FormSelect
        name="tz"
        label="Timezone (IANA)"
        options={tzOptions}
        hint="DST-aware; the recurrence is evaluated in this zone."
      />
    </div>
  )
}

// ─── Existing schedules list ─────────────────────────────────────────────────

function statusPill(change: ScheduledChange) {
  const g = statusGroup(change.status)
  const color =
    g === 'active' || change.status === 'applied'
      ? 'var(--success, #22c55e)'
      : g === 'paused' || change.status === 'pending'
        ? 'var(--warning, #d97706)'
        : change.status === 'failed'
          ? 'var(--danger)'
          : 'var(--fg-muted)'
  return (
    <span
      style={{
        fontSize: 11,
        padding: '1px 8px',
        borderRadius: 10,
        fontWeight: 600,
        color: '#fff',
        background: color,
      }}
    >
      {change.status}
    </span>
  )
}

function ScheduleRow({
  change,
  onAction,
  busy,
}: {
  change: ScheduledChange
  onAction: (action: 'cancel' | 'pause' | 'resume', change: ScheduledChange) => void
  busy: boolean
}) {
  const actions = availableActions(change)
  const lastRun = change.runs[0]

  return (
    <div
      style={{
        border: '1px solid var(--border)',
        borderRadius: 6,
        padding: '10px 12px',
        display: 'flex',
        flexDirection: 'column',
        gap: 6,
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        {statusPill(change)}
        <span style={{ fontSize: 12, fontWeight: 600 }}>{describeSchedule(change)}</span>
        <span style={{ flex: 1 }} />
        {actions.map((a) => (
          <button
            key={a}
            className={`btn sm ${a === 'cancel' ? 'danger' : ''}`}
            disabled={busy}
            onClick={() => onAction(a, change)}
          >
            {a}
          </button>
        ))}
      </div>

      {/* Diff / summary preview of the pending mutation. */}
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
        {summarizeMutation(change.mutation_payload).map((line, i) => (
          <code
            key={i}
            style={{
              fontSize: 11,
              padding: '2px 7px',
              borderRadius: 4,
              background: 'var(--bg-sunken)',
              border: '1px solid var(--border-faint)',
            }}
          >
            {line}
          </code>
        ))}
      </div>

      <div style={{ display: 'flex', gap: 16, fontSize: 11, color: 'var(--fg-muted)' }}>
        <span>next run: {formatInstant(change.next_run_at_ms)}</span>
        <span>last run: {formatInstant(change.last_run_at_ms)}</span>
        {lastRun && (
          <span>
            last outcome: <strong>{lastRun.outcome}</strong>
            {lastRun.detail ? ` — ${lastRun.detail}` : ''}
          </span>
        )}
      </div>
    </div>
  )
}

// ─── Builder ─────────────────────────────────────────────────────────────────

export function ScheduleBuilder({
  envId,
  entityKind,
  entityId,
  presets = [],
  allowRawJson = true,
}: ScheduleBuilderProps) {
  const [schedules, setSchedules] = useState<ScheduledChange[]>([])
  const [loading, setLoading] = useState(false)
  const [listError, setListError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [pendingAction, setPendingAction] = useState<{
    action: 'cancel' | 'pause' | 'resume'
    change: ScheduledChange
  } | null>(null)

  const refresh = useCallback(
    async (signal?: AbortSignal) => {
      if (!envId) return
      setLoading(true)
      setListError(null)
      try {
        const data = await listSchedules(envId, entityKind, entityId, signal)
        setSchedules(data)
      } catch (err) {
        if (!signal?.aborted) setListError(extractErrorMessage(err))
      } finally {
        if (!signal?.aborted) setLoading(false)
      }
    },
    [envId, entityKind, entityId],
  )

  useEffect(() => {
    const controller = new AbortController()
    void refresh(controller.signal)
    return () => controller.abort()
  }, [refresh])

  const initial: ScheduleFormValues = {
    schedule_kind: 'one_shot',
    scheduled_at: '',
    tz: guessLocalTimezone(),
    weekdays: [],
    hour: 9,
    minute: 0,
    mutation_payload: presets[0]
      ? JSON.stringify(presets[0].payload, null, 2)
      : '{\n  "enabled_override": false\n}',
  }

  async function handleCreate(
    values: ScheduleFormValues,
    helpers: { setStatus: (s: unknown) => void; resetForm: () => void; setSubmitting: (b: boolean) => void },
  ) {
    helpers.setStatus(undefined)
    try {
      await createSchedule(envId, entityKind, entityId, toCreateBody(values))
      helpers.resetForm()
      await refresh()
    } catch (err) {
      helpers.setStatus({ error: extractErrorMessage(err) })
    } finally {
      helpers.setSubmitting(false)
    }
  }

  async function runAction(action: 'cancel' | 'pause' | 'resume', change: ScheduledChange) {
    setBusy(true)
    setListError(null)
    try {
      if (action === 'cancel') await cancelSchedule(envId, change.id, change.version)
      else if (action === 'pause') await pauseSchedule(envId, change.id, change.version)
      else await resumeSchedule(envId, change.id, change.version)
      await refresh()
    } catch (err) {
      setListError(extractErrorMessage(err))
    } finally {
      setBusy(false)
      setPendingAction(null)
    }
  }

  const pending = schedules.filter(
    (s) => statusGroup(s.status) !== 'terminal',
  )
  const history = schedules.filter((s) => statusGroup(s.status) === 'terminal')

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
      {/* ── Create panel ── */}
      <div className="card">
        <div className="card-header">
          <div className="card-title">Schedule a change</div>
        </div>
        <div style={{ padding: '12px 16px' }}>
          <Formik
            initialValues={initial}
            validationSchema={scheduleSchema}
            onSubmit={handleCreate}
          >
            {() => (
              <Form id="schedule-create-form" style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
                <FormErrorBanner />
                <ScheduleFields />
                <MutationEditor presets={presets} allowRawJson={allowRawJson} />
                <div>
                  <SubmitButton />
                </div>
              </Form>
            )}
          </Formik>
        </div>
      </div>

      {/* ── Existing schedules ── */}
      <div className="card">
        <div className="card-header">
          <div className="card-title">Scheduled changes</div>
          <button className="btn sm" onClick={() => void refresh()} disabled={loading}>
            {loading ? 'Refreshing…' : 'Refresh'}
          </button>
        </div>
        <div style={{ padding: '12px 16px', display: 'flex', flexDirection: 'column', gap: 10 }}>
          {listError && (
            <div role="alert" style={{ fontSize: 13, color: 'var(--danger)' }}>
              {listError}
            </div>
          )}
          {!loading && schedules.length === 0 && !listError && (
            <div style={{ fontSize: 13, color: 'var(--fg-muted)' }}>
              No scheduled changes for this {entityKind.replace(/s$/, '')} yet.
            </div>
          )}
          {pending.map((c) => (
            <ScheduleRow key={c.id} change={c} onAction={(a, ch) => setPendingAction({ action: a, change: ch })} busy={busy} />
          ))}
          {history.length > 0 && (
            <>
              <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 4 }}>History</div>
              {history.map((c) => (
                <ScheduleRow key={c.id} change={c} onAction={() => undefined} busy={busy} />
              ))}
            </>
          )}
        </div>
      </div>

      {pendingAction && (
        <ConfirmDialog
          title={`${cap(pendingAction.action)} schedule?`}
          message={`${cap(pendingAction.action)} "${describeSchedule(pendingAction.change)}"? ${
            pendingAction.action === 'cancel'
              ? 'This cannot be undone.'
              : pendingAction.action === 'pause'
                ? 'It will stop firing until resumed.'
                : 'It will resume firing on its next window.'
          }`}
          confirmLabel={cap(pendingAction.action)}
          confirmDanger={pendingAction.action === 'cancel'}
          onConfirm={() => void runAction(pendingAction.action, pendingAction.change)}
          onCancel={() => setPendingAction(null)}
        />
      )}
    </div>
  )
}

function SubmitButton() {
  const { isSubmitting } = useFormikContext()
  return (
    <button type="submit" className="btn primary" disabled={isSubmitting}>
      {isSubmitting ? 'Scheduling…' : 'Schedule change'}
    </button>
  )
}

function cap(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1)
}
