import { useState, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { I } from '../../components/icons'
import { Modal } from '../../components/Modal'
import { useOrgContext } from '../../context/OrgContext'
import { api } from '../../lib/api'
import { slugify } from '../../lib/utils'
import { extractErrorMessage } from '../../lib/errors'
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

function parseVariantValue(raw: string, type: FlagType): unknown {
  if (type === 'bool') return raw === 'true'
  if (type === 'int') return parseInt(raw, 10)
  if (type === 'double') return parseFloat(raw)
  if (type === 'json') { try { return JSON.parse(raw) } catch { return raw } }
  return raw
}

/** Returns an error string if the raw value doesn't match the flag type, or null if valid. */
function validateVariantValue(raw: string, type: FlagType): string | null {
  if (type === 'bool') return null // dropdown — always valid
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

interface Props {
  onClose: () => void
}

export function CreateFlagModal({ onClose }: Props) {
  const navigate = useNavigate()
  const { projectId, orgId } = useOrgContext()

  const [name, setName] = useState('')
  const [key, setKey] = useState('')
  const [keyEdited, setKeyEdited] = useState(false)
  const [description, setDescription] = useState('')
  const [flagType, setFlagType] = useState<FlagType>('bool')
  const [variants, setVariants] = useState<VariantInput[]>(DEFAULT_VARIANTS.bool)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // Per-row value validation errors (index → message | null)
  const [variantErrors, setVariantErrors] = useState<(string | null)[]>([null, null])

  // Auto-generate key from name unless user edited it manually
  useEffect(() => {
    if (!keyEdited) setKey(slugify(name))
  }, [name, keyEdited])

  // Reset variants and errors when flag type changes
  useEffect(() => {
    setVariants(DEFAULT_VARIANTS[flagType].map((v) => ({ ...v })))
    setVariantErrors(DEFAULT_VARIANTS[flagType].map(() => null))
  }, [flagType])

  function setVariantKey(i: number, val: string) {
    setVariants(variants.map((v, j) => j === i ? { ...v, key: val } : v))
  }

  function setVariantValue(i: number, val: string) {
    const next = variants.map((v, j) => j === i ? { ...v, value: val } : v)
    setVariants(next)
    setVariantErrors(next.map((v) => validateVariantValue(v.value, flagType)))
  }

  function addVariant() {
    setVariants([...variants, { key: '', value: '' }])
    setVariantErrors([...variantErrors, null])
  }

  function removeVariant(i: number) {
    if (variants.length <= 1) return
    setVariants(variants.filter((_, j) => j !== i))
    setVariantErrors(variantErrors.filter((_, j) => j !== i))
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    if (!projectId) return
    if (!name.trim()) { setError('Name is required'); return }
    if (!key.trim()) { setError('Key is required'); return }

    const validVariants = variants.filter((v) => v.key.trim())

    const keys = validVariants.map((v) => v.key.trim())
    if (new Set(keys).size !== keys.length) {
      setError('Variant keys must be unique')
      return
    }

    for (const v of validVariants) {
      const err = validateVariantValue(v.value, flagType)
      if (err) { setError(`Variant "${v.key}": ${err}`); return }
    }

    setSaving(true)
    setError(null)
    try {
      const body = {
        key: key.trim(),
        name: name.trim(),
        description: description.trim(),
        enabled: false,
        value_type: flagType,
        variants: validVariants.map((v) => ({ key: v.key.trim(), value: parseVariantValue(v.value, flagType) })),
      }
      const { data } = await api.post<AdminFlagResponse>(`/v1/projects/${projectId}/flags`, body)
      onClose()
      navigate(`/org/${orgId}/flags/${data.key}`)
    } catch (err: unknown) {
      setError(extractErrorMessage(err))
    } finally {
      setSaving(false)
    }
  }

  const header = (
    <div className="card-header" style={{ padding: '16px 20px', borderBottom: '1px solid var(--border)', display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
      <div className="card-title"><I.flag size={15} /> New feature flag</div>
      <button className="icon-btn" onClick={onClose}><I.x size={16} /></button>
    </div>
  )

  const footer = (
    <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
      <button type="button" className="btn" onClick={onClose}>Cancel</button>
      <button type="submit" className="btn primary" form="create-flag-form" disabled={saving}>
        {saving ? 'Creating…' : <><I.plus size={13} /> Create flag</>}
      </button>
    </div>
  )

  return (
    <Modal isOpen onClose={onClose} size="md" title={header} footer={footer}>
      <form id="create-flag-form" onSubmit={submit} style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
        {error && (
          <div style={{ padding: '10px 14px', background: 'var(--danger-bg)', border: '1px solid rgba(196,43,28,0.3)', borderRadius: 6, color: 'var(--danger)', fontSize: 13 }}>
            <I.alert size={13} style={{ verticalAlign: 'middle', marginRight: 6 }} />{error}
          </div>
        )}

        <div>
          <label className="label">Name <span style={{ color: 'var(--danger)' }}>*</span></label>
          <input className="input" style={{ width: '100%' }} placeholder="e.g. Dark Mode Beta"
            value={name} onChange={(e) => setName(e.target.value)} autoFocus />
        </div>

        <div>
          <label className="label">Key <span style={{ color: 'var(--danger)' }}>*</span></label>
          <input
            className="input" style={{ width: '100%', fontFamily: 'var(--font-mono)' }}
            placeholder="e.g. dark-mode-beta"
            value={key}
            onChange={(e) => { setKey(e.target.value); setKeyEdited(true) }}
          />
          <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 4 }}>
            Immutable after creation. Used in SDK calls.
          </div>
        </div>

        <div>
          <label className="label">Description</label>
          <textarea className="input" style={{ width: '100%', minHeight: 64, resize: 'vertical' }}
            placeholder="Optional description of what this flag controls"
            value={description} onChange={(e) => setDescription(e.target.value)} />
        </div>

        <div>
          <label className="label">Flag type</label>
          <select className="input" style={{ width: '100%' }} value={flagType}
            onChange={(e) => setFlagType(e.target.value as FlagType)}>
            <option value="bool">Boolean — on/off toggle</option>
            <option value="string">String — text values</option>
            <option value="int">Integer — whole numbers</option>
            <option value="double">Double — decimal numbers</option>
            <option value="json">JSON — arbitrary objects</option>
          </select>
        </div>

        <div>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
            <label className="label" style={{ margin: 0 }}>Initial variants</label>
            <button type="button" className="btn sm" onClick={addVariant}><I.plus size={11} /> Add</button>
          </div>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
            {variants.map((v, i) => (
              <div key={i} style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                  <input className="input" style={{ width: 120, fontFamily: 'var(--font-mono)', fontSize: 12 }}
                    placeholder="name" value={v.key}
                    onChange={(e) => setVariantKey(i, e.target.value)} />
                  {flagType === 'bool' ? (
                    <select className="input" style={{ flex: 1, fontSize: 12 }}
                      value={v.value} onChange={(e) => setVariantValue(i, e.target.value)}>
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
                        borderColor: variantErrors[i] ? 'var(--danger)' : undefined,
                      }}
                      placeholder={flagType === 'json' ? '{}' : flagType === 'int' ? '42' : flagType === 'double' ? '3.14' : 'value'}
                      value={v.value}
                      onChange={(e) => setVariantValue(i, e.target.value)}
                    />
                  )}
                  <button type="button" className="icon-btn" style={{ color: variants.length <= 1 ? 'var(--fg-faint)' : 'var(--danger)' }}
                    disabled={variants.length <= 1} onClick={() => removeVariant(i)}>
                    <I.x size={12} />
                  </button>
                </div>
                {variantErrors[i] && (
                  <div style={{ fontSize: 11, color: 'var(--danger)', paddingLeft: 128 }}>
                    {variantErrors[i]}
                  </div>
                )}
              </div>
            ))}
          </div>
        </div>
      </form>
    </Modal>
  )
}
