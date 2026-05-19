import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { Formik, Form } from 'formik'
import { I } from '../components/icons'
import { StitchdMark } from '../components/primitives'
import { FormField } from '../components/form/FormField'
import { FormErrorBanner } from '../components/form/FormErrorBanner'
import { FormSubmit } from '../components/form/FormSubmit'
import { loginWithPassword, initiateOidc, initiateSaml, listUserOrgs } from '../lib/api'
import { auth } from '../lib/auth'
import { extractErrorMessage } from '../lib/errors'
import { loginSchema } from '../lib/validation/authSchema'
import type { LoginFormValues } from '../lib/validation/authSchema'

type AuthMethod = 'password' | 'oidc' | 'saml'

const DOCKER_SNIPPET = `$ docker compose up stitchd
✓ stitchd-gateway     :8080
✓ stitchd-flag-service
✓ stitchd-analytics-service
✓ stitchd-experimentation-service
✓ stitchd-segmentation-service
✓ stitchd-auth-service
─ ready in 1.4s`

export function LoginPage() {
  const navigate = useNavigate()
  const [method, setMethod] = useState<AuthMethod>('password')
  const [showPw, setShowPw] = useState(false)

  const initialValues: LoginFormValues = {
    email: '',
    password: '',
    org_id: '',
  }

  async function handleSubmit(
    values: LoginFormValues,
    { setStatus }: { setStatus: (s: unknown) => void },
  ) {
    try {
      if (method === 'password') {
        const res = await loginWithPassword(values.email, values.password, values.org_id || undefined)
        const isSystem = auth.decodeIsSystem(res.access_token)
        const roles = auth.decodeRoles(res.access_token)
        const permissions = auth.decodePermissions(res.access_token)
        auth.setSession({ token: res.access_token, refreshToken: res.refresh_token, orgId: res.org_id, isSystem, userId: res.user_id, roles, permissions })
        try {
          const orgs = await listUserOrgs()
          auth.setOrgs(orgs)
        } catch {
          auth.setOrgs([])
        }
        if (isSystem) {
          navigate('/superadmin')
        } else {
          auth.addOrgToHistory({ orgId: res.org_id, orgName: res.org_id })
          navigate(`/org/${res.org_id}`)
        }
      } else if (method === 'oidc') {
        const res = await initiateOidc(values.org_id ?? '')
        window.location.href = res.redirect_url
      } else {
        const res = await initiateSaml(values.org_id ?? '')
        window.location.href = res.redirect_url
      }
    } catch (err: unknown) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

  return (
    <div style={{ minHeight: '100%', display: 'grid', gridTemplateColumns: '1fr 1.05fr', background: 'var(--bg)' }}>
      {/* Left panel */}
      <div style={{ padding: '48px 56px', display: 'flex', flexDirection: 'column', justifyContent: 'space-between', borderRight: '1px solid var(--border)' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
          <StitchdMark size={32} radius={8} />
          <div className="brand-text" style={{ fontSize: 22 }}>Stitchd</div>
        </div>
        <div style={{ maxWidth: 420 }}>
          <div style={{ fontFamily: 'var(--font-display)', fontWeight: 800, fontSize: 36, lineHeight: 1.1, letterSpacing: '-0.025em', marginBottom: 12 }}>
            Feature flags &amp; experiments,<br />self-hosted.
          </div>
          <div style={{ color: 'var(--fg-muted)', fontSize: 15, lineHeight: 1.55, marginBottom: 24 }}>
            Sign in to your organization to manage flags, run rigorous A/B experiments, and ship without flinching.
          </div>
          <div className="code" style={{ fontSize: 11.5 }}>{DOCKER_SNIPPET}</div>
        </div>
        <div style={{ color: 'var(--fg-subtle)', fontSize: 12, fontFamily: 'var(--font-mono)' }}>
          self-hosted · v0.9.0 · 6 services healthy
        </div>
      </div>

      {/* Right panel */}
      <div style={{ display: 'grid', placeItems: 'center', padding: 48 }}>
        <div className="card" style={{ width: 420, padding: 32 }}>
          <div style={{ fontFamily: 'var(--font-display)', fontSize: 22, fontWeight: 800, letterSpacing: '-0.02em', marginBottom: 4 }}>Sign in</div>
          <div style={{ color: 'var(--fg-muted)', fontSize: 13, marginBottom: 20 }}>Use your work email to continue.</div>

          {/* Method tabs */}
          <div style={{ display: 'flex', gap: 6, marginBottom: 18, padding: 3, background: 'var(--bg-sunken)', borderRadius: 8 }}>
            {(['password', 'oidc', 'saml'] as const).map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => setMethod(m)}
                style={{ flex: 1, padding: '6px 8px', border: 'none', background: method === m ? 'var(--surface)' : 'transparent', borderRadius: 6, fontSize: 12, fontWeight: method === m ? 600 : 500, color: method === m ? 'var(--fg)' : 'var(--fg-muted)', boxShadow: method === m ? 'var(--shadow-xs)' : 'none', cursor: 'pointer' }}
              >
                {m === 'password' ? 'Password' : m === 'oidc' ? 'OIDC / SSO' : 'SAML'}
              </button>
            ))}
          </div>

          <Formik
            initialValues={initialValues}
            validationSchema={method === 'password' ? loginSchema : undefined}
            onSubmit={handleSubmit}
          >
            <Form style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
              <FormErrorBanner />

              <FormField name="email" label="Email" type="email" placeholder="you@company.com" />

              {(method === 'oidc' || method === 'saml') && (
                <FormField
                  name="org_id"
                  label="Organization ID"
                  placeholder="org_…"
                  hint={method === 'oidc' ? 'Your OIDC provider will be resolved from your org.' : 'Your IdP metadata must be configured by an admin.'}
                  style={{ fontFamily: 'var(--font-mono)' }}
                />
              )}

              {method === 'password' && (
                <div>
                  <div style={{ position: 'relative' }}>
                    <FormField
                      name="password"
                      label="Password"
                      type={showPw ? 'text' : 'password'}
                      placeholder="••••••••"
                      style={{ paddingRight: 36 }}
                    />
                    <button
                      type="button"
                      className="icon-btn"
                      onClick={() => setShowPw(!showPw)}
                      style={{ position: 'absolute', right: 4, bottom: 6 }}
                    >
                      {showPw ? <I.eyeOff size={14} /> : <I.eye size={14} />}
                    </button>
                  </div>
                  <div style={{ display: 'flex', justifyContent: 'space-between', marginTop: 6 }}>
                    <label style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 12, color: 'var(--fg-muted)', cursor: 'pointer' }}>
                      <input type="checkbox" defaultChecked /> Remember me
                    </label>
                    <a style={{ fontSize: 12, color: 'var(--accent)', cursor: 'pointer' }}>Forgot password?</a>
                  </div>
                </div>
              )}

              <FormSubmit
                label={method === 'password' ? 'Sign in' : method === 'oidc' ? 'Continue with OIDC' : 'Continue to IdP'}
                loadingLabel="Signing in…"
                className="btn primary lg"
                fullWidth
              />
            </Form>
          </Formik>

          <div style={{ display: 'flex', alignItems: 'center', gap: 10, margin: '16px 0', color: 'var(--fg-faint)', fontSize: 11 }}>
            <div style={{ flex: 1, height: 1, background: 'var(--border)' }} /> OR <div style={{ flex: 1, height: 1, background: 'var(--border)' }} />
          </div>
          <div style={{ display: 'flex', gap: 8 }}>
            <button className="btn" style={{ flex: 1 }}><I.github size={14} /> GitHub</button>
            <button className="btn" style={{ flex: 1 }}><I.mail size={14} /> Magic link</button>
          </div>
          <div style={{ marginTop: 18, fontSize: 12, color: 'var(--fg-muted)', textAlign: 'center' }}>
            New here? <a style={{ color: 'var(--accent)', cursor: 'pointer' }}>Ask your admin</a> for an invite.
          </div>
        </div>
      </div>
    </div>
  )
}
