import { useState, useEffect } from 'react'
import { useNavigate } from 'react-router-dom'
import { Formik, Form } from 'formik'
import { createOrg, listOrgs } from '../../lib/api'
import type { OrgSummary } from '../../lib/api'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { ErrorBanner } from '../../components/ErrorBanner'
import { EmptyState } from '../../components/EmptyState'
import { FormField } from '../../components/form/FormField'
import { FormErrorBanner } from '../../components/form/FormErrorBanner'
import { FormSubmit } from '../../components/form/FormSubmit'
import { orgSchema } from '../../lib/validation/orgSchema'
import type { OrgFormValues } from '../../lib/validation/orgSchema'
import { extractErrorMessage } from '../../lib/errors'

export function OrgsList() {
  const navigate = useNavigate()
  const [orgs, setOrgs] = useState<OrgSummary[]>([])
  const [loading, setLoading] = useState(true)
  const [showForm, setShowForm] = useState(false)
  const [listError, setListError] = useState<string | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    listOrgs(controller.signal)
      .then(setOrgs)
      .catch((err: unknown) => {
        if ((err as { name?: string }).name !== 'CanceledError') setListError('Failed to load organisations')
      })
      .finally(() => setLoading(false))
    return () => controller.abort()
  }, [])

  const initialValues: OrgFormValues = { name: '' }

  async function handleCreate(
    values: OrgFormValues,
    { setStatus, resetForm }: { setStatus: (s: unknown) => void; resetForm: () => void },
  ) {
    try {
      await createOrg(values.name.trim())
      resetForm()
      setShowForm(false)
      const updated = await listOrgs()
      setOrgs(updated)
    } catch (err: unknown) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

  return (
    <>
      <PageHeader
        crumbs={['Superadmin']}
        title="Organisations"
        subtitle="Manage customer organisations on the Stitchd platform"
        actions={
          <button
            className="btn primary"
            onClick={() => { setShowForm((v) => !v); setListError(null) }}
          >
            <I.plus size={14} /> New Organisation
          </button>
        }
      />

      <div className="page-body">
        {/* Inline create form */}
        {showForm && (
          <div className="card" style={{ marginBottom: 16 }}>
            <div className="card-header">
              <span className="card-title"><I.home size={13} /> Create Organisation</span>
            </div>
            <div className="card-body">
              <Formik
                initialValues={initialValues}
                validationSchema={orgSchema}
                onSubmit={handleCreate}
              >
                <Form style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                  <FormErrorBanner />
                  <div style={{ display: 'flex', gap: 10, alignItems: 'flex-end' }}>
                    <div style={{ flex: 1 }}>
                      <FormField
                        name="name"
                        label="Organisation name"
                        placeholder="e.g. Acme Corp"
                        autoFocus
                      />
                    </div>
                    <FormSubmit label="Create" loadingLabel="Creating…" />
                    <button
                      type="button"
                      className="btn"
                      onClick={() => setShowForm(false)}
                    >
                      Cancel
                    </button>
                  </div>
                </Form>
              </Formik>
            </div>
          </div>
        )}

        {listError && (
          <ErrorBanner message={listError} onDismiss={() => setListError(null)} />
        )}

        {loading ? (
          <div className="card">
            {[...Array(3)].map((_, i) => (
              <div key={i} style={{ padding: '14px 16px', borderBottom: '1px solid var(--border-faint)', display: 'flex', gap: 12, alignItems: 'center' }}>
                <div className="skel" style={{ width: 28, height: 28, borderRadius: 6, flexShrink: 0 }} />
                <div style={{ flex: 1 }}>
                  <div className="skel" style={{ width: 140, marginBottom: 6 }} />
                  <div className="skel" style={{ width: 220, height: 10 }} />
                </div>
                <div className="skel" style={{ width: 60, height: 26, borderRadius: 6 }} />
              </div>
            ))}
          </div>
        ) : orgs.length === 0 ? (
          <EmptyState
            icon={<I.home size={20} />}
            title="No organisations yet"
            desc="Create your first organisation to onboard a customer team."
            action={<button className="btn primary" onClick={() => setShowForm(true)}><I.plus size={14} /> New Organisation</button>}
          />
        ) : (
          <div className="card">
            <div className="card-header">
              <span className="card-title">
                <I.home size={13} /> All Organisations
              </span>
              <span className="badge">
                {orgs.length} {orgs.length === 1 ? 'org' : 'orgs'}
              </span>
            </div>
            <table className="table">
              <thead>
                <tr>
                  <th>Name</th>
                  <th>ID</th>
                  <th>Created</th>
                  <th style={{ width: 80 }} />
                </tr>
              </thead>
              <tbody>
                {orgs.map((org) => (
                  <tr
                    key={org.org_id}
                    className="row-clickable"
                    onClick={() => navigate(`/superadmin/orgs/${org.org_id}`)}
                  >
                    <td>
                      <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                        <div
                          className="org-avatar"
                          style={{ width: 28, height: 28, fontSize: 11, borderRadius: 6, flexShrink: 0 }}
                        >
                          {org.org_name.slice(0, 2).toUpperCase()}
                        </div>
                        <span style={{ fontWeight: 600, fontSize: 13 }}>{org.org_name}</span>
                      </div>
                    </td>
                    <td><span className="mono-key">{org.org_id}</span></td>
                    <td style={{ color: 'var(--fg-muted)', fontSize: 12, fontFamily: 'var(--font-mono)' }}>
                      {org.created_at ? new Date(org.created_at).toLocaleDateString() : '—'}
                    </td>
                    <td>
                      <button
                        className="btn sm"
                        onClick={(e) => { e.stopPropagation(); navigate(`/superadmin/orgs/${org.org_id}`) }}
                      >
                        Manage
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </>
  )
}
