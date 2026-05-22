import { useCallback, useEffect, useState } from 'react'
import { getMyPermissions } from '../lib/api'
import { auth } from '../lib/auth'
import type { Action } from '../lib/permissions'

interface PermissionsState {
  roles: string[]
  permissions: string[]
  loading: boolean
}

// Module-level cache keyed by session token so every component that calls
// usePermissions() shares a single in-flight request and a single cached
// result, instead of each mount firing its own /me/permissions GET.
let cachedToken: string | null = null
let cachedPromise: Promise<{ roles: string[]; permissions: string[] }> | null = null
let cachedResult: { roles: string[]; permissions: string[] } | null = null

function fetchPermissions(token: string): Promise<{ roles: string[]; permissions: string[] }> {
  if (cachedToken === token && cachedPromise) return cachedPromise
  cachedToken = token
  cachedResult = null
  cachedPromise = getMyPermissions()
    .then((data) => {
      const result = { roles: data.roles, permissions: data.permissions }
      cachedResult = result
      return result
    })
    .catch((err) => {
      cachedPromise = null
      throw err
    })
  return cachedPromise
}

export function usePermissions() {
  const session = auth.getSession()

  const [state, setState] = useState<PermissionsState>(() => {
    if (cachedResult && cachedToken === session?.token) {
      return { roles: cachedResult.roles, permissions: cachedResult.permissions, loading: false }
    }
    // Seed immediately from JWT claims for fast first render
    return {
      roles: session?.roles ?? [],
      permissions: session?.permissions ?? [],
      loading: !!session,
    }
  })

  useEffect(() => {
    if (!session) return

    let cancelled = false
    fetchPermissions(session.token)
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
