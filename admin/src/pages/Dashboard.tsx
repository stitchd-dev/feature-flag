/**
 * Dashboard — org-scoped overview page.
 *
 * Replaces the previous mock-data scaffold (feature-flag-tcy + feature-flag-zh9).
 * Renders real counts queried from the management API:
 *   • Projects count (per org)
 *   • Environments count (per selected project)
 *   • Flags count (per selected project)
 *
 * When there's no project selected yet (fresh org) we show a welcome empty
 * state instead of fake stats. Service-health panel was removed entirely —
 * the dashboard is not the right surface for ops-style health checks.
 */
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { PageHeader } from '../components/primitives'
import { I } from '../components/icons'
import { useOrgContext } from '../context/OrgContext'
import { listFlags } from '../lib/api'
import { auth } from '../lib/auth'

interface DashboardCounts {
  projects: number
  environments: number
  flags: number | null // null = not yet loaded (or no project)
}

export function Dashboard() {
  const navigate = useNavigate()
  const {
    orgId,
    projectId,
    projects,
    projectsLoading,
    environments,
    envsLoading,
  } = useOrgContext()

  const [flagsCount, setFlagsCount] = useState<number | null>(null)
  // True when more flags exist beyond the single page we sampled for the count.
  const [flagsHasMore, setFlagsHasMore] = useState(false)
  const [flagsLoading, setFlagsLoading] = useState(false)

  // Resolve a friendly org name: check history first, then org list, then fall back to ID.
  const orgName = (() => {
    const fromHistory = auth.getOrgHistory().find((h) => h.orgId === orgId)?.orgName
    if (fromHistory) return fromHistory
    const fromList = auth.getOrgs().find((o) => o.org_id === orgId)?.org_name
    if (fromList) return fromList
    return orgId ?? '—'
  })()

  const currentProject = projects.find((p) => p.project_id === projectId) ?? null

  useEffect(() => {
    if (!projectId) {
      setFlagsCount(null)
      setFlagsHasMore(false)
      return
    }
    let cancelled = false
    setFlagsLoading(true)
    // The flags list endpoint is cursor-paginated and no longer returns a total.
    // Fetch a single page and report the number of flags loaded; when a further
    // page exists the displayed value is suffixed with "+" (see StatCard below).
    listFlags(projectId, { limit: 100 })
      .then((res) => {
        if (!cancelled) {
          setFlagsCount(res.items?.length ?? 0)
          setFlagsHasMore(Boolean(res.next_cursor))
        }
      })
      .catch(() => {
        if (!cancelled) setFlagsCount(null)
      })
      .finally(() => {
        if (!cancelled) setFlagsLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [projectId])

  const counts: DashboardCounts = {
    projects: projects.length,
    environments: environments.length,
    flags: flagsCount,
  }

  const isFreshOrg = !projectsLoading && projects.length === 0

  return (
    <>
      <PageHeader
        crumbs={[orgName, currentProject?.project_name ?? 'No project']}
        title={`Welcome to ${orgName}`}
        subtitle={
          isFreshOrg
            ? 'Get started by creating your first project, then add an environment and a flag.'
            : 'Snapshot of projects, environments, and flags in this organisation.'
        }
        actions={
          <button className="btn primary" onClick={() => navigate(`/org/${orgId}/flags`)}>
            <I.flag size={14} /> View flags
          </button>
        }
      />
      <div className="page-body">
        {isFreshOrg ? (
          <FreshOrgEmptyState orgId={orgId} />
        ) : (
          <>
            <div className="stat-grid" style={{ marginBottom: 18 }}>
              <StatCard
                label="Projects"
                value={projectsLoading ? '—' : String(counts.projects)}
                hint={counts.projects === 0 ? 'No projects yet' : `${counts.projects} project${counts.projects === 1 ? '' : 's'}`}
                onClick={() => navigate(`/org/${orgId}/environments`)}
              />
              <StatCard
                label="Environments"
                value={!projectId ? '—' : envsLoading ? '…' : String(counts.environments)}
                hint={
                  !projectId
                    ? 'Select a project first'
                    : counts.environments === 0
                      ? 'No environments yet'
                      : currentProject
                        ? `in ${currentProject.project_name}`
                        : ''
                }
                onClick={() => navigate(`/org/${orgId}/environments`)}
              />
              <StatCard
                label="Flags"
                value={!projectId ? '—' : flagsLoading ? '…' : `${counts.flags ?? 0}${flagsHasMore ? '+' : ''}`}
                hint={
                  !projectId
                    ? 'Select a project first'
                    : (counts.flags ?? 0) === 0
                      ? 'No flags yet'
                      : currentProject
                        ? `in ${currentProject.project_name}`
                        : ''
                }
                onClick={() => navigate(`/org/${orgId}/flags`)}
              />
            </div>

            <div className="card">
              <div className="card-header">
                <div className="card-title">
                  <I.info size={14} /> Getting started
                </div>
              </div>
              <div style={{ padding: '14px 18px', fontSize: 13, color: 'var(--fg-muted)', lineHeight: 1.6 }}>
                <p style={{ margin: '0 0 8px' }}>
                  Use the sidebar to navigate between projects, environments, flags, segments,
                  experiments, events, and metrics.
                </p>
                <p style={{ margin: 0 }}>
                  Each environment scopes its own rules, segments, experiments, and SDK keys.
                  Make sure to create at least one environment per project before creating flags.
                </p>
              </div>
            </div>
          </>
        )}
      </div>
    </>
  )
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

interface StatCardProps {
  label: string
  value: string
  hint?: string
  onClick?: () => void
}

function StatCard({ label, value, hint, onClick }: StatCardProps) {
  return (
    <div
      className="stat"
      style={onClick ? { cursor: 'pointer' } : undefined}
      onClick={onClick}
    >
      <div className="stat-label">{label}</div>
      <div className="stat-value">{value}</div>
      {hint && <div className="stat-delta">{hint}</div>}
    </div>
  )
}

function FreshOrgEmptyState({ orgId }: { orgId: string }) {
  return (
    <div className="card" style={{ padding: 32 }}>
      <div style={{ textAlign: 'center' }}>
        <I.home size={28} stroke="var(--fg-subtle)" style={{ marginBottom: 12 }} />
        <div style={{ fontWeight: 600, fontSize: 16, marginBottom: 6 }}>
          No projects yet
        </div>
        <div style={{ color: 'var(--fg-muted)', fontSize: 13, maxWidth: 420, margin: '0 auto 16px' }}>
          A project groups your flags, segments, environments, and SDK keys. Open the
          Environments page from the sidebar to create your first project + environment.
        </div>
        <a className="btn primary" href={`/org/${orgId}/environments`}>
          <I.plus size={13} /> Go to Environments
        </a>
      </div>
    </div>
  )
}
