import { useCallback, useEffect, useMemo, useState } from 'react'
import { Formik, Form, FieldArray, useFormikContext } from 'formik'
import { useOrgContext } from '../../context/OrgContext'
import {
  getPrerequisites,
  setPrerequisites,
  listFlags,
  getFlag,
} from '../../lib/api'
import type { AdminFlagResponse } from '../../lib/types'
import { extractErrorMessage } from '../../lib/errors'
import { prerequisitesSchema } from '../../lib/validation/lifecycle'
import type { PrerequisitesFormValues } from '../../lib/validation/lifecycle'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { detectLocalCycle, toSetBody } from './prerequisiteHelpers'
import type { PrereqRow } from './prerequisiteHelpers'

interface Props {
  flag: AdminFlagResponse
  canWrite: boolean
  onSaved?: () => void
}

// ─── Cycle warning (live, inside Formik) ─────────────────────────────────────

function CycleWarning({
  flagKey,
  reverseDeps,
}: {
  flagKey: string
  reverseDeps: Record<string, string[]>
}) {
  const { values } = useFormikContext<PrerequisitesFormValues>()
  const cycle = detectLocalCycle(flagKey, values.prerequisites as PrereqRow[], reverseDeps)
  if (!cycle) return null
  return (
    <div
      role="alert"
      style={{
        padding: '8px 12px',
        borderRadius: 6,
        background: 'var(--danger-bg, #fee2e2)',
        color: 'var(--danger)',
        fontSize: 12,
      }}
    >
      Prerequisite cycle: <code>{cycle}</code>. A flag cannot (transitively) require
      itself — remove this row before saving.
    </div>
  )
}

// ─── Variant picker per row (fetches the prereq flag's variants) ─────────────

function VariantPicker({
  name,
  flagKey,
  variantsByFlag,
  loadVariants,
  disabled,
}: {
  name: string
  flagKey: string
  variantsByFlag: Record<string, string[]>
  loadVariants: (flagKey: string) => void
  disabled: boolean
}) {
  const { values, setFieldValue } = useFormikContext<PrerequisitesFormValues>()
  const idx = Number(name.split('.')[1])
  const current = values.prerequisites[idx]?.required_variant_key ?? ''

  useEffect(() => {
    if (flagKey && variantsByFlag[flagKey] == null) loadVariants(flagKey)
  }, [flagKey, variantsByFlag, loadVariants])

  const options = variantsByFlag[flagKey] ?? []

  return (
    <select
      className="input"
      style={{ flex: 1 }}
      value={current}
      disabled={disabled || !flagKey}
      onChange={(e) => void setFieldValue(`prerequisites.${idx}.required_variant_key`, e.target.value)}
    >
      <option value="">
        {flagKey ? (options.length ? 'Required variant…' : 'Loading variants…') : 'Pick a flag first'}
      </option>
      {options.map((v) => (
        <option key={v} value={v}>
          {v}
        </option>
      ))}
    </select>
  )
}

// ─── Editor ──────────────────────────────────────────────────────────────────

export function PrerequisitesEditor({ flag, canWrite, onSaved }: Props) {
  const { projectId } = useOrgContext()
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [initial, setInitial] = useState<PrerequisitesFormValues | null>(null)
  const [savedMsg, setSavedMsg] = useState<string | null>(null)

  // All project flags (for the prerequisite-flag picker), excluding this flag.
  const [allFlags, setAllFlags] = useState<{ key: string; deps: string[] }[]>([])
  // Lazily-loaded variant keys per prerequisite flag.
  const [variantsByFlag, setVariantsByFlag] = useState<Record<string, string[]>>({})

  const reverseDeps = useMemo(() => {
    const m: Record<string, string[]> = {}
    for (const f of allFlags) m[f.key] = f.deps
    return m
  }, [allFlags])

  const loadVariants = useCallback(
    async (flagKey: string) => {
      if (!projectId) return
      try {
        const f = await getFlag(projectId, flagKey)
        setVariantsByFlag((prev) => ({ ...prev, [flagKey]: f.variants.map((v) => v.key) }))
      } catch {
        setVariantsByFlag((prev) => ({ ...prev, [flagKey]: [] }))
      }
    },
    [projectId],
  )

  useEffect(() => {
    if (!projectId) return
    const controller = new AbortController()
    setLoading(true)
    setLoadError(null)
    Promise.all([
      getPrerequisites(projectId, flag.key, controller.signal),
      listFlags(projectId, { limit: 500 }, controller.signal),
    ])
      .then(([gate, flagsPage]) => {
        setInitial({
          prerequisites: gate.prerequisites.map((p) => ({
            prerequisite_flag_key: p.prerequisite_flag_key,
            required_variant_key: p.required_variant_key,
          })),
          fallback_variant_key: gate.fallback_variant_key,
        })
        setAllFlags(
          flagsPage.items
            .filter((f) => f.key !== flag.key)
            .map((f) => ({
              key: f.key,
              deps: f.prerequisites?.map((p) => p.prerequisite_flag_key) ?? [],
            })),
        )
        // Pre-load variants for already-configured prerequisites.
        for (const p of gate.prerequisites) void loadVariants(p.prerequisite_flag_key)
      })
      .catch((err) => {
        if (!controller.signal.aborted) setLoadError(extractErrorMessage(err))
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [projectId, flag.key, loadVariants])

  if (loading) {
    return <div style={{ fontSize: 13, color: 'var(--fg-muted)', padding: 12 }}>Loading prerequisites…</div>
  }
  if (loadError) {
    return <div role="alert" style={{ fontSize: 13, color: 'var(--danger)', padding: 12 }}>{loadError}</div>
  }
  if (!initial) return null

  async function handleSubmit(
    values: PrerequisitesFormValues,
    helpers: { setStatus: (s: unknown) => void; setSubmitting: (b: boolean) => void },
  ) {
    if (!projectId) return
    helpers.setStatus(undefined)
    setSavedMsg(null)
    try {
      await setPrerequisites(
        projectId,
        flag.key,
        toSetBody(values.prerequisites as PrereqRow[], values.fallback_variant_key, flag.version),
      )
      setSavedMsg('Prerequisites saved')
      onSaved?.()
    } catch (err) {
      // A 400 carries the cycle path; a 409 means the flag is experiment-locked.
      helpers.setStatus({ error: extractErrorMessage(err) })
    } finally {
      helpers.setSubmitting(false)
    }
  }

  const ownVariants = flag.variants.map((v) => v.key)

  return (
    <div className="card">
      <div className="card-header">
        <div className="card-title">Prerequisites</div>
      </div>
      <div style={{ padding: '12px 16px' }}>
        <p style={{ fontSize: 12, color: 'var(--fg-muted)', marginTop: 0 }}>
          Gate this flag on other flags resolving to a required variant. When a
          prerequisite is unmet, this flag returns its fallback variant and skips
          its rules.
        </p>

        <Formik
          initialValues={initial}
          validationSchema={prerequisitesSchema}
          enableReinitialize
          onSubmit={handleSubmit}
        >
          {({ values, setFieldValue }) => (
            <Form style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
              <FormErrorBanner />
              <CycleWarning flagKey={flag.key} reverseDeps={reverseDeps} />

              <FieldArray name="prerequisites">
                {(arrayHelpers) => (
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                    {values.prerequisites.length === 0 && (
                      <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>
                        No prerequisites — this flag is ungated.
                      </div>
                    )}
                    {values.prerequisites.map((row, i) => (
                      <div key={i} style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                        <select
                          className="input"
                          style={{ flex: 1 }}
                          value={row.prerequisite_flag_key}
                          disabled={!canWrite}
                          onChange={(e) => {
                            arrayHelpers.replace(i, {
                              prerequisite_flag_key: e.target.value,
                              required_variant_key: '',
                            })
                            if (e.target.value) void loadVariants(e.target.value)
                          }}
                        >
                          <option value="">Prerequisite flag…</option>
                          {allFlags.map((f) => (
                            <option key={f.key} value={f.key}>
                              {f.key}
                            </option>
                          ))}
                        </select>
                        <span style={{ fontSize: 12, color: 'var(--fg-muted)' }}>must be</span>
                        <VariantPicker
                          name={`prerequisites.${i}`}
                          flagKey={row.prerequisite_flag_key}
                          variantsByFlag={variantsByFlag}
                          loadVariants={(k) => void loadVariants(k)}
                          disabled={!canWrite}
                        />
                        <button
                          type="button"
                          className="btn sm danger"
                          disabled={!canWrite}
                          onClick={() => arrayHelpers.remove(i)}
                        >
                          Remove
                        </button>
                      </div>
                    ))}
                    {canWrite && (
                      <button
                        type="button"
                        className="btn sm"
                        onClick={() =>
                          arrayHelpers.push({ prerequisite_flag_key: '', required_variant_key: '' })
                        }
                      >
                        + Add prerequisite
                      </button>
                    )}
                  </div>
                )}
              </FieldArray>

              <div>
                <div className="label" style={{ marginBottom: 4 }}>
                  Fallback variant (returned when a prerequisite is unmet)
                </div>
                <select
                  className="input"
                  style={{ width: '100%' }}
                  value={values.fallback_variant_key}
                  disabled={!canWrite}
                  onChange={(e) => void setFieldValue('fallback_variant_key', e.target.value)}
                >
                  <option value="">Off / disabled variant (default)</option>
                  {ownVariants.map((v) => (
                    <option key={v} value={v}>
                      {v}
                    </option>
                  ))}
                </select>
              </div>

              {canWrite && (
                <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                  <SaveButton />
                  {savedMsg && (
                    <span style={{ fontSize: 12, color: 'var(--success, #22c55e)' }}>{savedMsg}</span>
                  )}
                </div>
              )}
            </Form>
          )}
        </Formik>
      </div>
    </div>
  )
}

function SaveButton() {
  const { isSubmitting } = useFormikContext()
  return (
    <button type="submit" className="btn primary" disabled={isSubmitting}>
      {isSubmitting ? 'Saving…' : 'Save prerequisites'}
    </button>
  )
}
