import * as Yup from 'yup'

export const SEGMENT_TYPES = ['list', 'rule'] as const
export type SegmentType = typeof SEGMENT_TYPES[number]

/** Segment create / edit. */
export const segmentSchema = Yup.object({
  name: Yup.string()
    .trim()
    .min(1, 'Name is required')
    .max(120, 'Name must be 120 characters or fewer')
    .required('Name is required'),

  segment_type: Yup.string()
    .oneOf(SEGMENT_TYPES as unknown as string[], 'Invalid segment type')
    .required('Segment type is required'),

  description: Yup.string().max(500, 'Description must be 500 characters or fewer'),

  /** Comma-separated tags — stored as string in the form, split on submit. */
  tags: Yup.string(),

  context_type: Yup.string().when('segment_type', {
    is: 'list',
    then: (s) =>
      s
        .trim()
        .min(1, 'Context type is required for list segments')
        .required('Context type is required'),
    otherwise: (s) => s.optional(),
  }),

  user_list: Yup.string(),
})

export type SegmentFormValues = Yup.InferType<typeof segmentSchema>
