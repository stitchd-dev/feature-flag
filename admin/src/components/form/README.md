# Formik Form Patterns

This directory contains the standard form primitives for all admin UI forms.
Every form in the admin uses Formik + Yup. Do not use ad-hoc `useState` for form field state.

---

## How to build a form

```tsx
import { Formik, Form } from 'formik'
import { FormField } from '../components/form/FormField'
import { FormErrorBanner } from '../components/form/FormErrorBanner'
import { FormSubmit } from '../components/form/FormSubmit'
import { mySchema } from '../lib/validation/mySchema'

interface Values { name: string; description: string }

function MyForm() {
  async function handleSubmit(
    values: Values,
    { setStatus }: { setStatus: (s: unknown) => void },
  ) {
    try {
      await api.post('/v1/things', values)
      onClose()
    } catch (err) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

  return (
    <Formik
      initialValues={{ name: '', description: '' }}
      validationSchema={mySchema}
      onSubmit={handleSubmit}
    >
      <Form style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
        <FormErrorBanner />       {/* shows status.error from API failures */}
        <FormField name="name" label="Name" />
        <FormField name="description" label="Description" />
        <FormSubmit label="Create" loadingLabel="Creating…" />
      </Form>
    </Formik>
  )
}
```

---

## Primitives

| Component | Purpose |
|-----------|---------|
| `FormField` | Text / email / password / number input with label + error + hint |
| `FormSelect` | `<select>` with options array |
| `FormCheckbox` | Boolean checkbox |
| `FormTextarea` | Resizable textarea |
| `FormSubmit` | Submit button; reads `isSubmitting` from Formik context; auto-disables |
| `FormErrorBanner` | Reads `status.error` from Formik context; renders API error at form top |

---

## Validation patterns

Schemas live in `admin/src/lib/validation/`. One schema file per domain.

```ts
// flagSchema.ts
import * as Yup from 'yup'

export const flagSchema = Yup.object({
  name: Yup.string().trim().min(1, 'Name is required').required(),
  key: Yup.string().trim().matches(/^[a-z0-9][a-z0-9_-]*$/, 'Invalid key format').required(),
})

export type FlagFormValues = Yup.InferType<typeof flagSchema>
```

Rules:
- Error message keys must align to API field names (e.g. `name`, `key`, not `flagName`).
- Use `.trim()` before `.min(1)` so whitespace-only is caught.
- Derive TypeScript types from the schema with `Yup.InferType<typeof schema>`.

---

## Submit-error pattern

API errors must surface via `formik.setStatus({ error: message })`, not a separate `useState`:

```ts
async function handleSubmit(values, { setStatus }) {
  try {
    await api.post('/v1/flags', values)
  } catch (err) {
    // extractErrorMessage handles Axios + plain Error shapes
    setStatus({ error: extractErrorMessage(err) })
  }
}
```

Place `<FormErrorBanner />` at the top of your `<Form>` — it renders only when `status.error` is set.

---

## Async validation example (flag key uniqueness)

```ts
import * as Yup from 'yup'

const schemaWithAsyncKey = flagSchema.shape({
  key: Yup.string()
    .trim()
    .min(1, 'Key is required')
    .matches(/^[a-z0-9][a-z0-9_-]*$/, 'Invalid format')
    .test('key-unique', 'This key is already taken', async (value) => {
      if (!value || value.length < 2) return true          // skip if too short
      try {
        await api.get(`/v1/projects/${projectId}/flags/${value}`)
        return false                                        // 200 = key exists
      } catch {
        return true                                         // 404 = available
      }
    })
    .required(),
})
```

Key points:
- Wrap the base schema with `.shape({...})` to override a single field.
- Return `true` (valid) for 404 (not found = key is free), `false` (invalid) for 200.
- Avoid running the async test until the value meets minimum format requirements.
- Use `validateOnChange={false}` on the `<Formik>` to avoid spamming the API on every keystroke.

---

## Testing patterns

The vitest environment is `node` (no DOM). Use **logic-mirror** tests that extract and test
pure logic rather than rendering components.

```ts
// form.test.ts
import { describe, it, expect } from 'vitest'

// Mirror the field error logic
function getFieldAriaProps(touched: boolean, error: string | undefined, name: string) {
  const isError = touched && Boolean(error)
  return {
    'aria-invalid': isError,
    'aria-describedby': isError ? `${name}-error` : undefined,
  }
}

describe('FormField aria props', () => {
  it('shows error when touched + error present', () => {
    expect(getFieldAriaProps(true, 'Required', 'name')['aria-invalid']).toBe(true)
  })
  it('no error when untouched', () => {
    expect(getFieldAriaProps(false, 'Required', 'name')['aria-invalid']).toBe(false)
  })
})
```

For Yup schemas, test with `.validate()` / `.isValid()` directly:

```ts
import { describe, it, expect } from 'vitest'
import { flagSchema } from '../lib/validation/flagSchema'

describe('flagSchema', () => {
  it('rejects empty name', async () => {
    await expect(flagSchema.validateAt('name', { name: '' })).rejects.toThrow()
  })
  it('accepts valid key', async () => {
    await expect(flagSchema.validateAt('key', { key: 'dark-mode-beta' })).resolves.toBeDefined()
  })
})
```

---

## Common gotchas

1. **FormCheckbox `value` type clash** — Formik's `useField({name, type:'checkbox'})` returns `value: boolean`. Spread `name`, `checked`, `onChange`, `onBlur` individually instead of `{...field}` to avoid the `value: boolean` vs `value: string` HTMLInput conflict.

2. **`enableReinitialize`** — Use when form `initialValues` depend on async-loaded data (e.g. `EditSegmentModal` re-fetches from API before showing). Without this, Formik won't re-initialize after the data arrives.

3. **Formik render-prop for footer buttons** — When a modal's footer contains the submit button but lives outside `<Form>`, use either `form="form-id"` on the button (HTML form attribute) or Formik's render-prop pattern to access `isSubmitting` in the footer.

4. **Mode-switching forms** — When a form has modes (e.g. existing/new user), add `key={mode}` to the `<Formik>` element to force a full remount and reset validation state when the mode changes.

5. **`validateOnChange: false` for async validation** — Always set this when using async `.test()` validators to avoid API calls on every keystroke.
