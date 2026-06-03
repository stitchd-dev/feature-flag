/**
 * Phase 10 Create / Edit experiment modal.
 *
 * Rewrites the Phase 7 modal to bind an experiment to either a specific
 * percentage-rollout rule on a flag OR to the flag's default-rule
 * percentage distribution (XOR). Surfaces the Phase 1 attribution fields
 * (`unit_context_types`, `pre_period_days`, primary + guardrail metric_ids,
 * `traffic_allocation`) and consumes the typed Phase 8 API wrappers.
 *
 * Form architecture:
 *
 *   - Formik with `validateOnChange={false}` (track learning — async/expensive
 *     validators on the schema would hammer the gateway on every keystroke).
 *   - `enableReinitialize` so edit mode picks up the async-loaded experiment
 *     after the GET resolves.
 *   - Yup schema in `lib/validation/experiment.ts`. Pure helpers in
 *     `./CreateExperimentModal.helpers.ts`.
 */
import { useEffect, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { Formik, Form, useFormikContext, useField } from 'formik'
import { I } from '../../components/icons'
import { Modal } from '../../components/Modal'
import { FormField } from '../../components/form/FormField'
import { FormCheckbox } from '../../components/form/FormCheckbox'
import { FormSelect } from '../../components/form/FormSelect'
import { FormTextarea } from '../../components/form/FormTextarea'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { FormSubmit } from '../../components/form/FormSubmit'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'
import { slugify } from '../../lib/utils'
import { extractErrorMessage } from '../../lib/errors'
import {
  listExclusionGroups,
  getExclusionGroup,
  assignExperimentToGroup,
  type ExclusionGroup,
} from '../../lib/api/exclusionGroups'
import {
  exclusionGroupFitsCapacity,
  formatBpPercent,
} from './exclusionGroups/capacity'
import {
  experimentSchema,
  MAX_METRIC_IDS,
  MAX_GUARDRAIL_METRIC_IDS,
  type ExperimentFormValues,
} from '../../lib/validation/experiment'
import type {
  AdminFlagResponse,
  RolloutDistribution,
} from '../../lib/types'
import {
  buildExperimentCreateBody,
  buildExperimentPatchBody,
  filterEligibleRules,
  filterMetricOptions,
  hasDefaultRuleDistribution,
  metricKindChipStyle,
  type MetricPickerOption,
  type RulePickerOption,
} from './CreateExperimentModal.helpers'

const MODEL_OPTIONS = [
  { value: 'bayesian', label: 'Bayesian' },
  { value: 'frequentist', label: 'Frequentist' },
]

/**
 * Per-flag shape consumed by this modal. Adds the Phase 1
 * `default_rule_distribution` field which the gateway emits but which is
 * scoped to specific endpoints — keep it optional so the typing degrades
 * gracefully when omitted. `locked_by_experiment_id` lives on
 * `AdminFlagResponse` (feature-flag-1p6) and is inherited via `extends`.
 */
interface ExpFlagShape extends AdminFlagResponse {
  /** Phase 1 default-rule percentage distribution; `null` when only the
   * single-variant default is configured. */
  default_rule_distribution?: RolloutDistribution | null
}

/** Per-rule shape consumed by the rule picker. The gateway surfaces
 *  `feature_flag_rules.id` as `rule_id` and the rule's optional UI label as
 *  `name` (feature-flag-1p6); both fields are required reads, no synthesised
 *  fallbacks. */
interface RuleWithId {
  rule_id: string
  name?: string | null
  output: unknown
}

interface MetricsListResponse {
  items: {
    id: string
    key: string
    name: string
    kind: 'aggregation' | 'ratio' | 'funnel'
  }[]
  total: number
}

interface FlagListResponse {
  items: ExpFlagShape[]
  total: number
}

// ── Auto-slug from name ───────────────────────────────────────────────────────

function AutoSlugKey({ disabled }: { disabled: boolean }) {
  const { values, setFieldValue } = useFormikContext<ExperimentFormValues>()
  const keyEditedRef = useRef(false)
  const prevNameRef = useRef('')

  useEffect(() => {
    if (disabled) return
    if (!keyEditedRef.current && values.name !== prevNameRef.current) {
      prevNameRef.current = values.name
      void setFieldValue('key', slugify(values.name), false)
    }
  }, [values.name, setFieldValue, disabled])

  return null
}

// ── Flag picker ─────────────────────────────────────────────────────────────

function FlagPicker({
  flags,
  onSelected,
  disabled,
}: {
  flags: ExpFlagShape[]
  onSelected: (flag: ExpFlagShape | null) => void
  disabled: boolean
}) {
  const [{ value }, meta, helpers] = useField<string>('flag_id')
  const isError = meta.touched && Boolean(meta.error)
  const errorMessage = isError ? meta.error : undefined

  function setFlag(flagId: string) {
    void helpers.setValue(flagId)
    const flag = flags.find((f) => f.flag_id === flagId) ?? null
    onSelected(flag)
  }

  return (
    <div>
      <label className="label" htmlFor="flag_id" style={{ display: 'block', marginBottom: 4 }}>
        Flag
      </label>
      <select
        id="flag_id"
        className="input"
        style={{
          width: '100%',
          borderColor: isError ? 'var(--danger)' : undefined,
        }}
        value={value}
        disabled={disabled}
        onChange={(e) => setFlag(e.target.value)}
      >
        <option value="">{disabled ? 'Flag is immutable' : 'Pick a flag…'}</option>
        {flags.map((f) => (
          <option key={f.flag_id} value={f.flag_id}>
            {f.name} ({f.key})
          </option>
        ))}
      </select>
      {errorMessage && (
        <div role="alert" style={{ fontSize: 12, color: 'var(--danger)', marginTop: 4 }}>
          {errorMessage}
        </div>
      )}
    </div>
  )
}

// ── Rule picker ─────────────────────────────────────────────────────────────

/**
 * The rule picker is fed via `filterEligibleRules`. Ineligible rows are
 * `disabled` with a `title=` tooltip (no `Tooltip` primitive exists in
 * `components/`, so we use the native HTML attribute — matches the
 * pattern in `RuleCard.tsx`).
 *
 * The picker injects a virtual `default_rule` row carrying `rule_id === null`;
 * selecting it flips `targets_default_rule = true` and clears `flag_rule_id`.
 */
function RulePicker({
  options,
  cta,
}: {
  options: RulePickerOption[]
  cta: { label: string; onClick: () => void } | null
}) {
  const { values, setFieldValue } = useFormikContext<ExperimentFormValues>()
  const [, meta] = useField<string>('flag_rule_id')
  const isError = meta.touched && Boolean(meta.error)
  const errorMessage = isError ? meta.error : undefined

  // Compute the currently-selected option id from the form state.
  const selectedId =
    values.targets_default_rule
      ? 'default_rule'
      : values.flag_rule_id
        ? `rule:${values.flag_rule_id}`
        : ''

  function onPick(id: string) {
    const opt = options.find((o) => o.id === id)
    if (!opt || !opt.eligible) return
    if (opt.kind === 'default_rule') {
      void setFieldValue('targets_default_rule', true)
      void setFieldValue('flag_rule_id', '')
    } else {
      void setFieldValue('targets_default_rule', false)
      void setFieldValue('flag_rule_id', opt.rule_id ?? '')
    }
  }

  const noEligible = options.every((o) => !o.eligible)

  return (
    <div>
      <label className="label" style={{ display: 'block', marginBottom: 4 }}>
        Target rule
      </label>
      <select
        className="input"
        aria-label="Target rule"
        value={selectedId}
        onChange={(e) => onPick(e.target.value)}
        style={{
          width: '100%',
          borderColor: isError ? 'var(--danger)' : undefined,
        }}
      >
        <option value="">Pick a target rule…</option>
        {options.map((o) => (
          <option
            key={o.id}
            value={o.id}
            disabled={!o.eligible}
            title={o.disabled_reason}
          >
            {o.label}{!o.eligible ? ' (requires percentage rollout)' : ''}
          </option>
        ))}
      </select>

      {/* Ineligible rule list — surfaced visibly under the picker so the
          "Experiments require a percentage rollout rule" tooltip text is
          always discoverable. */}
      {options.some((o) => !o.eligible) && (
        <ul
          aria-label="Ineligible rules"
          style={{
            margin: '6px 0 0',
            padding: 0,
            listStyle: 'none',
            fontSize: 11,
            color: 'var(--fg-muted)',
          }}
        >
          {options
            .filter((o) => !o.eligible)
            .map((o) => (
              <li
                key={`${o.id}-disabled`}
                title={o.disabled_reason}
                style={{ paddingTop: 2 }}
              >
                <I.lock size={9} /> {o.label} — {o.disabled_reason}
              </li>
            ))}
        </ul>
      )}

      {noEligible && cta && (
        <div
          style={{
            marginTop: 8,
            padding: '8px 12px',
            background: 'var(--bg-sunken)',
            border: '1px solid var(--border)',
            borderRadius: 6,
            fontSize: 12,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
          }}
        >
          <span style={{ color: 'var(--fg-muted)' }}>
            No eligible target — add a percentage rollout rule or configure the
            flag's default-rule distribution.
          </span>
          <button type="button" className="btn sm" onClick={cta.onClick}>
            {cta.label}
          </button>
        </div>
      )}

      {errorMessage && (
        <div role="alert" style={{ fontSize: 12, color: 'var(--danger)', marginTop: 4 }}>
          {errorMessage}
        </div>
      )}
    </div>
  )
}

// ── Metric picker ───────────────────────────────────────────────────────────

/**
 * Multi-select metric picker (used for both primary `metric_ids` and
 * `guardrail_metric_ids`). The implementation is identical between the two
 * — only the form field name + max changes — so we parameterise it.
 */
function MetricPicker({
  envId,
  fieldName,
  label,
  hint,
  max,
}: {
  envId: string
  fieldName: 'metric_ids' | 'guardrail_metric_ids'
  label: string
  hint: string
  max: number
}) {
  const [{ value: selectedIds }, meta, helpers] = useField<string[]>(fieldName)

  const [allOptions, setAllOptions] = useState<MetricPickerOption[]>([])
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [query, setQuery] = useState('')
  const [open, setOpen] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)

  // Fetch metric list once (scoped to current env). Shared across mounts via
  // the simple cache on `api`.
  useEffect(() => {
    if (!envId) return
    setLoading(true)
    setLoadError(null)
    const ctrl = new AbortController()
    api
      .get<MetricsListResponse>(
        `/v1/metrics?env_id=${encodeURIComponent(envId)}&per_page=200`,
        { signal: ctrl.signal },
      )
      .then(({ data }) => {
        setAllOptions(
          (data.items ?? []).map((m) => ({
            id: m.id,
            key: m.key,
            name: m.name,
            kind: m.kind,
          })),
        )
      })
      .catch((err: unknown) => {
        if ((err as { code?: string }).code === 'ERR_CANCELED') return
        setLoadError(extractErrorMessage(err))
      })
      .finally(() => setLoading(false))
    return () => ctrl.abort()
  }, [envId])

  useEffect(() => {
    if (!open) return
    function onMouseDown(e: MouseEvent) {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', onMouseDown)
    return () => document.removeEventListener('mousedown', onMouseDown)
  }, [open])

  const selectedSet = new Set(selectedIds ?? [])
  const selectedOptions = allOptions.filter((o) => selectedSet.has(o.id))
  const visibleOptions = filterMetricOptions(allOptions, query, selectedSet)
  const atMax = (selectedIds?.length ?? 0) >= max

  function addMetric(id: string) {
    if ((selectedIds ?? []).includes(id) || atMax) return
    void helpers.setValue([...(selectedIds ?? []), id])
    setQuery('')
    setOpen(false)
  }

  function removeMetric(id: string) {
    void helpers.setValue((selectedIds ?? []).filter((s) => s !== id))
  }

  const isError = meta.touched && Boolean(meta.error)
  const errorMessage = isError ? (Array.isArray(meta.error) ? meta.error[0] : meta.error) : undefined

  return (
    <div ref={containerRef}>
      <label className="label" style={{ display: 'block', marginBottom: 4 }}>
        {label}
      </label>

      {selectedOptions.length > 0 && (
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, marginBottom: 6 }}>
          {selectedOptions.map((opt) => {
            const chip = metricKindChipStyle(opt.kind)
            return (
              <span
                key={opt.id}
                style={{
                  display: 'inline-flex',
                  alignItems: 'center',
                  gap: 6,
                  padding: '4px 8px',
                  borderRadius: 6,
                  background: 'var(--bg-sunken)',
                  border: '1px solid var(--border)',
                  fontSize: 12,
                }}
              >
                <span
                  style={{
                    fontSize: 9,
                    fontWeight: 600,
                    padding: '1px 5px',
                    borderRadius: 3,
                    textTransform: 'uppercase',
                    letterSpacing: 0.3,
                    ...chip,
                  }}
                >
                  {opt.kind}
                </span>
                <span style={{ fontWeight: 600 }}>{opt.name}</span>
                <button
                  type="button"
                  className="icon-btn"
                  aria-label={`Remove ${opt.name}`}
                  onClick={() => removeMetric(opt.id)}
                  style={{ padding: 0 }}
                >
                  <I.x size={10} />
                </button>
              </span>
            )
          })}
        </div>
      )}

      <div style={{ position: 'relative' }}>
        <input
          className="input"
          type="text"
          placeholder={
            atMax
              ? `Maximum ${max} metrics selected`
              : loading
                ? 'Loading metrics…'
                : 'Search metrics by name or key…'
          }
          value={query}
          disabled={atMax || loading || Boolean(loadError)}
          onChange={(e) => {
            setQuery(e.target.value)
            setOpen(true)
          }}
          onFocus={() => setOpen(true)}
          aria-invalid={isError}
          style={{
            width: '100%',
            borderColor: isError ? 'var(--danger)' : undefined,
          }}
        />
        {open && !atMax && !loading && !loadError && (
          <ul
            role="listbox"
            style={{
              position: 'absolute',
              top: '100%',
              left: 0,
              right: 0,
              zIndex: 100,
              background: 'var(--surface)',
              border: '1px solid var(--border)',
              borderRadius: 6,
              boxShadow: 'var(--shadow-md)',
              listStyle: 'none',
              margin: '4px 0 0',
              padding: '4px 0',
              maxHeight: 240,
              overflowY: 'auto',
            }}
          >
            {visibleOptions.length === 0 && (
              <li
                style={{
                  padding: '8px 12px',
                  fontSize: 12,
                  color: 'var(--fg-muted)',
                  fontStyle: 'italic',
                }}
              >
                {query ? 'No matching metrics' : 'No metrics defined yet'}
              </li>
            )}
            {visibleOptions.map((opt) => {
              const chip = metricKindChipStyle(opt.kind)
              return (
                <li
                  key={opt.id}
                  role="option"
                  aria-selected={false}
                  onMouseDown={(e) => {
                    e.preventDefault()
                    addMetric(opt.id)
                  }}
                  style={{
                    padding: '8px 12px',
                    cursor: 'pointer',
                    display: 'flex',
                    alignItems: 'center',
                    gap: 8,
                    fontSize: 13,
                  }}
                  onMouseEnter={(e) => {
                    e.currentTarget.style.background = 'var(--bg-sunken)'
                  }}
                  onMouseLeave={(e) => {
                    e.currentTarget.style.background = 'transparent'
                  }}
                >
                  <span
                    style={{
                      fontSize: 9,
                      fontWeight: 600,
                      padding: '1px 5px',
                      borderRadius: 3,
                      textTransform: 'uppercase',
                      letterSpacing: 0.3,
                      flexShrink: 0,
                      ...chip,
                    }}
                  >
                    {opt.kind}
                  </span>
                  <span style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontWeight: 600 }}>{opt.name}</div>
                    <div
                      style={{
                        fontSize: 11,
                        color: 'var(--fg-muted)',
                        fontFamily: 'var(--font-mono)',
                      }}
                    >
                      {opt.key}
                    </div>
                  </span>
                </li>
              )
            })}
          </ul>
        )}
      </div>

      {loadError && (
        <div role="alert" style={{ fontSize: 12, color: 'var(--danger)', marginTop: 4 }}>
          Failed to load metrics: {loadError}
        </div>
      )}
      {errorMessage && !loadError && (
        <div role="alert" style={{ fontSize: 12, color: 'var(--danger)', marginTop: 4 }}>
          {errorMessage}
        </div>
      )}
      {!errorMessage && !loadError && (
        <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 4 }}>
          {hint}
        </div>
      )}
    </div>
  )
}

// ── Unit-context-types multi-select ─────────────────────────────────────────

interface ContextTypeSuggestionRow {
  context_type: string
  last_seen_at: string
}

function UnitContextTypesPicker({ envId }: { envId: string }) {
  const [{ value: selected }, meta, helpers] = useField<string[]>('unit_context_types')
  const [available, setAvailable] = useState<string[]>([])
  const [loadError, setLoadError] = useState<string | null>(null)

  useEffect(() => {
    if (!envId) return
    const ctrl = new AbortController()
    api
      .get<ContextTypeSuggestionRow[]>(
        `/v1/environments/${encodeURIComponent(envId)}/context-types`,
        { signal: ctrl.signal },
      )
      .then(({ data }) => {
        // Normalise: backend returns rows with last_seen_at; we only need
        // the names. Always include 'user' since that's the canonical default
        // context type and the registry may not have a row for it on a fresh env.
        const names = new Set(data.map((row) => row.context_type))
        names.add('user')
        setAvailable(Array.from(names).sort())
      })
      .catch((err: unknown) => {
        if ((err as { code?: string }).code === 'ERR_CANCELED') return
        setLoadError(extractErrorMessage(err))
        setAvailable(['user'])
      })
    return () => ctrl.abort()
  }, [envId])

  function toggle(ct: string) {
    const has = (selected ?? []).includes(ct)
    if (has) {
      void helpers.setValue((selected ?? []).filter((s) => s !== ct))
    } else {
      void helpers.setValue([...(selected ?? []), ct])
    }
  }

  const isError = meta.touched && Boolean(meta.error)
  const errorMessage = isError
    ? Array.isArray(meta.error)
      ? meta.error[0]
      : meta.error
    : undefined

  return (
    <div>
      <label className="label" style={{ display: 'block', marginBottom: 4 }}>
        Unit context types
      </label>
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
        {available.map((ct) => {
          const active = (selected ?? []).includes(ct)
          return (
            <button
              key={ct}
              type="button"
              onClick={() => toggle(ct)}
              style={{
                padding: '4px 10px',
                borderRadius: 6,
                border: active
                  ? '1px solid var(--accent)'
                  : '1px solid var(--border-strong)',
                background: active ? 'var(--surface)' : 'transparent',
                color: active ? 'var(--accent)' : 'var(--fg-muted)',
                fontWeight: active ? 600 : 500,
                fontSize: 12,
                cursor: 'pointer',
              }}
            >
              {ct}
            </button>
          )
        })}
      </div>
      {loadError && (
        <div role="alert" style={{ fontSize: 11, color: 'var(--danger)', marginTop: 4 }}>
          Failed to load context types: {loadError}
        </div>
      )}
      {errorMessage && !loadError && (
        <div role="alert" style={{ fontSize: 12, color: 'var(--danger)', marginTop: 4 }}>
          {errorMessage}
        </div>
      )}
      {!errorMessage && !loadError && (
        <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 4 }}>
          Experiment analysis is computed independently per context type.
        </div>
      )}
    </div>
  )
}

// ── Sequential testing (always-valid) section ─────────────────────────────────

/**
 * Opt-in section for sequential (always-valid) testing. A checkbox toggles the
 * feature; when enabled, three advanced knobs appear (α, τ², minimum sample
 * before the first look). Mirrors the gateway's `sequential_*` create-body
 * fields. The advanced inputs are gated behind the toggle so the common case
 * (fixed-horizon test) stays uncluttered.
 */
function SequentialTestingSection() {
  const { values } = useFormikContext<ExperimentFormValues>()
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: 12,
        padding: 12,
        border: '1px solid var(--border)',
        borderRadius: 8,
      }}
    >
      <FormCheckbox
        name="sequential_testing_enabled"
        label="Enable sequential testing (always-valid)"
        hint="Lets you peek at results any time without inflating the false-positive rate. Leave off for a standard fixed-horizon test."
      />
      {values.sequential_testing_enabled && (
        <>
          <FormField
            name="sequential_alpha"
            label="Significance level α"
            type="number"
            placeholder="0.05"
            step="0.01"
            hint="Target false-positive rate. Must be between 0 and 1 (default 0.05)."
          />
          <FormField
            name="sequential_tau_squared"
            label="Mixture variance τ² (blank = auto)"
            type="number"
            placeholder="auto"
            step="any"
            hint="Advanced: mSPRT mixing variance. Leave blank to auto-derive from the data."
          />
          <FormField
            name="sequential_min_sample_size"
            label="Min sample before first look"
            type="number"
            placeholder="100"
            hint="Per-variant sample size required before a sequential verdict is allowed (default 100)."
          />
        </>
      )}
    </div>
  )
}

// ── Exclusion-group picker (optional) ─────────────────────────────────────────

/**
 * Optional mutual-exclusion group selector. When a group is chosen, surfaces
 * its remaining capacity and validates that the experiment's traffic
 * allocation fits within the group's `free_bp` — blocking the form (and
 * disabling submit) when it does not.
 *
 * Ungrouped is the default (empty value).
 */
function ExclusionGroupPicker({ envId }: { envId: string }) {
  const { values } = useFormikContext<ExperimentFormValues>()
  const [{ value }, , helpers] = useField<string>('exclusion_group_id')

  const [groups, setGroups] = useState<ExclusionGroup[]>([])
  const [loadError, setLoadError] = useState<string | null>(null)

  useEffect(() => {
    if (!envId) return
    const ctrl = new AbortController()
    listExclusionGroups(envId, { per_page: 200 }, ctrl.signal)
      .then((res) => setGroups(res.items ?? []))
      .catch((err: unknown) => {
        if ((err as { code?: string }).code === 'ERR_CANCELED') return
        setLoadError(extractErrorMessage(err))
      })
    return () => ctrl.abort()
  }, [envId])

  const selected = groups.find((g) => g.id === value) ?? null
  const capacity = selected
    ? exclusionGroupFitsCapacity(selected, values.traffic_allocation)
    : null

  return (
    <div>
      <label
        className="label"
        htmlFor="exclusion_group_id"
        style={{ display: 'block', marginBottom: 4 }}
      >
        Mutual-exclusion group
      </label>
      <select
        id="exclusion_group_id"
        className="input"
        aria-label="Mutual-exclusion group"
        value={value ?? ''}
        onChange={(e) => void helpers.setValue(e.target.value)}
        style={{
          width: '100%',
          borderColor: capacity && !capacity.fits ? 'var(--danger)' : undefined,
        }}
      >
        <option value="">Ungrouped (default)</option>
        {groups.map((g) => (
          <option key={g.id} value={g.id}>
            {g.name} ({formatBpPercent(g.free_bp)} free)
          </option>
        ))}
      </select>

      {loadError && (
        <div role="alert" style={{ fontSize: 11, color: 'var(--danger)', marginTop: 4 }}>
          Failed to load groups: {loadError}
        </div>
      )}

      {selected && capacity && (
        <div
          role={capacity.fits ? undefined : 'alert'}
          style={{
            fontSize: 11,
            marginTop: 4,
            color: capacity.fits ? 'var(--fg-muted)' : 'var(--danger)',
          }}
        >
          {capacity.fits
            ? `Remaining capacity: ${formatBpPercent(selected.free_bp)} free. This experiment requests ${formatBpPercent(capacity.requestedBp)}.`
            : capacity.message}
        </div>
      )}

      {!selected && !loadError && (
        <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 4 }}>
          Optional. Grouped experiments never share traffic — their allocation
          is carved from the group's budget.
        </div>
      )}
    </div>
  )
}

// ── Main modal ────────────────────────────────────────────────────────────────

interface Props {
  onClose: () => void
  /** When present, the modal opens in edit mode for this experiment key. */
  editExperimentKey?: string
}

/**
 * Project the flag's `AdminFlagJson.rules` into the shape the rule picker
 * consumes. The gateway now surfaces `feature_flag_rules.id` as `rule_id` and
 * the rule's UI label as `name` (feature-flag-1p6); we read both directly and
 * pass them through.
 */
function ruleListFromFlag(flag: ExpFlagShape | null): RuleWithId[] {
  if (!flag) return []
  return flag.rules.map((r) => ({
    rule_id: r.rule_id,
    name: r.name ?? null,
    output: r.output,
  }))
}

export function CreateExperimentModal({ onClose, editExperimentKey }: Props) {
  const navigate = useNavigate()
  const { orgId, envId, projectId } = useOrgContext()
  const isEdit = Boolean(editExperimentKey)

  // ── Async loaders ─────────────────────────────────────────────────────────

  const [flags, setFlags] = useState<ExpFlagShape[]>([])
  const [flagsLoading, setFlagsLoading] = useState(true)
  const [activeFlag, setActiveFlag] = useState<ExpFlagShape | null>(null)
  const [loadedExperiment, setLoadedExperiment] = useState<{
    name: string
    key: string
    description: string
    flag_id: string
    flag_rule_id: string
    targets_default_rule: boolean
    metric_ids: string[]
    guardrail_metric_ids: string[]
    unit_context_types: string[]
    pre_period_days: number
    sequential_testing_enabled: boolean
    sequential_alpha: number
    sequential_tau_squared?: number | null
    sequential_min_sample_size: number
    traffic_allocation: number
    model: 'bayesian' | 'frequentist'
    exclusion_group_id: string
  } | null>(null)

  // Flag list (env-scoped) — backs the FlagPicker.
  useEffect(() => {
    if (!projectId) return
    setFlagsLoading(true)
    const ctrl = new AbortController()
    api
      .get<FlagListResponse>(
        `/v1/projects/${encodeURIComponent(projectId)}/flags?per_page=200`,
        { signal: ctrl.signal },
      )
      .then(({ data }) => setFlags(data.items ?? []))
      .catch((err: unknown) => {
        if ((err as { code?: string }).code === 'ERR_CANCELED') return
      })
      .finally(() => setFlagsLoading(false))
    return () => ctrl.abort()
  }, [projectId])

  // Edit-mode: load the experiment.
  useEffect(() => {
    if (!isEdit || !envId || !editExperimentKey) return
    const ctrl = new AbortController()
    interface FullExperimentJson {
      key: string
      name: string
      description: string
      flag_id?: string
      flag_rule_id?: string | null
      targets_default_rule?: boolean
      metric_ids: string[]
      guardrail_metric_ids?: string[]
      unit_context_types?: string[]
      pre_period_days?: number
      sequential_testing_enabled?: boolean
      sequential_alpha?: number
      sequential_tau_squared?: number | null
      sequential_min_sample_size?: number
      traffic_allocation?: number
      model: 'bayesian' | 'frequentist'
      exclusion_group_id?: string | null
    }
    api
      .get<FullExperimentJson>(
        `/v1/environments/${encodeURIComponent(envId)}/experiments/${encodeURIComponent(editExperimentKey)}`,
        { signal: ctrl.signal },
      )
      .then(({ data }) => {
        setLoadedExperiment({
          key: data.key,
          name: data.name,
          description: data.description ?? '',
          flag_id: data.flag_id ?? '',
          flag_rule_id: data.flag_rule_id ?? '',
          targets_default_rule: Boolean(data.targets_default_rule),
          metric_ids: data.metric_ids,
          guardrail_metric_ids: data.guardrail_metric_ids ?? [],
          unit_context_types: data.unit_context_types ?? ['user'],
          pre_period_days: data.pre_period_days ?? 0,
          sequential_testing_enabled: Boolean(data.sequential_testing_enabled),
          // `0` from the gateway means "service default" — show 0.05 so the
          // field is meaningful; a stored positive value prefills as-is.
          sequential_alpha:
            data.sequential_alpha != null && data.sequential_alpha > 0
              ? data.sequential_alpha
              : 0.05,
          // Blank when absent/null → the input shows empty (= auto-derive).
          sequential_tau_squared: data.sequential_tau_squared ?? undefined,
          sequential_min_sample_size:
            data.sequential_min_sample_size != null &&
            data.sequential_min_sample_size > 0
              ? data.sequential_min_sample_size
              : 100,
          traffic_allocation:
            data.traffic_allocation != null ? data.traffic_allocation * 100 : 100,
          model: data.model,
          exclusion_group_id: data.exclusion_group_id ?? '',
        })
      })
      .catch(() => undefined)
    return () => ctrl.abort()
  }, [isEdit, envId, editExperimentKey])

  // When the loaded experiment refers to a flag, lift it as `activeFlag`.
  useEffect(() => {
    if (!loadedExperiment || flagsLoading) return
    const flag = flags.find((f) => f.flag_id === loadedExperiment.flag_id) ?? null
    setActiveFlag(flag)
  }, [loadedExperiment, flags, flagsLoading])

  // ── Form initial values ───────────────────────────────────────────────────

  const initialValues: ExperimentFormValues = loadedExperiment ?? {
    name: '',
    key: '',
    description: '',
    flag_id: '',
    flag_rule_id: '',
    targets_default_rule: false,
    metric_ids: [],
    guardrail_metric_ids: [],
    unit_context_types: ['user'],
    pre_period_days: 0,
    sequential_testing_enabled: false,
    sequential_alpha: 0.05,
    sequential_tau_squared: undefined,
    sequential_min_sample_size: 100,
    traffic_allocation: 100,
    model: 'bayesian',
    exclusion_group_id: '',
  }

  // ── Submit handler ────────────────────────────────────────────────────────

  async function handleSubmit(
    values: ExperimentFormValues,
    { setStatus }: { setStatus: (s: unknown) => void },
  ) {
    if (!envId) return
    try {
      if (isEdit && editExperimentKey) {
        const body = buildExperimentPatchBody(values)
        await api.patch(
          `/v1/environments/${envId}/experiments/${editExperimentKey}`,
          body,
        )
        onClose()
        navigate(`/org/${orgId}/experiments/${editExperimentKey}`)
      } else {
        const body = buildExperimentCreateBody(values, envId)
        const { data } = await api.post<{ key: string; id?: string }>(
          `/v1/environments/${envId}/experiments`,
          body,
        )
        // Optional mutual-exclusion assignment. `requested_bp` is the traffic
        // allocation in basis points (traffic% × 100). Errors here surface in
        // the banner without losing the just-created experiment.
        const groupId = values.exclusion_group_id?.trim()
        if (groupId) {
          // Re-validate against the group's live free capacity before assigning
          // (the picker shows a warning, but capacity can drift between load
          // and submit). Block when the request doesn't fit.
          const group = await getExclusionGroup(envId, groupId)
          const capacity = exclusionGroupFitsCapacity(
            group,
            values.traffic_allocation,
          )
          if (!capacity.fits) {
            setStatus({ error: capacity.message })
            return
          }
          await assignExperimentToGroup(envId, data.id ?? data.key, {
            group_id: groupId,
            requested_bp: capacity.requestedBp,
          })
        }
        onClose()
        navigate(`/org/${orgId}/experiments/${data.key}`)
      }
    } catch (err: unknown) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

  // ── Render ───────────────────────────────────────────────────────────────

  const rulePickerOptions: RulePickerOption[] = filterEligibleRules(
    ruleListFromFlag(activeFlag).map((r) => ({
      rule_id: r.rule_id,
      name: r.name ?? undefined,
      output: r.output,
    })),
    activeFlag?.default_rule_distribution ?? null,
  )

  // CTA to nudge the user to the flag-edit page when no eligible rule exists.
  const cta =
    activeFlag &&
    !hasDefaultRuleDistribution(activeFlag.default_rule_distribution) &&
    rulePickerOptions.every((o) => !o.eligible)
      ? {
          label: 'Configure flag',
          onClick: () => navigate(`/org/${orgId}/flags/${activeFlag.key}`),
        }
      : null

  const header = (
    <div
      className="card-header"
      style={{
        padding: '16px 20px',
        borderBottom: '1px solid var(--border)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
      }}
    >
      <div className="card-title">
        <I.beaker size={15} /> {isEdit ? 'Edit experiment' : 'New experiment'}
      </div>
      <button type="button" className="icon-btn" onClick={onClose}>
        <I.x size={16} />
      </button>
    </div>
  )

  const footer = (
    <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
      <button type="button" className="btn" onClick={onClose}>
        Cancel
      </button>
      <FormSubmit
        label={isEdit ? 'Save changes' : 'Create experiment'}
        loadingLabel={isEdit ? 'Saving…' : 'Creating…'}
        form="experiment-form"
      />
    </div>
  )

  return (
    <Formik
      initialValues={initialValues}
      validationSchema={experimentSchema}
      validateOnChange={false}
      enableReinitialize
      onSubmit={handleSubmit}
    >
      <Modal isOpen onClose={onClose} size="md" title={header} footer={footer}>
        <Form
          id="experiment-form"
          style={{ display: 'flex', flexDirection: 'column', gap: 16 }}
        >
          <AutoSlugKey disabled={isEdit} />
          <FormErrorBanner />

          <FormField
            name="name"
            label="Name"
            placeholder="e.g. Checkout Button Colour"
            autoFocus
          />
          <FormField
            name="key"
            label="Key"
            placeholder="e.g. checkout-btn-colour"
            hint={
              isEdit
                ? 'Immutable.'
                : 'Auto-generated from name. Immutable after creation.'
            }
            disabled={isEdit}
            style={{ fontFamily: 'var(--font-mono)' }}
          />
          <FormTextarea
            name="description"
            label="Description"
            placeholder="Optional"
            style={{ minHeight: 56 }}
          />

          <FlagPicker
            flags={flags}
            onSelected={(f) => setActiveFlag(f)}
            disabled={isEdit}
          />

          <RulePicker options={rulePickerOptions} cta={cta} />

          <FormSelect name="model" label="Statistical model" options={MODEL_OPTIONS} />

          {envId && (
            <>
              <MetricPicker
                envId={envId}
                fieldName="metric_ids"
                label="Primary metrics"
                hint={`Select 1–${MAX_METRIC_IDS} primary metrics.`}
                max={MAX_METRIC_IDS}
              />
              <MetricPicker
                envId={envId}
                fieldName="guardrail_metric_ids"
                label="Guardrail metrics"
                hint={`Optional — up to ${MAX_GUARDRAIL_METRIC_IDS} guardrails. Violations surface as warnings.`}
                max={MAX_GUARDRAIL_METRIC_IDS}
              />
            </>
          )}

          {envId && <UnitContextTypesPicker envId={envId} />}

          <FormField
            name="pre_period_days"
            label="Pre-period days (CUPED)"
            type="number"
            placeholder="0"
            hint="Pre-experiment lookback window for CUPED variance reduction. 0 = disabled."
          />

          <SequentialTestingSection />

          <FormField
            name="traffic_allocation"
            label="Traffic allocation (%)"
            type="number"
            placeholder="100"
            hint="Fraction of exposures included in the experiment. 100 = full rollout."
          />

          {envId && <ExclusionGroupPicker envId={envId} />}
        </Form>
      </Modal>
    </Formik>
  )
}
