import { Modal } from '../Modal'
import { I } from '../icons'
import type { DependencyExistsError } from '../../lib/types'

interface Props {
  /** What the user tried to do, e.g. "archive" / "delete". */
  action: string
  /** The entity kind, e.g. "flag" / "segment" / "experiment". */
  entityLabel: string
  /** The structured 409 body returned by the gateway. */
  error: DependencyExistsError
  onClose: () => void
}

/**
 * Delete-blocked UX (flag_lifecycle_20260604, Phase 8.4). Shown when a
 * delete/archive returns `409 dependency_exists`: lists the blocking dependents
 * and explains that the references must be removed before the entity can be
 * removed.
 */
export function DeleteBlockedDialog({ action, entityLabel, error, onClose }: Props) {
  const footer = (
    <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
      <button className="btn primary" onClick={onClose}>
        Got it
      </button>
    </div>
  )

  return (
    <Modal isOpen onClose={onClose} size="sm" footer={footer}>
      <div style={{ display: 'flex', alignItems: 'flex-start', gap: 12 }}>
        <div style={{ padding: 8, background: 'var(--danger-bg)', borderRadius: 8 }}>
          <I.alert size={18} stroke="var(--danger)" />
        </div>
        <div>
          <div style={{ fontSize: 15, fontWeight: 600, marginBottom: 4 }}>
            Can’t {action} this {entityLabel}
          </div>
          <div style={{ fontSize: 13, color: 'var(--fg-muted)', lineHeight: 1.5, marginBottom: 10 }}>
            {error.message} Remove the references below first, then try again.
          </div>
          {error.dependents.length > 0 && (
            <div>
              <div style={{ fontSize: 11, fontWeight: 700, color: 'var(--fg-muted)', marginBottom: 4 }}>
                Blocking dependents ({error.dependents.length})
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                {error.dependents.map((d) => (
                  <code
                    key={d}
                    style={{
                      fontSize: 12,
                      padding: '3px 8px',
                      borderRadius: 4,
                      background: 'var(--bg-sunken)',
                      border: '1px solid var(--border-faint)',
                      wordBreak: 'break-all',
                    }}
                  >
                    {d}
                  </code>
                ))}
              </div>
            </div>
          )}
        </div>
      </div>
    </Modal>
  )
}
