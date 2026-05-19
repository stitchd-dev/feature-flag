import * as Yup from 'yup'

/** Standard password login. */
export const loginSchema = Yup.object({
  email: Yup.string()
    .trim()
    .email('Must be a valid email address')
    .required('Email is required'),

  password: Yup.string()
    .min(1, 'Password is required')
    .required('Password is required'),

  /** Optional — for OIDC/SAML flows. */
  org_id: Yup.string(),
})

export type LoginFormValues = Yup.InferType<typeof loginSchema>

/** MFA TOTP challenge (after password accepted). */
export const mfaSchema = Yup.object({
  code: Yup.string()
    .trim()
    .matches(/^\d{6}$/, 'Code must be exactly 6 digits')
    .required('MFA code is required'),
})

export type MfaFormValues = Yup.InferType<typeof mfaSchema>

/** Password reset — request stage (email only). */
export const passwordResetRequestSchema = Yup.object({
  email: Yup.string()
    .trim()
    .email('Must be a valid email address')
    .required('Email is required'),
})

export type PasswordResetRequestFormValues = Yup.InferType<typeof passwordResetRequestSchema>

/** Password reset — confirm stage (token + new password). */
export const passwordResetConfirmSchema = Yup.object({
  token: Yup.string().required('Reset token is required'),

  new_password: Yup.string()
    .min(8, 'Password must be at least 8 characters')
    .required('New password is required'),

  confirm_password: Yup.string()
    .oneOf([Yup.ref('new_password')], 'Passwords do not match')
    .required('Please confirm your password'),
})

export type PasswordResetConfirmFormValues = Yup.InferType<typeof passwordResetConfirmSchema>

/** Switch-org — enter an org ID to context-switch. */
export const switchOrgSchema = Yup.object({
  org_id: Yup.string()
    .trim()
    .min(1, 'Organisation ID is required')
    .required('Organisation ID is required'),
})

export type SwitchOrgFormValues = Yup.InferType<typeof switchOrgSchema>
