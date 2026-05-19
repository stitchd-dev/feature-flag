/**
 * PermissionGate — renders children if the user has a given permission,
 * otherwise renders a fallback (locked overlay or custom fallback).
 *
 * Props:
 *   permission  — permission string, e.g. 'flag:write'
 *   fallback    — content to render when access is denied (default: locked card)
 *   children    — content to render when access is granted
 */
import type { ReactNode } from 'react'
import { usePermissions } from '../hooks/usePermissions'
import { I } from './icons'

interface Props {
  permission: string
  fallback?: ReactNode
  children: ReactNode
}

function DefaultFallback() {
  return (
    <div className="card" style={{ padding: 48, textAlign: 'center' }}>
      <I.lock size={28} stroke="var(--fg-subtle)" style={{ marginBottom: 12 }} />
      <div style={{ fontWeight: 600, fontSize: 15, marginBottom: 6 }}>Access restricted</div>
      <div style={{ color: 'var(--fg-muted)', fontSize: 13 }}>
        You do not have permission to view this content.
      </div>
    </div>
  )
}

export function PermissionGate({ permission, fallback = <DefaultFallback />, children }: Props) {
  const { can, loading } = usePermissions()

  // While loading, render nothing (avoids flash of lock screen)
  if (loading) return null

  if (!can(permission as Parameters<typeof can>[0])) {
    return <>{fallback}</>
  }

  return <>{children}</>
}
