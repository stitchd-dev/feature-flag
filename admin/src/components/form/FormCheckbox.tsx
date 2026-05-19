import { useField } from 'formik'

interface Props {
  name: string
  label: string
  hint?: string
  disabled?: boolean
}

/**
 * FormCheckbox — wraps a Formik boolean checkbox.
 * Renders: checkbox + label → optional hint → inline error.
 */
export function FormCheckbox({ name, label, hint, disabled }: Props) {
  const [field, meta] = useField<boolean>({ name, type: 'checkbox' })
  const isError = meta.touched && Boolean(meta.error)

  return (
    <div>
      <label
        htmlFor={name}
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          cursor: disabled ? 'not-allowed' : 'pointer',
          fontSize: 13,
        }}
      >
        <input
          id={name}
          type="checkbox"
          disabled={disabled}
          aria-describedby={isError ? `${name}-error` : hint ? `${name}-hint` : undefined}
          aria-invalid={isError}
          name={field.name}
          checked={field.value}
          onChange={field.onChange}
          onBlur={field.onBlur}
        />
        {label}
      </label>
      {isError && (
        <div
          id={`${name}-error`}
          role="alert"
          style={{ fontSize: 12, color: 'var(--danger)', marginTop: 4 }}
        >
          {meta.error}
        </div>
      )}
      {!isError && hint && (
        <div
          id={`${name}-hint`}
          style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 4 }}
        >
          {hint}
        </div>
      )}
    </div>
  )
}
