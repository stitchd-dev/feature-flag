import { useRef, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { Formik, Form, useFormikContext } from 'formik'
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
import { experimentSchema } from '../../lib/validation/experimentSchema'
import type { ExperimentFormValues } from '../../lib/validation/experimentSchema'

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
    primary_metric: '',
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
      const body = {
        key: values.key.trim(),
        name: values.name.trim(),
        description: values.description?.trim(),
        flag_key: values.flag_key.trim(),
        model: values.model,
        primary_metric: values.primary_metric.trim(),
        duration_days: Number(values.duration_days),
        environment_id: envId,
        variants: values.variants.map((v) => ({ key: v.key.trim(), allocation: v.allocation / 100 })),
      }
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
      <FormSubmit label="Create experiment" loadingLabel="Creating…" />
    </div>
  )

  return (
    <Formik
      initialValues={initialValues}
      validationSchema={experimentSchema}
      onSubmit={handleSubmit}
    >
      <Modal isOpen onClose={onClose} size="md" title={header} footer={footer}>
        <Form style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
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
          <FormField name="primary_metric" label="Primary metric" placeholder="e.g. checkout_completed" hint="Event key used to compute lift." />
          <FormField name="duration_days" label="Duration (days)" type="number" placeholder="14" />
          <VariantEditor />
        </Form>
      </Modal>
    </Formik>
  )
}
