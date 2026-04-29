import { useCallback, useEffect, useState } from 'react'
import { getMyPermissions } from '../lib/api'
import { auth } from '../lib/auth'
import type { Action } from '../lib/permissions'

interface PermissionsState {
  roles: string[]
  permissions: string[]
  loading: boolean
}

export function usePermissions() {
  const session = auth.getSession()

  const [state, setState] = useState<PermissionsState>(() => ({
    // Seed immediately from JWT claims for fast first render
    roles: session?.roles ?? [],
    permissions: session?.permissions ?? [],
    loading: !!session,
  }))

  useEffect(() => {
    if (!session) return

    let cancelled = false
    getMyPermissions()
      .then((data) => {
        if (!cancelled) {
          setState({ roles: data.roles, permissions: data.permissions, loading: false })
        }
      })
      .catch(() => {
        if (!cancelled) {
          setState((prev) => ({ ...prev, loading: false }))
        }
      })

    return () => {
      cancelled = true
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session?.token])

  const can = useCallback(
    (action: Action): boolean => state.permissions.includes(action),
    [state.permissions],
  )

  return { ...state, can }
}
