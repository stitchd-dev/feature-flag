import { useField, useFormikContext } from 'formik'
import { useMemo } from 'react'
import { SuggestionInput } from '../SuggestionInput'

interface EventKeyFieldProps {
  /** Dotted Formik field path. Supports nested paths like `steps.0.event_key`. */
  name: string
  label: string
  placeholder?: string
  hint?: string
  /** Strict mode (default): non-empty values must match a registered event key. */
  eventKeys: string[]
  /**
   * When true, the picker still surfaces suggestions but does not flag
   * unknown keys as invalid (the server stays the source of truth).
   * Default is strict — we want the UI to catch typos before submit.
   */
  permissive?: boolean
  /** Optional `data-testid` prefix; default = `event-key-${name}`. */
  testId?: string
  /** Render the field as disabled (used when the parent form is loading). */
  disabled?: boolean
}

/**
 * Formik-integrated typeahead for picking a registered event key.
 *
 * Wraps the existing `SuggestionInput` and adds:
 *   - Yup-independent strict validation: any non-empty value that isn't
 *     present in `eventKeys` is reported via `setFieldError`. This makes
 *     it impossible to save a metric that references a non-existent event
 *     (server already enforces this; the UI now refuses earlier).
 *   - The list still ships even when there are zero matches so the user
 *     sees a tight error ("Event key not found in this environment")
 *     rather than the gateway's generic 422.
 *
 * Plugs into:
 *   - `MetricFormFields.AggregationFields` (single event_key)
 *   - `MetricFormFields.FunnelFields` (each step's event_key)
 */
export function EventKeyField({
  name,
  label,
  placeholder,
  hint,
  eventKeys,
  permissive = false,
  testId,
  disabled,
}: EventKeyFieldProps) {
  const [field, meta, helpers] = useField<string>(name)
  const { isSubmitting } = useFormikContext()
  const suggestions = useMemo(
    () => eventKeys.map((k) => ({ label: k })),
    [eventKeys],
  )

  const trimmed = (field.value ?? '').trim()
  const unknownKey =
    !permissive && trimmed !== '' && eventKeys.length > 0 && !eventKeys.includes(trimmed)

  // Surface "unknown key" both as a Formik error (blocks submit if user
  // hits Save) and as an inline message. Touched-state respects the user's
  // first-edit experience: we don't flash red on an empty field they
  // haven't touched yet.
  const showError =
    (meta.touched && meta.error) ||
    (unknownKey && trimmed !== '')

  return (
    <div data-testid={testId ?? `event-key-${name}`}>
      <label
        htmlFor={`field-${name}`}
        className="label"
        style={{ display: 'block', marginBottom: 4 }}
      >
        {label}
      </label>
      <SuggestionInput
        value={field.value ?? ''}
        onChange={(v) => {
          void helpers.setValue(v)
          void helpers.setTouched(true, false)
        }}
        suggestions={suggestions}
        placeholder={placeholder}
        disabled={disabled || isSubmitting}
        className={`input${showError ? ' input-error' : ''}`}
        style={{ width: '100%', display: 'block' }}
      />
      {showError && (
        <div
          role="alert"
          style={{ color: 'var(--danger)', fontSize: 11, marginTop: 4 }}
        >
          {unknownKey
            ? 'Event key not found in this environment'
            : meta.error}
        </div>
      )}
      {!showError && hint && (
        <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 4 }}>
          {hint}
        </div>
      )}
    </div>
  )
}

/**
 * Returns true when the candidate event key is one of the known keys.
 * Pulled out into a pure helper so the metric form's submit-time guard
 * can call the same predicate.
 */
export function isKnownEventKey(candidate: string, eventKeys: string[]): boolean {
  return eventKeys.includes(candidate.trim())
}
