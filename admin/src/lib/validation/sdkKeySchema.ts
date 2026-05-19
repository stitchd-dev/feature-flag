import * as Yup from 'yup'

/** SDK key create (name only — key itself is generated server-side). */
export const sdkKeySchema = Yup.object({
  name: Yup.string()
    .trim()
    .min(1, 'Name is required')
    .max(80, 'Name must be 80 characters or fewer')
    .required('Name is required'),
})

export type SdkKeyFormValues = Yup.InferType<typeof sdkKeySchema>
