import { useState } from 'react'
import { Formik, Form, useField } from 'formik'
import { Modal } from '../../components/Modal'
import { I } from '../../components/icons'
import { FormField } from '../../components/form/FormField'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { FormSubmit } from '../../components/form/FormSubmit'
import { extractErrorMessage } from '../../lib/errors'
import { createOrgMember } from '../../lib/api'
import { addMemberSchema, ROLE_OPTIONS, type AddMemberFormValues } from './membersHelpers'

interface Props {
  orgId: string
  onClose: () => void
  onCreated: () => void
}

/**
 * Add-member modal. The backend `CreateUser` provisions a credentialed account
 * directly (not an email invite), so the form collects email, display name, an
 * initial password and the org role.
 */
export function AddMemberModal({ orgId, onClose, onCreated }: Props) {
  const [showPw, setShowPw] = useState(false)

  const initialValues: AddMemberFormValues = {
    email: '',
    display_name: '',
    password: '',
    org_role: 'org_member',
  }

  async function handleSubmit(
    values: AddMemberFormValues,
    { setStatus }: { setStatus: (s: unknown) => void },
  ) {
    try {
      await createOrgMember(orgId, {
        email: values.email.trim(),
        display_name: values.display_name.trim(),
        password: values.password,
        org_role: values.org_role,
      })
      onCreated()
    } catch (err: unknown) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

  return (
    <Modal isOpen onClose={onClose} title="Add member" size="md">
      <Formik
        initialValues={initialValues}
        validationSchema={addMemberSchema}
        validateOnMount={false}
        onSubmit={handleSubmit}
      >
        <Form style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
          <FormErrorBanner />
          <p style={{ fontSize: 12, color: 'var(--fg-muted)', margin: 0 }}>
            Creates a new platform account and adds it to this organisation. Share
            the initial password securely; the member can change it after signing in.
          </p>

          <FormField name="email" label="Email" type="email" placeholder="user@example.com" autoFocus />
          <FormField name="display_name" label="Display name" type="text" placeholder="Jane Smith" />

          <div style={{ position: 'relative' }}>
            <FormField
              name="password"
              label="Initial password"
              type={showPw ? 'text' : 'password'}
              placeholder="Minimum 8 characters"
              style={{ paddingRight: 40 }}
            />
            <button
              type="button"
              onClick={() => setShowPw((v) => !v)}
              style={{ position: 'absolute', right: 10, bottom: 8, background: 'none', border: 'none', cursor: 'pointer', color: 'var(--fg-muted)' }}
              aria-label={showPw ? 'Hide password' : 'Show password'}
            >
              {showPw ? <I.eyeOff size={14} /> : <I.eye size={14} />}
            </button>
          </div>

          <div>
            <label className="label" style={{ display: 'block', marginBottom: 6 }}>Role</label>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
              {ROLE_OPTIONS.map((opt) => (
                <label
                  key={opt.value}
                  style={{ display: 'flex', alignItems: 'center', gap: 12, padding: '10px 14px', borderRadius: 8, cursor: 'pointer', border: '1px solid var(--border)' }}
                >
                  <FieldRadio name="org_role" value={opt.value} defaultChecked={opt.value === 'org_member'} />
                  <div>
                    <div style={{ fontWeight: 500, fontSize: 13 }}>{opt.label}</div>
                    <div style={{ fontSize: 11, color: 'var(--fg-muted)' }}>{opt.desc}</div>
                  </div>
                </label>
              ))}
            </div>
          </div>

          <div style={{ display: 'flex', gap: 8, paddingTop: 4 }}>
            <FormSubmit label="Add member" loadingLabel="Adding…" className="btn primary" fullWidth />
            <button type="button" className="btn" onClick={onClose}>Cancel</button>
          </div>
        </Form>
      </Formik>
    </Modal>
  )
}

/** A Formik-bound radio input for the role selector. */
function FieldRadio({ name, value, defaultChecked }: { name: string; value: string; defaultChecked?: boolean }) {
  const [field] = useField({ name, type: 'radio', value })
  return (
    <input
      type="radio"
      {...field}
      defaultChecked={defaultChecked}
      style={{ accentColor: 'var(--accent)' }}
    />
  )
}
