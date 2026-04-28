import { Navigate, Outlet } from 'react-router-dom'
import { auth } from '../lib/auth'

/** Protects /superadmin/* — must be authenticated AND is_system=true. */
export function SuperAdminGuard() {
  const session = auth.getSession()
  if (!session) return <Navigate to="/login" replace />
  if (!session.isSystem) return <Navigate to={`/org/${session.orgId}`} replace />
  return <Outlet />
}

/** Protects /org/:orgId/* — must be authenticated AND is_system=false. */
export function OrgGuard() {
  const session = auth.getSession()
  if (!session) return <Navigate to="/login" replace />
  if (session.isSystem) return <Navigate to="/superadmin" replace />
  return <Outlet />
}
