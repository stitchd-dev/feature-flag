import { useState } from 'react'
import { I } from '../../components/icons'
import { Modal } from '../../components/Modal'
import { api } from '../../lib/api'
import { extractErrorMessage } from '../../lib/errors'

import { useOrgContext } from '../../context/OrgContext'

interface Props {
  /** The event being archived — used in the confirmation copy + the DELETE URL. */
  eventKey: string
  onClose: () => void
  /** Fired after a successful DELETE — parent should refresh / navigate away. */
  onArchived: () => void
}

/**
 * Confirm-then-DELETE modal for an event definition. Soft-deletes server-
 * side (sets `deleted_at`); archived events reject new firings at the
 * gateway with HTTP 410, while existing ClickHouse data is preserved.
 *
 * The DELETE call is fire-and-confirm — no payload, no optimistic locking
 * (delete is idempotent: a second delete on a soft-deleted row also 410s
 * but the UI gets a clean "already archived" experience).
 */
export function ArchiveEventModal({ eventKey, onClose, onArchived }: Props) {
  const { envId } = useOrgContext()
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleConfirm() {
    setSubmitting(true)
    setError(null)
    try {
      await api.delete(`/v1/events/${encodeURIComponent(eventKey)}?env_id=${envId ?? ''}`)
      onArchived()
    } catch (err: unknown) {
      setError(extractErrorMessage(err))
    } finally {
      setSubmitting(false)
    }
  }

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
        <I.trash size={15} /> Archive event
      </div>
      <button type="button" className="icon-btn" onClick={onClose}>
        <I.x size={16} />
      </button>
    </div>
  )

  const footer = (
    <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end', paddingTop: 4 }}>
      <button type="button" className="btn" onClick={onClose} disabled={submitting}>
        Cancel
      </button>
      <button
        type="button"
        className="btn primary"
        onClick={handleConfirm}
        disabled={submitting}
        data-testid="archive-confirm"
      >
        {submitting ? 'Archiving…' : 'Archive'}
      </button>
    </div>
  )

  return (
    <Modal isOpen onClose={onClose} size="sm" title={header} footer={footer}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 12, padding: '4px 0' }}>
        {error && (
          <div
            className="banner danger"
            style={{
              padding: '8px 12px',
              background: 'var(--danger-bg, rgba(229,57,53,0.08))',
              borderLeft: '3px solid var(--danger)',
              fontSize: 12,
            }}
          >
            {error}
          </div>
        )}
        <p style={{ fontSize: 14, margin: 0 }}>
          Archive{' '}
          <code style={{ fontFamily: 'var(--font-mono)' }} data-testid="archive-event-key">
            {eventKey}
          </code>
          ?
        </p>
        <p style={{ fontSize: 12, color: 'var(--fg-muted)', margin: 0, lineHeight: 1.4 }}>
          Archived events reject new firings with <code style={{ fontFamily: 'var(--font-mono)' }}>HTTP 410</code>.
          Existing data in ClickHouse is preserved and remains queryable. This
          action can be reversed by re-creating the event with the same key.
        </p>
      </div>
    </Modal>
  )
}
