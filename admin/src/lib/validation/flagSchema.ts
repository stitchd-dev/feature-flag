import * as Yup from 'yup'

export const FLAG_TYPES = ['bool', 'string', 'int', 'double', 'json'] as const
export type FlagType = typeof FLAG_TYPES[number]

/** Variant row: key + raw string value. */
const variantSchema = Yup.object({
  key: Yup.string()
    .trim()
    .min(1, 'Variant key is required')
    .matches(/^[a-z0-9_-]+$/, 'Variant key: lowercase letters, digits, hyphens, underscores only')
    .required('Variant key is required'),
  value: Yup.string().required('Variant value is required'),
})

/** Flag create / edit. */
export const flagSchema = Yup.object({
  name: Yup.string()
    .trim()
    .min(1, 'Name is required')
    .max(120, 'Name must be 120 characters or fewer')
    .required('Name is required'),

  key: Yup.string()
    .trim()
    .min(1, 'Key is required')
    .max(120, 'Key must be 120 characters or fewer')
    .matches(
      /^[a-z0-9][a-z0-9_-]*$/,
      'Key must start with a letter/digit and contain only lowercase letters, digits, hyphens, underscores',
    )
    .required('Key is required'),

  description: Yup.string().max(500, 'Description must be 500 characters or fewer'),

  value_type: Yup.string()
    .oneOf(FLAG_TYPES as unknown as string[], 'Invalid flag type')
    .required('Flag type is required'),

  variants: Yup.array()
    .of(variantSchema)
    .min(1, 'At least one variant is required')
    .test(
      'unique-variant-keys',
      'Variant keys must be unique',
      (variants) => {
        if (!variants) return true
        const keys = variants.map((v) => v.key?.trim()).filter(Boolean)
        return new Set(keys).size === keys.length
      },
    )
    .required(),
})

export type FlagFormValues = Yup.InferType<typeof flagSchema>
