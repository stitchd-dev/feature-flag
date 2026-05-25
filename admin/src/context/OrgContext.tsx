import { createContext, useCallback, useContext, useEffect, useState } from 'react'
import type { ReactNode } from 'react'
import { useParams } from 'react-router-dom'
import type { EnvironmentSummary, ProjectSummary } from '../lib/api'
import { listEnvironments, listProjects } from '../lib/api'
import { auth } from '../lib/auth'

interface OrgContextValue {
  orgId: string
  projectId: string | null
  envId: string | null
  setProjectId: (id: string) => void
  setEnvId: (id: string) => void
  projects: ProjectSummary[]
  projectsLoading: boolean
  refreshProjects: () => void
  environments: EnvironmentSummary[]
  envsLoading: boolean
  refreshEnvironments: () => void
}

const OrgContext = createContext<OrgContextValue | null>(null)
const PROJECT_KEY = 'stitchd_project_id'
const ENV_KEY = 'stitchd_env_id'

export function OrgProvider({ children }: { children: ReactNode }) {
  const { orgId } = useParams<{ orgId: string }>()
  const resolvedOrgId = orgId ?? auth.getOrgId() ?? ''
  const [projectId, setProjectIdState] = useState<string | null>(() => localStorage.getItem(PROJECT_KEY))
  const [envId, setEnvIdState] = useState<string | null>(() => localStorage.getItem(ENV_KEY))
  const [projects, setProjects] = useState<ProjectSummary[]>([])
  const [projectsLoading, setProjectsLoading] = useState(false)
  const [environments, setEnvironments] = useState<EnvironmentSummary[]>([])
  const [envsLoading, setEnvsLoading] = useState(false)

  const setProjectId = (id: string) => {
    localStorage.setItem(PROJECT_KEY, id)
    setProjectIdState(id)
    // Clear env selection when project changes
    localStorage.removeItem(ENV_KEY)
    setEnvIdState(null)
    setEnvironments([])
  }
  const setEnvId = (id: string) => {
    localStorage.setItem(ENV_KEY, id)
    setEnvIdState(id)
  }

  const refreshProjects = useCallback(() => {
    if (!resolvedOrgId) return
    setProjectsLoading(true)
    listProjects(resolvedOrgId)
      .then((p) => setProjects(p))
      .catch(() => setProjects([]))
      .finally(() => setProjectsLoading(false))
  }, [resolvedOrgId])

  const refreshEnvironments = useCallback(() => {
    if (!projectId) { setEnvironments([]); return }
    setEnvsLoading(true)
    listEnvironments(projectId)
      .then((envs) => {
        setEnvironments(envs)
        // Auto-select first env if stored one is no longer valid
        setEnvIdState((prev) => {
          if (prev && envs.some((e) => e.environment_id === prev)) return prev
          const first = envs[0]?.environment_id ?? null
          if (first) localStorage.setItem(ENV_KEY, first)
          else localStorage.removeItem(ENV_KEY)
          return first
        })
      })
      .catch(() => setEnvironments([]))
      .finally(() => setEnvsLoading(false))
  }, [projectId])

  useEffect(() => { refreshProjects() }, [refreshProjects])
  useEffect(() => { refreshEnvironments() }, [refreshEnvironments])

  return (
    <OrgContext.Provider value={{
      orgId: resolvedOrgId, projectId, envId, setProjectId, setEnvId,
      projects, projectsLoading, refreshProjects,
      environments, envsLoading, refreshEnvironments,
    }}>
      {children}
    </OrgContext.Provider>
  )
}

export function useOrgContext(): OrgContextValue {
  const ctx = useContext(OrgContext)
  if (!ctx) throw new Error('useOrgContext must be used inside OrgProvider')
  return ctx
}

/** Returns null when used outside OrgProvider (e.g. superadmin shell). */
export function useOptionalOrgContext(): OrgContextValue | null {
  return useContext(OrgContext)
}
