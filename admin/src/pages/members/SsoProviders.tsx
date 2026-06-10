import { useCallback, useEffect, useState } from 'react'
import { Formik, Form, Field, useField } from 'formik'
import * as Yup from 'yup'
import {
  listAuthProviders,
  createAuthProvider,
  updateAuthProvider,
  deleteAuthProvider,
  getSamlSpMetadata,
  type AuthProvider,
  type CreateAuthProviderBody,
} from '../../lib/api'
import { I } from '../../components/icons'
import { Modal } from '../../components/Modal'
import { ConfirmDialog } from '../../components/ConfirmDialog'
import { EmptyState } from '../../components/EmptyState'
import { LoadingSpinner } from '../../components/LoadingSpinner'
import { ErrorBanner } from '../../components/ErrorBanner'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { FormSubmit } from '../../components/form/FormSubmit'
import { extractErrorMessage } from '../../lib/errors'

interface Props {
  orgId: string
  canManage: boolean
}

type ProviderType = 'oidc' | 'saml'

interface ProviderFormValues {
  provider_type: ProviderType
  display_name: string
  // OIDC
  issuer_url: string
  client_id: string
  client_secret: string
  scopes: string
  // SAML
  idp_metadata_url: string
  name_id_format: string
  sp_entity_id: string
}

const providerSchema = Yup.object({
  provider_type: Yup.string().oneOf(['oidc', 'saml']).required(),
  display_name: Yup.string().trim().min(1, 'Display name is required').required('Display name is required'),
  issuer_url: Yup.string().when('provider_type', {
    is: 'oidc',
    then: (s) => s.trim().url('Must be a valid URL').required('Issuer URL is required'),
    otherwise: (s) => s.optional(),
  }),
  client_id: Yup.string().when('provider_type', {
    is: 'oidc',
    then: (s) => s.trim().required('Client ID is required'),
    otherwise: (s) => s.optional(),
  }),
  client_secret: Yup.string().when('provider_type', {
    is: 'oidc',
    then: (s) => s.trim().required('Client secret is required'),
    otherwise: (s) => s.optional(),
  }),
  sp_entity_id: Yup.string().when('provider_type', {
    is: 'saml',
    then: (s) => s.trim().required('SP entity ID is required'),
    otherwise: (s) => s.optional(),
  }),
})

function emptyForm(): ProviderFormValues {
  return {
    provider_type: 'oidc',
    display_name: '',
    issuer_url: '',
    client_id: '',
    client_secret: '',
    scopes: 'openid,profile,email',
    idp_metadata_url: '',
    name_id_format: '',
    sp_entity_id: '',
  }
}

function toCreateBody(v: ProviderFormValues): CreateAuthProviderBody {
  if (v.provider_type === 'oidc') {
    return {
      provider_type: 'oidc',
      display_name: v.display_name.trim(),
      config: {
        issuer_url: v.issuer_url.trim(),
        client_id: v.client_id.trim(),
        client_secret: v.client_secret,
        scopes: v.scopes.split(',').map((s) => s.trim()).filter(Boolean),
      },
    }
  }
  return {
    provider_type: 'saml',
    display_name: v.display_name.trim(),
    config: {
      sp_entity_id: v.sp_entity_id.trim(),
      idp_metadata_url: v.idp_metadata_url.trim() || undefined,
      name_id_format: v.name_id_format.trim() || undefined,
    },
  }
}

/** A Formik-bound radio for the provider-type toggle. */
function TypeRadio({ value }: { value: ProviderType }) {
  const [field] = useField({ name: 'provider_type', type: 'radio', value })
  return <input type="radio" {...field} style={{ accentColor: 'var(--accent)' }} />
}

function ProviderModal({ orgId, onClose, onSaved }: { orgId: string; onClose: () => void; onSaved: () => void }) {
  async function handleSubmit(values: ProviderFormValues, { setStatus }: { setStatus: (s: unknown) => void }) {
    try {
      await createAuthProvider(orgId, toCreateBody(values))
      onSaved()
    } catch (err: unknown) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

  return (
    <Modal isOpen onClose={onClose} title="Add SSO provider" size="md">
      <Formik initialValues={emptyForm()} validationSchema={providerSchema} onSubmit={handleSubmit}>
        {({ values }) => (
          <Form style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
            <FormErrorBanner />

            <div>
              <label className="label" style={{ display: 'block', marginBottom: 6 }}>Protocol</label>
              <div style={{ display: 'flex', gap: 10 }}>
                {(['oidc', 'saml'] as ProviderType[]).map((t) => (
                  <label key={t} style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '8px 14px', border: '1px solid var(--border)', borderRadius: 8, cursor: 'pointer', flex: 1 }}>
                    <TypeRadio value={t} />
                    <span style={{ fontWeight: 500, fontSize: 13 }}>{t === 'oidc' ? 'OIDC' : 'SAML 2.0'}</span>
                  </label>
                ))}
              </div>
            </div>

            <LabeledField name="display_name" label="Display name" placeholder="Okta" />

            {values.provider_type === 'oidc' ? (
              <>
                <LabeledField name="issuer_url" label="Issuer URL" placeholder="https://acme.okta.com" />
                <LabeledField name="client_id" label="Client ID" />
                <LabeledField name="client_secret" label="Client secret" type="password" />
                <LabeledField name="scopes" label="Scopes (comma-separated)" />
              </>
            ) : (
              <>
                <LabeledField name="sp_entity_id" label="SP entity ID" placeholder="stitchd" />
                <LabeledField name="idp_metadata_url" label="IdP metadata URL (optional)" />
                <LabeledField name="name_id_format" label="NameID format (optional)" />
              </>
            )}

            <div style={{ display: 'flex', gap: 8, paddingTop: 4 }}>
              <FormSubmit label="Add provider" loadingLabel="Saving…" className="btn primary" fullWidth />
              <button type="button" className="btn" onClick={onClose}>Cancel</button>
            </div>
          </Form>
        )}
      </Formik>
    </Modal>
  )
}

function LabeledField({ name, label, type = 'text', placeholder }: { name: string; label: string; type?: string; placeholder?: string }) {
  const [, meta] = useField<string>(name)
  const isError = meta.touched && Boolean(meta.error)
  return (
    <div>
      <label htmlFor={name} className="label" style={{ display: 'block', marginBottom: 4 }}>{label}</label>
      <Field id={name} name={name} type={type} className="input" placeholder={placeholder} style={{ width: '100%', borderColor: isError ? 'var(--danger)' : undefined }} />
      {isError && <div style={{ fontSize: 11, color: 'var(--danger)', marginTop: 4 }}>{meta.error}</div>}
    </div>
  )
}

/**
 * SSO providers tab — real CRUD over the org auth-provider API
 * (OIDC + SAML). SAML providers expose a "Download SP metadata" action.
 */
export function SsoProviders({ orgId, canManage }: Props) {
  const [providers, setProviders] = useState<AuthProvider[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [showAdd, setShowAdd] = useState(false)
  const [deleting, setDeleting] = useState<AuthProvider | null>(null)

  const load = useCallback(() => {
    setLoading(true)
    setError(null)
    const controller = new AbortController()
    listAuthProviders(orgId, controller.signal)
      .then(setProviders)
      .catch((err) => { if (!controller.signal.aborted) setError(extractErrorMessage(err)) })
      .finally(() => setLoading(false))
    return controller
  }, [orgId])

  useEffect(() => {
    const controller = load()
    return () => controller.abort()
  }, [load])

  async function handleToggle(p: AuthProvider) {
    setBusy(true)
    setError(null)
    try {
      await updateAuthProvider(orgId, p.id, { enabled: !p.enabled })
      load()
    } catch (err: unknown) {
      setError(extractErrorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  async function handleDelete(p: AuthProvider) {
    setBusy(true)
    setError(null)
    try {
      await deleteAuthProvider(orgId, p.id)
      setDeleting(null)
      load()
    } catch (err: unknown) {
      setError(extractErrorMessage(err))
      setDeleting(null)
    } finally {
      setBusy(false)
    }
  }

  async function handleDownloadMetadata(p: AuthProvider) {
    setError(null)
    try {
      const xml = await getSamlSpMetadata(orgId, p.id)
      const blob = new Blob([xml], { type: 'application/xml' })
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = `${p.display_name || 'sp'}-metadata.xml`
      a.click()
      URL.revokeObjectURL(url)
    } catch (err: unknown) {
      setError(extractErrorMessage(err))
    }
  }

  if (loading) return <div className="card" style={{ padding: 32 }}><LoadingSpinner label="Loading SSO providers…" /></div>

  return (
    <div>
      {error && <ErrorBanner message={error} onDismiss={() => setError(null)} />}
      <div className="card">
        <div className="card-header">
          <div className="card-title"><I.fingerprint size={14} /> SSO providers</div>
          {canManage && (
            <button className="btn primary sm" disabled={busy} onClick={() => setShowAdd(true)}>
              <I.plus size={12} /> Add provider
            </button>
          )}
        </div>
        {providers.length === 0 ? (
          <EmptyState
            icon={<I.fingerprint size={20} />}
            title="No SSO providers"
            desc="Add an OIDC or SAML provider to let members sign in with your identity provider."
          />
        ) : (
          <table className="table">
            <thead>
              <tr><th>Provider</th><th>Type</th><th>Status</th>{canManage && <th />}</tr>
            </thead>
            <tbody>
              {providers.map((p) => (
                <tr key={p.id}>
                  <td>
                    <div style={{ fontWeight: 600 }}>{p.display_name}</div>
                    <div style={{ fontSize: 11, color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)' }}>
                      {p.oidc?.issuer_url ?? p.saml?.sp_entity_id ?? p.id}
                    </div>
                  </td>
                  <td><span className="badge">{p.provider_type.toUpperCase()}</span></td>
                  <td>
                    <span className={`badge ${p.enabled ? 'success' : 'warning'}`}>
                      {p.enabled ? 'enabled' : 'disabled'}
                    </span>
                  </td>
                  {canManage && (
                    <td style={{ textAlign: 'right' }}>
                      <div style={{ display: 'inline-flex', gap: 6 }}>
                        {p.provider_type === 'saml' && (
                          <button className="btn sm" disabled={busy} title="Download SP metadata" onClick={() => void handleDownloadMetadata(p)}>
                            <I.download size={12} /> Metadata
                          </button>
                        )}
                        <button className="btn sm" disabled={busy} onClick={() => void handleToggle(p)}>
                          {p.enabled ? 'Disable' : 'Enable'}
                        </button>
                        <button className="icon-btn icon-btn--danger" disabled={busy} title="Delete provider" aria-label="Delete provider" onClick={() => setDeleting(p)}>
                          <I.trash size={13} />
                        </button>
                      </div>
                    </td>
                  )}
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {showAdd && (
        <ProviderModal orgId={orgId} onClose={() => setShowAdd(false)} onSaved={() => { setShowAdd(false); load() }} />
      )}
      {deleting && (
        <ConfirmDialog
          title="Delete SSO provider"
          message={`Delete "${deleting.display_name}"? Members will no longer be able to sign in with it.`}
          confirmLabel="Delete"
          confirmDanger
          onConfirm={() => void handleDelete(deleting)}
          onCancel={() => setDeleting(null)}
        />
      )}
    </div>
  )
}
