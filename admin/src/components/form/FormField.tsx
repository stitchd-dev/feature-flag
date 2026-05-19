import { useField } from 'formik'
import type { HTMLInputTypeAttribute, InputHTMLAttributes } from 'react'

interface Props extends Omit<InputHTMLAttributes<HTMLInputElement>, 'name' | 'type'> {
  name: string
  label: string
  hint?: string
  type?: HTMLInputTypeAttribute
}

/**
 * FormField — wraps a Formik text/email/number/password input.
 * Renders: label → input → inline error → optional hint.
 */
export function FormField({ name, label, hint, type = 'text', ...rest }: Props) {
  const [field, meta] = useField<string>(name)
  const isError = meta.touched && Boolean(meta.error)

  return (
    <div>
      <label
        htmlFor={name}
        className="label"
        style={{ display: 'block', marginBottom: 4 }}
      >
        {label}
      </label>
      <input
        id={name}
        type={type}
        className="input"
        style={{
          width: '100%',
          borderColor: isError ? 'var(--danger)' : undefined,
        }}
        aria-describedby={isError ? `${name}-error` : hint ? `${name}-hint` : undefined}
        aria-invalid={isError}
        {...field}
        {...rest}
      />
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
