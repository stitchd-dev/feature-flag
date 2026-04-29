import { createContext, useContext, useState } from 'react'
import type { ReactNode } from 'react'
import { useParams } from 'react-router-dom'
import { auth } from '../lib/auth'

interface OrgContextValue {
  orgId: string
  projectId: string | null
  envId: string | null
  setProjectId: (id: string) => void
  setEnvId: (id: string) => void
}

const OrgContext = createContext<OrgContextValue | null>(null)
const PROJECT_KEY = 'stitchd_project_id'
const ENV_KEY = 'stitchd_env_id'

export function OrgProvider({ children }: { children: ReactNode }) {
  const { orgId } = useParams<{ orgId: string }>()
  const resolvedOrgId = orgId ?? auth.getOrgId() ?? ''
  const [projectId, setProjectIdState] = useState<string | null>(() => localStorage.getItem(PROJECT_KEY))
  const [envId, setEnvIdState] = useState<string | null>(() => localStorage.getItem(ENV_KEY))

  const setProjectId = (id: string) => {
    localStorage.setItem(PROJECT_KEY, id)
    setProjectIdState(id)
  }
  const setEnvId = (id: string) => {
    localStorage.setItem(ENV_KEY, id)
    setEnvIdState(id)
  }

  return (
    <OrgContext.Provider value={{ orgId: resolvedOrgId, projectId, envId, setProjectId, setEnvId }}>
      {children}
    </OrgContext.Provider>
  )
}

export function useOrgContext(): OrgContextValue {
  const ctx = useContext(OrgContext)
  if (!ctx) throw new Error('useOrgContext must be used inside OrgProvider')
  return ctx
}
