import { useCallback, useEffect, useState } from 'react'
import { Formik, Form, Field, useField } from 'formik'
import {
  listBanditCampaigns,
  createBanditCampaign,
  stopBanditCampaign,
  type BanditCampaign,
} from '../../lib/api/bandit'
import { I } from '../../components/icons'
import { Modal } from '../../components/Modal'
import { ConfirmDialog } from '../../components/ConfirmDialog'
import { EmptyState } from '../../components/EmptyState'
import { LoadingSpinner } from '../../components/LoadingSpinner'
import { ErrorBanner } from '../../components/ErrorBanner'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { FormSubmit } from '../../components/form/FormSubmit'
import { extractErrorMessage } from '../../lib/errors'
import {
  buildCampaignConfig,
  campaignConfigSchema,
  campaignConfigSummary,
  campaignStatusBadge,
  isTerminalCampaign,
  VARIANT_DISCOVERY_OPTIONS,
  type CampaignFormValues,
} from './banditCampaignHelpers'

export interface FlagOption {
  flag_id: string
  key: string
  name: string
}

interface Props {
  envId: string
  flags: FlagOption[]
  canManage: boolean
}

// ─── Presentational table ─────────────────────────────────────────────────────

export function CampaignsTable({
  campaigns,
  flags,
  canManage,
  onStop,
  busy = false,
}: {
  campaigns: BanditCampaign[]
  flags: FlagOption[]
  canManage: boolean
  onStop: (c: BanditCampaign) => void
  busy?: boolean
}) {
  const flagKey = (id: string) => flags.find((f) => f.flag_id === id)?.key ?? id
  return (
    <table className="table">
      <thead>
        <tr>
          <th>Campaign</th>
          <th>Flag</th>
          <th>Status</th>
          <th>Iterations</th>
          <th>Config</th>
          {canManage && <th />}
        </tr>
      </thead>
      <tbody>
        {campaigns.map((c) => {
          const badge = campaignStatusBadge(c.status)
          return (
            <tr key={c.id}>
              <td style={{ fontWeight: 600 }}>{c.name}</td>
              <td><span className="mono-key">{flagKey(c.flag_id)}</span></td>
              <td><span className={badge.className}>{badge.label}</span></td>
              <td style={{ fontFamily: 'var(--font-mono)' }}>{c.iterations_spawned}</td>
              <td style={{ fontSize: 12, color: 'var(--fg-muted)' }}>{campaignConfigSummary(c.config)}</td>
              {canManage && (
                <td style={{ textAlign: 'right' }}>
                  {!isTerminalCampaign(c.status) && (
                    <button className="btn sm danger" disabled={busy} onClick={() => onStop(c)}>Stop</button>
                  )}
                </td>
              )}
            </tr>
          )
        })}
      </tbody>
    </table>
  )
}

// ─── Create modal ─────────────────────────────────────────────────────────────

function NumField({ name, label, step, placeholder }: { name: string; label: string; step?: string; placeholder?: string }) {
  const [, meta] = useField<string | number>(name)
  const isError = meta.touched && Boolean(meta.error)
  return (
    <div>
      <label htmlFor={name} className="label" style={{ display: 'block', marginBottom: 4 }}>{label}</label>
      <Field id={name} name={name} type="number" step={step} placeholder={placeholder} className="input" style={{ width: '100%', borderColor: isError ? 'var(--danger)' : undefined }} />
      {isError && <div style={{ fontSize: 11, color: 'var(--danger)', marginTop: 4 }}>{meta.error}</div>}
    </div>
  )
}

function CreateCampaignModal({ envId, flags, onClose, onCreated }: { envId: string; flags: FlagOption[]; onClose: () => void; onCreated: () => void }) {
  const initial: CampaignFormValues = {
    flag_id: flags[0]?.flag_id ?? '',
    name: '',
    max_iterations: 5,
    drift_threshold: 0.1,
    variant_discovery: 'winner_plus_new',
    max_total_units: '',
  }

  async function handleSubmit(values: CampaignFormValues, { setStatus }: { setStatus: (s: unknown) => void }) {
    try {
      await createBanditCampaign(envId, {
        flag_id: values.flag_id,
        name: values.name.trim(),
        config: buildCampaignConfig(values),
      })
      onCreated()
    } catch (err: unknown) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

  return (
    <Modal isOpen onClose={onClose} title="New bandit campaign" size="md">
      <Formik initialValues={initial} validationSchema={campaignConfigSchema} onSubmit={handleSubmit}>
        <Form style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
          <FormErrorBanner />
          <p style={{ fontSize: 12, color: 'var(--fg-muted)', margin: 0 }}>
            An autonomous campaign spawns successive bandit iterations on a flag, reopening
            exploration on drift, until it converges or hits its iteration / budget ceiling.
          </p>

          <div>
            <label htmlFor="flag_id" className="label" style={{ display: 'block', marginBottom: 4 }}>Flag</label>
            <Field as="select" id="flag_id" name="flag_id" className="input" style={{ width: '100%' }}>
              {flags.length === 0 && <option value="">No flags in this environment</option>}
              {flags.map((f) => <option key={f.flag_id} value={f.flag_id}>{f.key}</option>)}
            </Field>
          </div>

          <div>
            <label htmlFor="name" className="label" style={{ display: 'block', marginBottom: 4 }}>Name</label>
            <Field id="name" name="name" className="input" placeholder="Checkout optimisation" style={{ width: '100%' }} />
          </div>

          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12 }}>
            <NumField name="max_iterations" label="Max iterations" step="1" placeholder="5" />
            <NumField name="drift_threshold" label="Drift threshold (0–1)" step="0.01" placeholder="0.1" />
          </div>

          <div>
            <label htmlFor="variant_discovery" className="label" style={{ display: 'block', marginBottom: 4 }}>Variant discovery</label>
            <Field as="select" id="variant_discovery" name="variant_discovery" className="input" style={{ width: '100%' }}>
              {VARIANT_DISCOVERY_OPTIONS.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
            </Field>
          </div>

          <NumField name="max_total_units" label="Budget cap — max total units (optional)" step="1" placeholder="uncapped" />

          <div style={{ display: 'flex', gap: 8, paddingTop: 4 }}>
            <FormSubmit label="Create campaign" loadingLabel="Creating…" className="btn primary" fullWidth />
            <button type="button" className="btn" onClick={onClose}>Cancel</button>
          </div>
        </Form>
      </Formik>
    </Modal>
  )
}

// ─── Panel ──────────────────────────────────────────────────────────────────

/** Env-scoped bandit campaign management surface (list + create + stop). */
export function BanditCampaignsPanel({ envId, flags, canManage }: Props) {
  const [campaigns, setCampaigns] = useState<BanditCampaign[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [showCreate, setShowCreate] = useState(false)
  const [stopping, setStopping] = useState<BanditCampaign | null>(null)

  const load = useCallback(() => {
    setLoading(true)
    setError(null)
    const ctrl = new AbortController()
    listBanditCampaigns(envId, ctrl.signal)
      .then((res) => setCampaigns(res.campaigns ?? []))
      .catch((err) => { if (!ctrl.signal.aborted) setError(extractErrorMessage(err)) })
      .finally(() => { if (!ctrl.signal.aborted) setLoading(false) })
    return ctrl
  }, [envId])

  useEffect(() => {
    const ctrl = load()
    return () => ctrl.abort()
  }, [load])

  async function handleStop(c: BanditCampaign) {
    setBusy(true)
    setError(null)
    try {
      await stopBanditCampaign(envId, c.id)
      setStopping(null)
      load()
    } catch (err: unknown) {
      setError(extractErrorMessage(err))
      setStopping(null)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="card" style={{ marginTop: 20 }}>
      <div className="card-header">
        <div className="card-title"><I.zap size={14} /> Bandit campaigns</div>
        {canManage && (
          <button className="btn primary sm" disabled={busy || flags.length === 0} onClick={() => setShowCreate(true)}>
            <I.plus size={12} /> New campaign
          </button>
        )}
      </div>
      {error && <div style={{ padding: '0 16px' }}><ErrorBanner message={error} onDismiss={() => setError(null)} /></div>}
      {loading ? (
        <div style={{ padding: 24 }}><LoadingSpinner label="Loading campaigns…" /></div>
      ) : campaigns.length === 0 ? (
        <EmptyState
          icon={<I.zap size={20} />}
          title="No bandit campaigns"
          desc="Autonomous campaigns continuously optimise a flag by spawning successive bandit iterations on convergence or drift."
        />
      ) : (
        <CampaignsTable campaigns={campaigns} flags={flags} canManage={canManage} onStop={(c) => setStopping(c)} busy={busy} />
      )}

      {showCreate && (
        <CreateCampaignModal envId={envId} flags={flags} onClose={() => setShowCreate(false)} onCreated={() => { setShowCreate(false); load() }} />
      )}
      {stopping && (
        <ConfirmDialog
          title="Stop campaign"
          message={`Stop "${stopping.name}"? No further iterations will spawn. This cannot be undone.`}
          confirmLabel="Stop"
          confirmDanger
          onConfirm={() => void handleStop(stopping)}
          onCancel={() => setStopping(null)}
        />
      )}
    </div>
  )
}
