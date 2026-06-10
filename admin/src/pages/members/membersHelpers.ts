/**
 * Pure helpers + validation for the Members page.
 *
 * The backend exposes a fixed two-role RBAC model (`org_admin` / `org_member`)
 * and NO role-change / invite / custom-role API — see the track learnings. These
 * helpers reflect that reality honestly rather than inventing capabilities.
 */
import * as Yup from 'yup'
import type { OrgMemberRole } from '../../lib/api'

/** Deterministic two-letter avatar initials from a display name (or email). */
export function memberInitials(displayName: string, email: string): string {
  const name = displayName.trim()
  if (name) {
    const words = name.split(/\s+/).filter(Boolean)
    const letters = words.length >= 2 ? words[0][0] + words[1][0] : words[0].slice(0, 2)
    return letters.toUpperCase()
  }
  // Email fallback: use the local part (before @) so we don't span the domain.
  const local = email.trim().split('@')[0]
  return (local.slice(0, 2) || '?').toUpperCase()
}

/** Map a backend role string to a human label + badge class. */
export function roleBadge(role: string): { label: string; className: string } {
  switch (role) {
    case 'org_admin':
      return { label: 'Admin', className: 'badge accent' }
    case 'org_member':
      return { label: 'Member', className: 'badge' }
    default:
      // Surface unknown roles verbatim rather than hiding them.
      return { label: role, className: 'badge' }
  }
}

/** The real, fixed RBAC roles — documented for the honest "Roles" tab. */
export const ROLE_MODEL: { role: OrgMemberRole; label: string; summary: string }[] = [
  {
    role: 'org_admin',
    label: 'Org Admin',
    summary:
      'Full read/write across the organisation: manage flags, segments, experiments, metrics, events, environments, SDK keys, members and SSO providers.',
  },
  {
    role: 'org_member',
    label: 'Member',
    summary:
      'Read access to flags, segments, experiments and analytics. Cannot manage members, environments, SDK keys or SSO providers.',
  },
]

export const ROLE_OPTIONS: { value: OrgMemberRole; label: string; desc: string }[] = [
  { value: 'org_member', label: 'Member', desc: 'Read access across the org' },
  { value: 'org_admin', label: 'Org Admin', desc: 'Full management of the org' },
]

/**
 * Add-member form schema. The backend `CreateUser` directly provisions a
 * credentialed account (email + display name + password + role) — it is not an
 * email invite — so all four fields are required.
 */
export const addMemberSchema = Yup.object({
  email: Yup.string().trim().email('Must be a valid email address').required('Email is required'),
  display_name: Yup.string()
    .trim()
    .min(1, 'Display name is required')
    .max(120, 'Display name must be 120 characters or fewer')
    .required('Display name is required'),
  password: Yup.string().min(8, 'Password must be at least 8 characters').required('Password is required'),
  org_role: Yup.string()
    .oneOf(['org_admin', 'org_member'], 'Invalid role')
    .required('Role is required'),
})

export type AddMemberFormValues = Yup.InferType<typeof addMemberSchema>
