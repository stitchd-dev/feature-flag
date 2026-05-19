import { useFormikContext } from 'formik'
import { I } from '../icons'

interface FormStatus {
  error?: string
}

/**
 * FormErrorBanner — reads `status.error` from Formik context.
 * Render this at the top of a <Form> to show API/submit errors.
 * Set via `formik.setStatus({ error: extractErrorMessage(err) })`.
 */
export function FormErrorBanner() {
  const { status } = useFormikContext<unknown>()
  const formStatus = status as FormStatus | undefined
  const error = formStatus?.error

  if (!error) return null

  return (
    <div
      role="alert"
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        padding: '10px 14px',
        background: 'var(--danger-bg)',
        border: '1px solid rgba(196,43,28,0.3)',
        borderRadius: 6,
        color: 'var(--danger)',
        fontSize: 13,
      }}
    >
      <I.alert size={13} style={{ flexShrink: 0 }} />
      <span>{error}</span>
    </div>
  )
}
