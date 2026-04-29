const SESSION_KEY = 'stitchd_session'
const ORG_HISTORY_KEY = 'stitchd_org_history'

export interface Session {
  token: string
  orgId: string
  isSystem: boolean
  userId: string
}

export interface OrgHistoryEntry {
  orgId: string
  orgName: string
}

function decodeJwtPayload(token: string): Record<string, unknown> {
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
}
