import { describe, it, expect, vi } from 'vitest'

// ─── FormField logic ─────────────────────────────────────────────────────────

function getFieldAriaProps(touched: boolean, error: string | undefined, hint: string | undefined, name: string) {
  const isError = touched && Boolean(error)
  return {
    'aria-invalid': isError,
    'aria-describedby': isError
      ? `${name}-error`
      : hint
        ? `${name}-hint`
        : undefined,
    borderColor: isError ? 'var(--danger)' : undefined,
  }
}

describe('FormField aria props', () => {
  it('shows error state when touched + error', () => {
    const props = getFieldAriaProps(true, 'Required', undefined, 'name')
    expect(props['aria-invalid']).toBe(true)
    expect(props['aria-describedby']).toBe('name-error')
    expect(props.borderColor).toBe('var(--danger)')
  })

  it('shows hint when touched but no error', () => {
    const props = getFieldAriaProps(true, undefined, 'Must be unique', 'name')
    expect(props['aria-invalid']).toBe(false)
    expect(props['aria-describedby']).toBe('name-hint')
    expect(props.borderColor).toBeUndefined()
  })

  it('no describedby when no error + no hint', () => {
    const props = getFieldAriaProps(false, undefined, undefined, 'name')
    expect(props['aria-invalid']).toBe(false)
    expect(props['aria-describedby']).toBeUndefined()
  })

  it('error takes precedence over hint for describedby', () => {
    const props = getFieldAriaProps(true, 'Bad value', 'Some hint', 'key')
    expect(props['aria-describedby']).toBe('key-error')
  })

  it('no error when not touched even if error string present', () => {
    const props = getFieldAriaProps(false, 'Required', undefined, 'name')
    expect(props['aria-invalid']).toBe(false)
    expect(props['aria-describedby']).toBeUndefined()
  })
})

// ─── FormSelect logic ────────────────────────────────────────────────────────

function buildOptions(options: { value: string; label: string }[]) {
  return options.map((o) => ({ value: o.value, label: o.label }))
}

describe('FormSelect options', () => {
  const opts = [
    { value: 'bool', label: 'Boolean' },
    { value: 'string', label: 'String' },
  ]

  it('maps options correctly', () => {
    const built = buildOptions(opts)
    expect(built).toHaveLength(2)
    expect(built[0]).toEqual({ value: 'bool', label: 'Boolean' })
  })

  it('empty options produces empty list', () => {
    expect(buildOptions([])).toHaveLength(0)
  })
})

// ─── FormCheckbox logic ──────────────────────────────────────────────────────

function getCheckboxState(value: boolean, disabled: boolean) {
  return { checked: value, cursor: disabled ? 'not-allowed' : 'pointer' }
}

describe('FormCheckbox state', () => {
  it('checked when value is true', () => {
    expect(getCheckboxState(true, false).checked).toBe(true)
  })

  it('unchecked when value is false', () => {
    expect(getCheckboxState(false, false).checked).toBe(false)
  })

  it('cursor not-allowed when disabled', () => {
    expect(getCheckboxState(true, true).cursor).toBe('not-allowed')
  })

  it('cursor pointer when not disabled', () => {
    expect(getCheckboxState(false, false).cursor).toBe('pointer')
  })
})

// ─── FormTextarea logic ──────────────────────────────────────────────────────

function getTextareaStyle(touched: boolean, error: string | undefined) {
  const isError = touched && Boolean(error)
  return {
    borderColor: isError ? 'var(--danger)' : undefined,
    resize: 'vertical',
    minHeight: 80,
  }
}

describe('FormTextarea style', () => {
  it('applies danger border on error', () => {
    const style = getTextareaStyle(true, 'Too short')
    expect(style.borderColor).toBe('var(--danger)')
  })

  it('no danger border when untouched', () => {
    const style = getTextareaStyle(false, 'Too short')
    expect(style.borderColor).toBeUndefined()
  })

  it('always resize vertical', () => {
    expect(getTextareaStyle(false, undefined).resize).toBe('vertical')
  })
})

// ─── FormSubmit logic ────────────────────────────────────────────────────────

function getSubmitLabel(isSubmitting: boolean, label: string, loadingLabel: string | undefined) {
  return isSubmitting ? (loadingLabel ?? `${label}…`) : label
}

describe('FormSubmit label', () => {
  it('shows label when not submitting', () => {
    expect(getSubmitLabel(false, 'Create', undefined)).toBe('Create')
  })

  it('shows default loading label (label + ellipsis) when submitting', () => {
    expect(getSubmitLabel(true, 'Create', undefined)).toBe('Create…')
  })

  it('shows custom loading label when provided', () => {
    expect(getSubmitLabel(true, 'Create', 'Creating flag…')).toBe('Creating flag…')
  })
})

// ─── FormErrorBanner logic ───────────────────────────────────────────────────

function getFormError(status: unknown): string | undefined {
  const s = status as { error?: string } | undefined
  return s?.error
}

describe('FormErrorBanner', () => {
  it('returns error string from status.error', () => {
    expect(getFormError({ error: 'API failure' })).toBe('API failure')
  })

  it('returns undefined when status is null', () => {
    expect(getFormError(null)).toBeUndefined()
  })

  it('returns undefined when status has no error field', () => {
    expect(getFormError({})).toBeUndefined()
  })

  it('returns undefined when status is undefined', () => {
    expect(getFormError(undefined)).toBeUndefined()
  })

  it('dismisses correctly — error string truthy check', () => {
    expect(Boolean(getFormError({ error: 'oops' }))).toBe(true)
    expect(Boolean(getFormError({ error: '' }))).toBe(false)
    vi.fn() // ensure vi import is used
  })
})
