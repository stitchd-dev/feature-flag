import { useRef, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { Formik, Form, useFormikContext, useField } from 'formik'
import { I } from '../../components/icons'
import { Modal } from '../../components/Modal'
import { FormField } from '../../components/form/FormField'
import { FormSelect } from '../../components/form/FormSelect'
import { FormTextarea } from '../../components/form/FormTextarea'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { FormSubmit } from '../../components/form/FormSubmit'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'
import { slugify } from '../../lib/utils'
import { extractErrorMessage } from '../../lib/errors'
import { experimentSchema, MAX_METRIC_IDS } from '../../lib/validation/experimentSchema'
import type { ExperimentFormValues } from '../../lib/validation/experimentSchema'
import {
  buildExperimentCreateBody,
  filterMetricOptions,
  metricKindChipStyle,
  type MetricPickerOption,
} from './CreateExperimentModal.helpers'

const MODEL_OPTIONS = [
  { value: 'bayesian', label: 'Bayesian' },
  { value: 'frequentist', label: 'Frequentist' },
]

interface ExperimentVariant {
  key: string
  allocation: number
}

interface FormValues extends Omit<ExperimentFormValues, 'variants'> {
  variants: ExperimentVariant[]
}

// ── Auto-slug from name ───────────────────────────────────────────────────────

function AutoSlugKey() {
  const { values, setFieldValue } = useFormikContext<FormValues>()
  const keyEditedRef = useRef(false)
  const prevNameRef = useRef('')

  useEffect(() => {
    if (!keyEditedRef.current && values.name !== prevNameRef.current) {
      prevNameRef.current = values.name
      void setFieldValue('key', slugify(values.name), false)
    }
  }, [values.name, setFieldValue])

  return null
}

// ── Metric picker ─────────────────────────────────────────────────────────────

interface MetricsListResponse {
  items: { id: string; key: string; name: string; kind: 'aggregation' | 'ratio' | 'funnel' }[]
  total: number
}

/**
 * Multi-select metric picker.
 *
 * Backed by `GET /v1/metrics?env_id=...&limit=200` — fetched once on mount.
 * Renders a search input + searchable dropdown of name+kind+key rows; selected
 * picks display as removable chips above the input.
 */
function MetricPicker({ envId }: { envId: string }) {
  const [{ value: selectedIds }, meta, helpers] = useField<string[]>('metric_ids')

  const [allOptions, setAllOptions] = useState<MetricPickerOption[]>([])
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [query, setQuery] = useState('')
  const [open, setOpen] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)

  // Fetch metric list once (scoped to current env).
  useEffect(() => {
    if (!envId) return
    setLoading(true)
    setLoadError(null)
    const ctrl = new AbortController()
    api
      .get<MetricsListResponse>(
        `/v1/metrics?env_id=${encodeURIComponent(envId)}&limit=200`,
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

  // Click-outside to close the dropdown.
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

  const selectedSet = new Set(selectedIds)
  const selectedOptions = allOptions.filter((o) => selectedSet.has(o.id))
  const visibleOptions = filterMetricOptions(allOptions, query, selectedSet)
  const atMax = selectedIds.length >= MAX_METRIC_IDS

  function addMetric(id: string) {
    if (selectedIds.includes(id) || atMax) return
    void helpers.setValue([...selectedIds, id])
    setQuery('')
    setOpen(false)
  }

  function removeMetric(id: string) {
    void helpers.setValue(selectedIds.filter((s) => s !== id))
  }

  const isError = meta.touched && Boolean(meta.error)
  const errorMessage = isError ? (Array.isArray(meta.error) ? meta.error[0] : meta.error) : undefined

  return (
    <div ref={containerRef}>
      <label className="label" style={{ display: 'block', marginBottom: 4 }}>
        Metrics
      </label>

      {/* Selected metric chips */}
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

      {/* Search input */}
      <div style={{ position: 'relative' }}>
        <input
          className="input"
          type="text"
          placeholder={
            atMax
              ? `Maximum ${MAX_METRIC_IDS} metrics selected`
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
          Select 1–{MAX_METRIC_IDS} metrics. Lift is computed for each.
        </div>
      )}
    </div>
  )
}

// ── Variant allocation editor ─────────────────────────────────────────────────

function VariantEditor() {
  const { values, setFieldValue } = useFormikContext<FormValues>()

  function setKey(i: number, val: string) {
    const next = values.variants.map((v, j) => j === i ? { ...v, key: val } : v)
    void setFieldValue('variants', next)
  }

  function setAllocation(i: number, val: string) {
    const n = parseFloat(val) || 0
    const next = values.variants.map((v, j) => j === i ? { ...v, allocation: n } : v)
    void setFieldValue('variants', next)
  }

  function add() {
    const equal = Math.floor(100 / (values.variants.length + 1))
    const next = [...values.variants, { key: '', allocation: equal }]
    void setFieldValue('variants', next)
  }

  function remove(i: number) {
    if (values.variants.length <= 2) return
    void setFieldValue('variants', values.variants.filter((_, j) => j !== i))
  }

  const total = values.variants.reduce((s, v) => s + (v.allocation || 0), 0)
  const totalOk = Math.abs(total - 100) < 0.01

  return (
    <div>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
        <label className="label" style={{ margin: 0 }}>
          Variants &amp; allocation
        </label>
        <button type="button" className="btn sm" onClick={add}><I.plus size={11} /> Add</button>
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        {values.variants.map((v, i) => (
          <div key={i} style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
            <input
              className="input"
              style={{ flex: 1, fontFamily: 'var(--font-mono)', fontSize: 12 }}
              placeholder={i === 0 ? 'control' : `variant-${i}`}
              value={v.key}
              onChange={(e) => setKey(i, e.target.value)}
            />
            <input
              className="input"
              type="number"
              min={0}
              max={100}
              step={0.1}
              style={{ width: 80, fontFamily: 'var(--font-mono)', fontSize: 12 }}
              value={v.allocation}
              onChange={(e) => setAllocation(i, e.target.value)}
            />
            <span style={{ fontSize: 12, color: 'var(--fg-muted)' }}>%</span>
            <button
              type="button"
              className="icon-btn"
              style={{ color: values.variants.length <= 2 ? 'var(--fg-faint)' : 'var(--danger)' }}
              disabled={values.variants.length <= 2}
              onClick={() => remove(i)}
            >
              <I.x size={12} />
            </button>
          </div>
        ))}
      </div>
      <div style={{ fontSize: 11, marginTop: 6, color: totalOk ? 'var(--success)' : 'var(--danger)' }}>
        Total: {total.toFixed(1)}% {totalOk ? '✓' : '— must equal 100%'}
      </div>
    </div>
  )
}

// ── Main modal ────────────────────────────────────────────────────────────────

interface Props {
  onClose: () => void
}

export function CreateExperimentModal({ onClose }: Props) {
  const navigate = useNavigate()
  const { orgId, envId } = useOrgContext()

  const initialValues: FormValues = {
    name: '',
    key: '',
    flag_key: '',
    description: '',
    model: 'bayesian',
    metric_ids: [],
    duration_days: 14,
    variants: [
      { key: 'control', allocation: 50 },
      { key: 'treatment', allocation: 50 },
    ],
  }

  async function handleSubmit(
    values: FormValues,
    { setStatus }: { setStatus: (s: unknown) => void },
  ) {
    try {
      const body = buildExperimentCreateBody(values, envId ?? '')
      const { data } = await api.post<{ key: string }>(`/v1/environments/${envId}/experiments`, body)
      onClose()
      navigate(`/org/${orgId}/experiments/${data.key}`)
    } catch (err: unknown) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

  const header = (
    <div className="card-header" style={{ padding: '16px 20px', borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
      <div className="card-title"><I.beaker size={15} /> New experiment</div>
      <button type="button" className="icon-btn" onClick={onClose}><I.x size={16} /></button>
    </div>
  )

  const footer = (
    <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
      <button type="button" className="btn" onClick={onClose}>Cancel</button>
      <FormSubmit label="Create experiment" loadingLabel="Creating…" form="create-experiment-form" />
    </div>
  )

  return (
    <Formik
      initialValues={initialValues}
      validationSchema={experimentSchema}
      onSubmit={handleSubmit}
    >
      <Modal isOpen onClose={onClose} size="md" title={header} footer={footer}>
        <Form id="create-experiment-form" style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
          <AutoSlugKey />
          <FormErrorBanner />

          <FormField name="name" label="Name" placeholder="e.g. Checkout Button Colour" autoFocus />
          <FormField
            name="key"
            label="Key"
            placeholder="e.g. checkout-btn-colour"
            hint="Auto-generated from name. Immutable after creation."
            style={{ fontFamily: 'var(--font-mono)' }}
          />
          <FormField
            name="flag_key"
            label="Flag key"
            placeholder="e.g. checkout-btn-colour"
            hint="The feature flag that controls variant assignment."
            style={{ fontFamily: 'var(--font-mono)' }}
          />
          <FormTextarea name="description" label="Description" placeholder="Optional" style={{ minHeight: 56 }} />
          <FormSelect name="model" label="Statistical model" options={MODEL_OPTIONS} />
          {envId && <MetricPicker envId={envId} />}
          <FormField name="duration_days" label="Duration (days)" type="number" placeholder="14" />
          <VariantEditor />
        </Form>
      </Modal>
    </Formik>
  )
}
