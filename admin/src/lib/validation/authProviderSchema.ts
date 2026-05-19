import * as Yup from 'yup'

const URL_REGEX = /^https?:\/\/.+/

/** OIDC provider config. */
export const oidcProviderSchema = Yup.object({
  provider_id: Yup.string()
    .trim()
    .min(1, 'Provider ID is required')
    .max(80, 'Provider ID must be 80 characters or fewer')
    .matches(/^[a-z0-9_-]+$/, 'Provider ID: lowercase letters, digits, hyphens, underscores only')
    .required('Provider ID is required'),

  client_id: Yup.string()
    .trim()
    .min(1, 'Client ID is required')
    .required('Client ID is required'),

  client_secret: Yup.string()
    .trim()
    .min(1, 'Client secret is required')
    .required('Client secret is required'),

  issuer_url: Yup.string()
    .trim()
    .matches(URL_REGEX, 'Issuer URL must start with http:// or https://')
    .required('Issuer URL is required'),

  /** Space-separated list of scopes; defaults to "openid email profile". */
  scopes: Yup.string(),

  redirect_uri: Yup.string()
    .trim()
    .matches(URL_REGEX, 'Redirect URI must start with http:// or https://'),
})

export type OidcProviderFormValues = Yup.InferType<typeof oidcProviderSchema>

/** SAML provider config. */
export const samlProviderSchema = Yup.object({
  provider_id: Yup.string()
    .trim()
    .min(1, 'Provider ID is required')
    .max(80, 'Provider ID must be 80 characters or fewer')
    .matches(/^[a-z0-9_-]+$/, 'Provider ID: lowercase letters, digits, hyphens, underscores only')
    .required('Provider ID is required'),

  entity_id: Yup.string()
    .trim()
    .min(1, 'Entity ID is required')
    .required('Entity ID is required'),

  sso_url: Yup.string()
    .trim()
    .matches(URL_REGEX, 'SSO URL must start with http:// or https://')
    .required('SSO URL is required'),

  /** PEM-encoded X.509 certificate from the IdP. */
  certificate: Yup.string()
    .trim()
    .test('is-pem-cert', 'Must be a PEM certificate (BEGIN CERTIFICATE)', (value) => {
      if (!value || value.trim() === '') return false
      return value.includes('-----BEGIN CERTIFICATE-----')
    })
    .required('Certificate is required'),

  attribute_email: Yup.string()
    .trim()
    .min(1, 'Email attribute name is required')
    .required('Email attribute name is required'),

  attribute_display_name: Yup.string().trim(),
})

export type SamlProviderFormValues = Yup.InferType<typeof samlProviderSchema>
