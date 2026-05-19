import { useField } from 'formik'
import type { SelectHTMLAttributes } from 'react'

interface Option {
  value: string
  label: string
}

interface Props extends Omit<SelectHTMLAttributes<HTMLSelectElement>, 'name'> {
  name: string
  label: string
  options: Option[]
  hint?: string
}

/**
 * FormSelect — wraps a Formik <select>.
 * Renders: label → select → inline error → optional hint.
 */
export function FormSelect({ name, label, options, hint, ...rest }: Props) {
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
      <select
        id={name}
        className="input"
        style={{
          width: '100%',
          borderColor: isError ? 'var(--danger)' : undefined,
        }}
        aria-describedby={isError ? `${name}-error` : hint ? `${name}-hint` : undefined}
        aria-invalid={isError}
        {...field}
        {...rest}
      >
        {options.map((opt) => (
          <option key={opt.value} value={opt.value}>
            {opt.label}
          </option>
        ))}
      </select>
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
