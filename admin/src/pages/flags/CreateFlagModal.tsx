import { useEffect, useRef, useCallback } from 'react'
import { useNavigate } from 'react-router-dom'
import { Formik, Form, useFormikContext, useField } from 'formik'
import * as Yup from 'yup'
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
import { flagSchema } from '../../lib/validation/flagSchema'
import type { AdminFlagResponse } from '../../lib/types'

type FlagType = 'bool' | 'string' | 'int' | 'double' | 'json'

interface VariantInput {
  key: string
  value: string
}

const DEFAULT_VARIANTS: Record<FlagType, VariantInput[]> = {
  bool: [{ key: 'on', value: 'true' }, { key: 'off', value: 'false' }],
  string: [{ key: 'control', value: '' }, { key: 'treatment', value: '' }],
  int: [{ key: 'control', value: '0' }, { key: 'treatment', value: '1' }],
  double: [{ key: 'control', value: '0.0' }, { key: 'treatment', value: '1.0' }],
  json: [{ key: 'control', value: '{}' }, { key: 'treatment', value: '{}' }],
}

const FLAG_TYPE_OPTIONS = [
  { value: 'bool', label: 'Boolean — on/off toggle' },
  { value: 'string', label: 'String — text values' },
  { value: 'int', label: 'Integer — whole numbers' },
  { value: 'double', label: 'Double — decimal numbers' },
  { value: 'json', label: 'JSON — arbitrary objects' },
]

function parseVariantValue(raw: string, type: FlagType): unknown {
  if (type === 'bool') return raw === 'true'
  if (type === 'int') return parseInt(raw, 10)
  if (type === 'double') return parseFloat(raw)
  if (type === 'json') { try { return JSON.parse(raw) } catch { return raw } }
  return raw
}

function validateVariantValue(raw: string, type: FlagType): string | null {
  if (type === 'bool') return null
  if (type === 'int') {
    if (!/^-?\d+$/.test(raw.trim())) return 'Must be a whole number (e.g. 42)'
  }
  if (type === 'double') {
    const n = Number(raw.trim())
    if (raw.trim() === '' || isNaN(n) || !isFinite(n)) return 'Must be a decimal number (e.g. 3.14)'
  }
  if (type === 'json') {
    try { JSON.parse(raw.trim()) } catch { return 'Must be valid JSON (e.g. {"key": "value"})' }
  }
  return null
}

// ── Auto-slug subcomponent ────────────────────────────────────────────────────
// Keeps the key field in sync with name unless the user has manually edited it.

function AutoSlugKey() {
  const { values, setFieldValue } = useFormikContext<FlagFormValues>()
  const keyEditedRef = useRef(false)
  const prevNameRef = useRef('')

  useEffect(() => {
    if (!keyEditedRef.current && values.name !== prevNameRef.current) {
      prevNameRef.current = values.name
      void setFieldValue('key', slugify(values.name), false)
    }
  }, [values.name, setFieldValue])

  const [, , keyHelpers] = useField('key')
  // Mark as manually edited when user types directly into key
  const originalSetValue = keyHelpers.setValue
  const markEdited = useCallback(
    (v: string) => {
      keyEditedRef.current = true
      void originalSetValue(v)
    },
    [originalSetValue],
  )

  return <input type="hidden" data-slug-sync="true" onChange={() => markEdited('')} />
}

// ── Variant rows editor ───────────────────────────────────────────────────────

function VariantEditor() {
  const { values, setFieldValue } = useFormikContext<FlagFormValues>()
  const flagType = values.value_type as FlagType

  function setVariantKey(i: number, val: string) {
    const next = values.variants.map((v, j) => j === i ? { ...v, key: val } : v)
    void setFieldValue('variants', next)
  }

  function setVariantValue(i: number, val: string) {
    const next = values.variants.map((v, j) => j === i ? { ...v, value: val } : v)
    void setFieldValue('variants', next)
  }

  function addVariant() {
    void setFieldValue('variants', [...values.variants, { key: '', value: '' }])
  }

  function removeVariant(i: number) {
    if (values.variants.length <= 1) return
    void setFieldValue('variants', values.variants.filter((_, j) => j !== i))
  }

  return (
    <div>
      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
        <label className="label" style={{ margin: 0 }}>Initial variants</label>
        <button type="button" className="btn sm" onClick={addVariant}><I.plus size={11} /> Add</button>
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        {values.variants.map((v, i) => {
          const valueErr = validateVariantValue(v.value, flagType)
          return (
            <div key={i} style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
              <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                <input
                  className="input"
                  style={{ width: 120, fontFamily: 'var(--font-mono)', fontSize: 12 }}
                  placeholder="name"
                  value={v.key}
                  onChange={(e) => setVariantKey(i, e.target.value)}
                />
                {flagType === 'bool' ? (
                  <select
                    className="input"
                    style={{ flex: 1, fontSize: 12 }}
                    value={v.value}
                    onChange={(e) => setVariantValue(i, e.target.value)}
                  >
                    <option value="true">true</option>
                    <option value="false">false</option>
                  </select>
                ) : (
                  <input
                    className="input"
                    style={{
                      flex: 1,
                      fontFamily: 'var(--font-mono)',
                      fontSize: 12,
                      borderColor: valueErr ? 'var(--danger)' : undefined,
                    }}
                    placeholder={flagType === 'json' ? '{}' : flagType === 'int' ? '42' : flagType === 'double' ? '3.14' : 'value'}
                    value={v.value}
                    onChange={(e) => setVariantValue(i, e.target.value)}
                  />
                )}
                <button
                  type="button"
                  className="icon-btn"
                  style={{ color: values.variants.length <= 1 ? 'var(--fg-faint)' : 'var(--danger)' }}
                  disabled={values.variants.length <= 1}
                  onClick={() => removeVariant(i)}
                >
                  <I.x size={12} />
                </button>
              </div>
              {valueErr && (
                <div style={{ fontSize: 11, color: 'var(--danger)', paddingLeft: 128 }}>
                  {valueErr}
                </div>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}

// ── Types ──────────────────────────────────────────────────────────────────────

interface FlagFormValues {
  name: string
  key: string
  description: string
  value_type: string
  variants: VariantInput[]
}

interface Props {
  onClose: () => void
}

// ── Flag type change resets variants ─────────────────────────────────────────

function FlagTypeWatcher() {
  const { values, setFieldValue } = useFormikContext<FlagFormValues>()
  const prevTypeRef = useRef(values.value_type)

  useEffect(() => {
    if (values.value_type !== prevTypeRef.current) {
      prevTypeRef.current = values.value_type
      const defaults = DEFAULT_VARIANTS[values.value_type as FlagType] ?? DEFAULT_VARIANTS.bool
      void setFieldValue('variants', defaults.map((v) => ({ ...v })))
    }
  }, [values.value_type, setFieldValue])

  return null
}

// ── Async key uniqueness validation schema ────────────────────────────────────

function buildFlagSchemaWithAsyncKey(projectId: string | null) {
  if (!projectId) return flagSchema

  return flagSchema.shape({
    key: Yup.string()
      .trim()
      .min(1, 'Key is required')
      .max(120, 'Key must be 120 characters or fewer')
      .matches(
        /^[a-z0-9][a-z0-9_-]*$/,
        'Key must start with a letter/digit and contain only lowercase letters, digits, hyphens, underscores',
      )
      .test('key-unique', 'This key is already taken', async (value) => {
        if (!value || value.trim().length < 2) return true
        try {
          await api.get(`/v1/projects/${projectId}/flags/${value.trim()}`)
          // If we get a 200, the key exists
          return false
        } catch {
          // 404 = key is available
          return true
        }
      })
      .required('Key is required'),
  })
}

// ── Main component ────────────────────────────────────────────────────────────

export function CreateFlagModal({ onClose }: Props) {
  const navigate = useNavigate()
  const { projectId, orgId } = useOrgContext()

  const schema = buildFlagSchemaWithAsyncKey(projectId)

  const initialValues: FlagFormValues = {
    name: '',
    key: '',
    description: '',
    value_type: 'bool',
    variants: DEFAULT_VARIANTS.bool.map((v) => ({ ...v })),
  }

  async function handleSubmit(
    values: FlagFormValues,
    { setStatus }: { setStatus: (s: unknown) => void },
  ) {
    if (!projectId) return
    const flagType = values.value_type as FlagType
    const validVariants = values.variants.filter((v) => v.key.trim())

    for (const v of validVariants) {
      const err = validateVariantValue(v.value, flagType)
      if (err) {
        setStatus({ error: `Variant "${v.key}": ${err}` })
        return
      }
    }

    try {
      const body = {
        key: values.key.trim(),
        name: values.name.trim(),
        description: values.description.trim(),
        enabled: false,
        value_type: flagType,
        variants: validVariants.map((v) => ({
          key: v.key.trim(),
          value: parseVariantValue(v.value, flagType),
        })),
      }
      const { data } = await api.post<AdminFlagResponse>(`/v1/projects/${projectId}/flags`, body)
      onClose()
      navigate(`/org/${orgId}/flags/${data.key}`)
    } catch (err: unknown) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

  const header = (
    <div className="card-header" style={{ padding: '16px 20px', borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
      <div className="card-title"><I.flag size={15} /> New feature flag</div>
      <button type="button" className="icon-btn" onClick={onClose}><I.x size={16} /></button>
    </div>
  )

  return (
    <Formik
      initialValues={initialValues}
      validationSchema={schema}
      validateOnChange={false}
      onSubmit={handleSubmit}
    >
      {({ isSubmitting }) => {
        const footer = (
          <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
            <button type="button" className="btn" onClick={onClose}>Cancel</button>
            <FormSubmit label="Create flag" loadingLabel="Creating…" form="create-flag-form" />
          </div>
        )

        return (
          <Modal isOpen onClose={onClose} size="md" title={header} footer={footer}>
            <Form id="create-flag-form" style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
              <AutoSlugKey />
              <FlagTypeWatcher />
              <FormErrorBanner />

              <FormField
                name="name"
                label="Name"
                placeholder="e.g. Dark Mode Beta"
                autoFocus
              />

              <div>
                <FormField
                  name="key"
                  label="Key"
                  placeholder="e.g. dark-mode-beta"
                  style={{ fontFamily: 'var(--font-mono)' }}
                />
                <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 4 }}>
                  Immutable after creation. Used in SDK calls.
                </div>
              </div>

              <FormTextarea
                name="description"
                label="Description"
                placeholder="Optional description of what this flag controls"
                style={{ minHeight: 64 }}
              />

              <FormSelect
                name="value_type"
                label="Flag type"
                options={FLAG_TYPE_OPTIONS}
              />

              <VariantEditor />

              {/* suppress unused variable warning */}
              <input type="hidden" value={isSubmitting ? '1' : '0'} />
            </Form>
          </Modal>
        )
      }}
    </Formik>
  )
}
