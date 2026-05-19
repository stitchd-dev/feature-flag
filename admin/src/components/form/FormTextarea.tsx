import { useField } from 'formik'
import type { TextareaHTMLAttributes } from 'react'

interface Props extends Omit<TextareaHTMLAttributes<HTMLTextAreaElement>, 'name'> {
  name: string
  label: string
  hint?: string
}

/**
 * FormTextarea — wraps a Formik <textarea>.
 * Renders: label → textarea → inline error → optional hint.
 */
export function FormTextarea({ name, label, hint, ...rest }: Props) {
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
      <textarea
        id={name}
        className="input"
        style={{
          width: '100%',
          minHeight: 80,
          resize: 'vertical',
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
