/**
 * SeedUser — regression test for feature-flag-042.
 *
 * Bug: 'Email is required' error rendered on initial mount, before the
 * user touched / submitted the form.
 *
 * Fix invariant: SSR'ing the form with `userInviteSchema` MUST NOT emit
 * any Yup error strings, because none of the fields are touched yet and
 * the submit hasn't fired. The component uses `FormField`, which gates
 * `meta.error` on `meta.touched && Boolean(meta.error)` — this test pins
 * that contract end-to-end (schema + form + primitives) so future
 * refactors can't regress it.
 *
 * Admin vitest env is `environment: 'node'`, so we use react-dom/server
 * + raw-import shape assertions per the project test convention.
 */
import { describe, it, expect } from 'vitest'
import { renderToString } from 'react-dom/server'
import { Formik, Form } from 'formik'
import { FormField } from '../../components/form/FormField'
import { userInviteSchema } from '../../lib/validation/orgSchema'
import SOURCE from './SeedUser.tsx?raw'

describe('SeedUser — eager-error regression (feature-flag-042)', () => {
  it('renders the email field without any Yup error string on initial mount', () => {
    // Mirror the Formik shell used in SeedUser.tsx exactly:
    // - same initial values (all empty)
    // - same validationSchema
    // - mode='existing' (default tab on entry)
    const html = renderToString(
      <Formik
        initialValues={{
          email: '',
          display_name: '',
          password: '',
          org_role: 'org_admin',
          mode: 'existing',
        }}
        validationSchema={userInviteSchema}
        onSubmit={() => {}}
      >
        <Form>
          <FormField name="email" label="Email" type="email" />
        </Form>
      </Formik>,
    )
    // None of the Yup messages from userInviteSchema should appear before
    // the user touches a field or submits the form.
    expect(html).not.toMatch(/Email is required/)
    expect(html).not.toMatch(/Must be a valid email address/)
  })

  it('renders the email field for "new" mode without showing display_name / password required errors', () => {
    // Same as above but with mode='new', which adds two more required fields.
    const html = renderToString(
      <Formik
        initialValues={{
          email: '',
          display_name: '',
          password: '',
          org_role: 'org_admin',
          mode: 'new',
        }}
        validationSchema={userInviteSchema}
        onSubmit={() => {}}
      >
        <Form>
          <FormField name="email" label="Email" type="email" />
          <FormField name="display_name" label="Display Name" />
          <FormField name="password" label="Password" type="password" />
        </Form>
      </Formik>,
    )
    expect(html).not.toMatch(/Email is required/)
    expect(html).not.toMatch(/Display name is required/)
    expect(html).not.toMatch(/Password is required/)
    expect(html).not.toMatch(/Password must be at least 8 characters/)
  })

  it('uses the touch-gated FormField primitive (no direct errors.<field> render in source)', () => {
    // Belt-and-braces: ensure the source itself never bypasses FormField by
    // rendering `errors.<field>` or `<ErrorMessage>` directly. If a future
    // edit reintroduces an eager-error pattern, this test fires.
    expect(SOURCE).not.toMatch(/errors\.email/)
    expect(SOURCE).not.toMatch(/errors\.display_name/)
    expect(SOURCE).not.toMatch(/errors\.password/)
    expect(SOURCE).not.toMatch(/<ErrorMessage\b/)
    // The form still uses the touched-gated primitive — assertions on shape:
    expect(SOURCE).toMatch(/FormField\s+name="email"/)
  })
})
