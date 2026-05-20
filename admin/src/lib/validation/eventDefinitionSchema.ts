import * as Yup from 'yup'

export const METRIC_TYPES = ['count', 'conversion', 'revenue', 'duration', 'custom'] as const
export type MetricType = typeof METRIC_TYPES[number]

/** Event definition create / edit. */
export const eventDefinitionSchema = Yup.object({
  key: Yup.string()
    .trim()
    .min(1, 'Event key is required')
    .max(120, 'Event key must be 120 characters or fewer')
    .matches(
      /^[a-z0-9][a-z0-9_.:-]*$/,
      'Key must start with a letter/digit; may contain letters, digits, dots, colons, underscores, hyphens',
    )
    .required('Event key is required'),

  metric_type: Yup.string()
    .oneOf(METRIC_TYPES as unknown as string[], 'Invalid metric type')
    .required('Metric type is required'),

  description: Yup.string().max(500, 'Description must be 500 characters or fewer'),

  /** Optional JSON schema string for the event payload. */
  schema: Yup.string().test('valid-json-or-empty', 'Schema must be valid JSON', (value) => {
    if (!value || value.trim() === '') return true
    try {
      JSON.parse(value)
      return true
    } catch {
      return false
    }
  }),
})

export type EventDefinitionFormValues = Yup.InferType<typeof eventDefinitionSchema>

/**
 * Edit-mode schema — same as create but `key` is immutable, so it's omitted.
 * The form still surfaces the key as a read-only display field; this schema
 * only validates the editable subset.
 */
export const eventDefinitionEditSchema = Yup.object({
  metric_type: Yup.string()
    .oneOf(METRIC_TYPES as unknown as string[], 'Invalid metric type')
    .required('Metric type is required'),

  description: Yup.string().max(500, 'Description must be 500 characters or fewer'),

  schema: Yup.string().test('valid-json-or-empty', 'Schema must be valid JSON', (value) => {
    if (!value || value.trim() === '') return true
    try {
      JSON.parse(value)
      return true
    } catch {
      return false
    }
  }),
})

export type EventDefinitionEditValues = Yup.InferType<typeof eventDefinitionEditSchema>
