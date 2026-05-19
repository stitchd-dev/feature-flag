/**
 * SeedUser — two-mode Formik form for adding users to an organisation.
 *
 * "Add existing" — enter an email that already has a platform account.
 * "Create new"   — full form; creates a new platform user and adds them.
 */
import { useState, useEffect } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { Formik, Form } from 'formik'
import { api, getOrg } from '../../lib/api'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { FormField } from '../../components/form/FormField'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { FormSubmit } from '../../components/form/FormSubmit'
import { extractErrorMessage } from '../../lib/errors'
import { userInviteSchema } from '../../lib/validation/orgSchema'

interface CreatedUser {
  user_id: string
  email: string
  display_name: string
}

type Mode = 'existing' | 'new'
type OrgRole = 'org_admin' | 'member'

const ROLE_OPTIONS: { value: OrgRole; label: string; desc: string }[] = [
  { value: 'org_admin', label: 'Org Admin', desc: 'Can manage flags, experiments, members' },
  { value: 'member',    label: 'Member',    desc: 'Read-only access' },
]

interface FormValues {
  email: string
  display_name: string
  password: string
  org_role: OrgRole
  mode: Mode
}

export function SeedUser() {
  const { orgId } = useParams<{ orgId: string }>()
  const navigate = useNavigate()
  const [orgName, setOrgName] = useState<string | null>(null)
  const [mode, setMode] = useState<Mode>('existing')
  const [showPw, setShowPw] = useState(false)
  const [result, setResult] = useState<CreatedUser | null>(null)

  useEffect(() => {
    if (!orgId) return
    const controller = new AbortController()
    getOrg(orgId, controller.signal)
      .then((org) => setOrgName(org.org_name))
      .catch(() => { /* non-critical */ })
    return () => controller.abort()
  }, [orgId])

  const initialValues: FormValues = {
    email: '',
    display_name: '',
    password: '',
    org_role: 'org_admin',
    mode,
  }

  async function handleSubmit(
    values: FormValues,
    { setStatus, resetForm }: { setStatus: (s: unknown) => void; resetForm: () => void },
  ) {
    if (!orgId) return
    try {
      const body: Record<string, string> = { email: values.email.trim(), org_role: values.org_role }
      if (mode === 'new') {
        body.display_name = values.display_name.trim()
        body.password = values.password
      }
      const { data } = await api.post<CreatedUser>(`/v1/superadmin/orgs/${orgId}/users`, body)
      setResult(data)
      resetForm()
    } catch (err: unknown) {
      const msg = extractErrorMessage(err)
      if (mode === 'existing' && msg.toLowerCase().includes('password')) {
        setStatus({ error: `No platform account found for "${values.email.trim()}". Switch to Create New to register them first.` })
      } else {
        setStatus({ error: msg })
      }
    }
  }

  function switchMode(m: Mode) {
    setMode(m)
    setResult(null)
  }

  return (
    <div className="page-content">
      <PageHeader
        title="Seed User"
        subtitle={orgName ? `Add a user to "${orgName}"` : 'Add a user to this organisation'}
        actions={
          <button className="btn" onClick={() => navigate(`/superadmin/orgs/${orgId}`)}>
            Back to Org
          </button>
        }
      />

      {result && (
        <div className="card" style={{ padding: 20, marginBottom: 24, border: '1px solid #22c55e', background: '#f0fdf4' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 12 }}>
            <I.check size={18} style={{ color: '#16a34a' }} />
            <span style={{ fontWeight: 600, color: '#16a34a' }}>
              {mode === 'existing' ? 'User added to organisation!' : 'User created and added!'}
            </span>
          </div>
          <div style={{ display: 'grid', gridTemplateColumns: '120px 1fr', gap: '6px 0', fontSize: 13 }}>
            <span style={{ color: 'var(--fg-muted)' }}>User ID</span>
            <span className="mono-key">{result.user_id}</span>
            <span style={{ color: 'var(--fg-muted)' }}>Email</span>
            <span>{result.email}</span>
            <span style={{ color: 'var(--fg-muted)' }}>Name</span>
            <span>{result.display_name}</span>
          </div>
          <div style={{ marginTop: 14, display: 'flex', gap: 8 }}>
            <button className="btn primary sm" onClick={() => navigate(`/superadmin/orgs/${orgId}`)}>Back to Org</button>
            <button className="btn sm" onClick={() => setResult(null)}>Add Another</button>
          </div>
        </div>
      )}

      {!result && (
        <div className="card" style={{ padding: 24, maxWidth: 540 }}>
          {/* Mode tabs */}
          <div style={{ display: 'flex', gap: 0, marginBottom: 24, border: '1px solid var(--border)', borderRadius: 8, overflow: 'hidden' }}>
            {(['existing', 'new'] as Mode[]).map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => switchMode(m)}
                style={{
                  flex: 1, padding: '9px 0', fontSize: 13, fontWeight: 500,
                  background: mode === m ? 'var(--accent)' : 'transparent',
                  color: mode === m ? '#fff' : 'var(--fg-muted)',
                  border: 'none', cursor: 'pointer', transition: 'background 0.15s',
                }}
              >
                {m === 'existing' ? '＋ Add Existing User' : '✦ Create New User'}
              </button>
            ))}
          </div>

          <p style={{ fontSize: 12, color: 'var(--fg-muted)', marginBottom: 20, marginTop: -8 }}>
            {mode === 'existing'
              ? 'The user already has a Stitchd account. Enter their email to grant them access to this org.'
              : 'Create a brand-new platform account and add them to this org in one step.'}
          </p>

          <Formik
            key={mode}
            initialValues={{ ...initialValues, mode }}
            validationSchema={userInviteSchema}
            onSubmit={handleSubmit}
          >
            <Form style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
              <FormErrorBanner />

              <FormField name="email" label="Email" type="email" placeholder="user@example.com" autoFocus />

              {mode === 'new' && (
                <>
                  <FormField name="display_name" label="Display Name" type="text" placeholder="Jane Smith" />
                  <div>
                    <div style={{ position: 'relative' }}>
                      <FormField
                        name="password"
                        label="Password"
                        type={showPw ? 'text' : 'password'}
                        placeholder="Minimum 8 characters"
                        style={{ paddingRight: 40 }}
                      />
                      <button
                        type="button"
                        onClick={() => setShowPw((v) => !v)}
                        style={{ position: 'absolute', right: 10, bottom: 6, background: 'none', border: 'none', cursor: 'pointer', color: 'var(--fg-muted)' }}
                      >
                        {showPw ? <I.eyeOff size={14} /> : <I.eye size={14} />}
                      </button>
                    </div>
                  </div>
                </>
              )}

              {/* Role selector */}
              <div>
                <label className="label">Role in this Organisation</label>
                <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginTop: 6 }}>
                  {ROLE_OPTIONS.map((opt) => (
                    <label
                      key={opt.value}
                      style={{
                        display: 'flex', alignItems: 'center', gap: 12,
                        padding: '10px 14px', borderRadius: 8, cursor: 'pointer',
                        border: `1px solid var(--border)`,
                        background: 'transparent',
                        transition: 'border-color 0.15s, background 0.15s',
                      }}
                    >
                      <input
                        type="radio"
                        name="org_role"
                        value={opt.value}
                        defaultChecked={opt.value === 'org_admin'}
                        style={{ accentColor: 'var(--accent)' }}
                      />
                      <div>
                        <div style={{ fontWeight: 500, fontSize: 13 }}>{opt.label}</div>
                        <div style={{ fontSize: 11, color: 'var(--fg-muted)' }}>{opt.desc}</div>
                      </div>
                    </label>
                  ))}
                </div>
              </div>

              <div style={{ display: 'flex', gap: 8, paddingTop: 4 }}>
                <FormSubmit
                  label={mode === 'existing' ? 'Add to Organisation' : 'Create & Add'}
                  loadingLabel={mode === 'existing' ? 'Adding…' : 'Creating…'}
                  className="btn primary"
                  fullWidth
                />
                <button
                  type="button"
                  className="btn"
                  onClick={() => navigate(`/superadmin/orgs/${orgId}`)}
                >
                  Cancel
                </button>
              </div>
            </Form>
          </Formik>
        </div>
      )}
    </div>
  )
}
