import { useState } from 'react'
import { I } from '../../components/icons'
import { ConfirmDialog } from '../../components/ConfirmDialog'
import { transitionExperiment, type ExperimentSummary } from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'
import { allowedTransitions, type TransitionAction } from './lifecycleHelpers'

interface Props {
  envId: string
  experiment: ExperimentSummary
  canManage: boolean
  onUpdated: (exp: ExperimentSummary) => void
  onError: (message: string) => void
}

/**
 * Renders the valid status-transition buttons for an experiment and performs
 * them via the transitions endpoint. Only valid transitions for the current
 * status are shown; controls are hidden when the viewer cannot manage the org.
 */
export function LifecycleActions({ envId, experiment, canManage, onUpdated, onError }: Props) {
  const [pending, setPending] = useState<TransitionAction | null>(null)
  const [busy, setBusy] = useState(false)

  const actions = allowedTransitions(experiment.status)
  if (!canManage || actions.length === 0) return null

  async function run(action: TransitionAction) {
    setBusy(true)
    try {
      const updated = await transitionExperiment(envId, experiment.key, action.target)
      onUpdated(updated)
      setPending(null)
    } catch (err: unknown) {
      onError(extractErrorMessage(err))
      setPending(null)
    } finally {
      setBusy(false)
    }
  }

  const icon = (a: TransitionAction) =>
    a.target === 'paused' ? <I.pause size={13} />
      : a.target === 'concluded' ? <I.check size={13} />
      : <I.play size={13} />

  return (
    <>
      {actions.map((a) => (
        <button
          key={a.target + a.label}
          className={`btn ${a.danger ? 'danger' : ''}`}
          disabled={busy}
          onClick={() => setPending(a)}
        >
          {icon(a)} {a.label}
        </button>
      ))}
      {pending && (
        <ConfirmDialog
          title={`${pending.label} experiment`}
          message={pending.confirm}
          confirmLabel={pending.label}
          confirmDanger={pending.danger}
          onConfirm={() => void run(pending)}
          onCancel={() => setPending(null)}
        />
      )}
    </>
  )
}
