import { useFormikContext } from 'formik'

interface Props {
  label: string
  loadingLabel?: string
  className?: string
  /** Full button width override (default: false) */
  fullWidth?: boolean
  /**
   * When the submit button renders OUTSIDE its <Form> (e.g. in a Modal
   * footer slot), set this to the form's `id` so the browser still wires
   * up the click → submit relationship via the HTML `form` attribute.
   * Without this, `<button type="submit">` only triggers the nearest
   * enclosing <form>, which doesn't exist when the button lives in a
   * sibling subtree (Modal renders header / body / footer separately).
   */
  form?: string
}

/**
 * FormSubmit — reads `isSubmitting` from Formik context.
 * Shows loadingLabel while submission is in-flight, disables button.
 *
 * If the button is rendered outside its <Form> (typical when placed in a
 * Modal's `footer` prop), pass `form="<form-id>"` to keep the submit
 * relationship intact via the HTML `form` attribute.
 */
export function FormSubmit({ label, loadingLabel, className = 'btn primary', fullWidth = false, form }: Props) {
  const { isSubmitting } = useFormikContext()

  return (
    <button
      type="submit"
      form={form}
      className={className}
      disabled={isSubmitting}
      style={fullWidth ? { width: '100%' } : undefined}
    >
      {isSubmitting ? (loadingLabel ?? `${label}…`) : label}
    </button>
  )
}
