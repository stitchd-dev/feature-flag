import { useFormikContext } from 'formik'

interface Props {
  label: string
  loadingLabel?: string
  className?: string
  /** Full button width override (default: false) */
  fullWidth?: boolean
}

/**
 * FormSubmit — reads `isSubmitting` from Formik context.
 * Shows loadingLabel while submission is in-flight, disables button.
 */
export function FormSubmit({ label, loadingLabel, className = 'btn primary', fullWidth = false }: Props) {
  const { isSubmitting } = useFormikContext()

  return (
    <button
      type="submit"
      className={className}
      disabled={isSubmitting}
      style={fullWidth ? { width: '100%' } : undefined}
    >
      {isSubmitting ? (loadingLabel ?? `${label}…`) : label}
    </button>
  )
}
