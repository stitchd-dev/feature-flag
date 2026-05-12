// Permission action strings — must match what the auth service embeds in JWTs.

export const PERMISSIONS = {
  PROJECT_CREATE: 'project:create',
  PROJECT_RENAME: 'project:rename',
  PROJECT_DELETE: 'project:delete',
  ENVIRONMENT_READ: 'environment:read',
  ENVIRONMENT_CREATE: 'environment:create',
  ENVIRONMENT_RENAME: 'environment:rename',
  ENVIRONMENT_DELETE: 'environment:delete',
  SDK_KEY_CREATE: 'sdk_key:create',
  SDK_KEY_REVOKE: 'sdk_key:revoke',
  FLAG_READ: 'flag:read',
  FLAG_WRITE: 'flag:write',
} as const

export type Action = (typeof PERMISSIONS)[keyof typeof PERMISSIONS]
