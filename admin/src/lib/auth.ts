const SESSION_KEY = 'stitchd_session'
const ORG_HISTORY_KEY = 'stitchd_org_history'
const ORG_LIST_KEY = 'stitchd_org_list'

export interface OrgEntry {
  org_id: string
  org_name: string
  role: string
}

export interface Session {
  token: string
  refreshToken?: string
  orgId: string
  isSystem: boolean
  userId: string
  email?: string
  name?: string
  roles: string[]
  permissions: string[]
}

export interface OrgHistoryEntry {
  orgId: string
  orgName: string
}

export function decodeJwtPayload(token: string): Record<string, unknown> {
  try {
    const parts = token.split('.')
    if (parts.length !== 3) return {}
    const payload = parts[1].replace(/-/g, '+').replace(/_/g, '/')
    const padded = payload + '='.repeat((4 - (payload.length % 4)) % 4)
    return JSON.parse(atob(padded)) as Record<string, unknown>
  } catch {
    return {}
  }
}

export const auth = {
  // Legacy token helpers (kept for backwards compat during migration)
  getToken: (): string | null => {
    const session = auth.getSession()
    return session?.token ?? null
  },

  isAuthenticated: (): boolean => !!auth.getSession(),

  // New session helpers
  setSession: (session: Session): void => {
    localStorage.setItem(SESSION_KEY, JSON.stringify(session))
  },

  getSession: (): Session | null => {
    try {
      const raw = localStorage.getItem(SESSION_KEY)
      if (!raw) return null
      return JSON.parse(raw) as Session
    } catch {
      return null
    }
  },

  clearSession: (): void => {
    localStorage.removeItem(SESSION_KEY)
  },

  getOrgId: (): string | null => auth.getSession()?.orgId ?? null,

  isSystem: (): boolean => auth.getSession()?.isSystem ?? false,

  // Decode JWT and extract is_system from custom claims
  decodeIsSystem: (token: string): boolean => {
    const payload = decodeJwtPayload(token)
    return payload['is_system'] === true
  },

  // Decode JWT and extract roles/permissions arrays from custom claims
  decodeRoles: (token: string): string[] => {
    const payload = decodeJwtPayload(token)
    return Array.isArray(payload['roles']) ? (payload['roles'] as string[]) : []
  },

  decodePermissions: (token: string): string[] => {
    const payload = decodeJwtPayload(token)
    return Array.isArray(payload['permissions'])
      ? (payload['permissions'] as string[])
      : []
  },

  decodeEmail: (token: string): string | undefined => {
    const payload = decodeJwtPayload(token)
    return typeof payload['email'] === 'string' ? payload['email'] : undefined
  },

  decodeName: (token: string): string | undefined => {
    const payload = decodeJwtPayload(token)
    const name = typeof payload['name'] === 'string' ? payload['name'] : undefined
    return name && name.length > 0 ? name : undefined
  },

  // Org history for the org switcher
  getOrgHistory: (): OrgHistoryEntry[] => {
    try {
      const raw = localStorage.getItem(ORG_HISTORY_KEY)
      if (!raw) return []
      return JSON.parse(raw) as OrgHistoryEntry[]
    } catch {
      return []
    }
  },

  addOrgToHistory: (entry: OrgHistoryEntry): void => {
    const history = auth.getOrgHistory()
    const filtered = history.filter((h) => h.orgId !== entry.orgId)
    localStorage.setItem(ORG_HISTORY_KEY, JSON.stringify([entry, ...filtered].slice(0, 10)))
  },

  // Org list from server (used for seamless switching)
  setOrgs: (orgs: OrgEntry[]): void => {
    localStorage.setItem(ORG_LIST_KEY, JSON.stringify(orgs))
  },

  getOrgs: (): OrgEntry[] => {
    try {
      const raw = localStorage.getItem(ORG_LIST_KEY)
      if (!raw) return []
      return JSON.parse(raw) as OrgEntry[]
    } catch {
      return []
    }
  },
}
