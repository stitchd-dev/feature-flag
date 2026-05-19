import * as Yup from 'yup'

/** Org create. */
export const orgSchema = Yup.object({
  name: Yup.string()
    .trim()
    .min(1, 'Organisation name is required')
    .max(120, 'Organisation name must be 120 characters or fewer')
    .required('Organisation name is required'),
})

export type OrgFormValues = Yup.InferType<typeof orgSchema>

/** User invite into an org. */
export const userInviteSchema = Yup.object({
  email: Yup.string()
    .trim()
    .email('Must be a valid email address')
    .required('Email is required'),

  display_name: Yup.string()
    .trim()
    .min(1, 'Display name is required')
    .max(120, 'Display name must be 120 characters or fewer')
    .required('Display name is required'),

  org_role: Yup.string()
    .oneOf(['org_admin', 'member'], 'Invalid role')
    .required('Role is required'),

  /** Password — required only when creating a new user. */
  password: Yup.string().when('mode', {
    is: 'new',
    then: (s) =>
      s
        .min(8, 'Password must be at least 8 characters')
        .required('Password is required'),
    otherwise: (s) => s.optional(),
  }),

  mode: Yup.string().oneOf(['existing', 'new']).required(),
})

export type UserInviteFormValues = Yup.InferType<typeof userInviteSchema>
