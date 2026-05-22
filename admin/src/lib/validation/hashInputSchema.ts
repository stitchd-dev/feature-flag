/**
 * Yup schema for the cross-context percentage-hash selector list.
 *
 * Mirrors the gateway's `validate_hash_inputs` in
 * `crates/stitchd-gateway/src/routes/flags.rs`. The three constraints are:
 *
 *   1. At least one selector.
 *   2. `ContextParameter` selectors require a non-empty `parameter`
 *      (after trimming).
 *   3. No two selectors share the same `(context_type, field)` identity,
 *      where `field` is the `__key__` sentinel for `ContextKey` or the
 *      parameter name for `ContextParameter`.
 *
 * Authoring the same rules on both sides means the user sees friendly
 * client-side errors before submit, and the server's 422 is a safety net
 * for race conditions (e.g. concurrent edits dropping selectors).
 *
 * Used by:
 *   • `HashInputSelectorList` (per-row + array-level errors)
 *   • Rule builder pre-submit validation (allocation outputs +
 *     default-rule distribution body)
 */
import * as Yup from 'yup'
import {
  selectorIdentity,
  type HashSelector,
} from '../hashInputTypes'

/** Selector tuple — discriminated union over `kind`. */
const selectorSchema = Yup.object({
  kind: Yup.string()
    .oneOf(['context_key', 'context_parameter'])
    .required('Selector kind is required'),
  context_type: Yup.string()
    .trim()
    .min(1, 'Context type is required')
    .required('Context type is required'),
  // `parameter` is only required when kind === 'context_parameter'.
  // Yup conditional keeps the message routed to the offending row.
  parameter: Yup.string()
    .trim()
    .when('kind', {
      is: 'context_parameter',
      then: (s) =>
        s
          .min(1, 'Parameter name is required when field type is Parameter.')
          .required('Parameter name is required when field type is Parameter.'),
      otherwise: (s) => s.notRequired(),
    }),
})

export const hashInputsSchema = Yup.array()
  .of(selectorSchema)
  .min(1, 'At least one hash input is required.')
  .test(
    'unique-selectors',
    'Duplicate hash inputs: each selector must combine a unique (context type, field) pair.',
    (value) => {
      if (!value || value.length === 0) return true
      const seen = new Set<string>()
      for (const sel of value) {
        // Tolerate partially-typed rows: `selectorIdentity` reads
        // `context_type` directly, so a half-filled row still hashes
        // distinctly until both fields land in the duplicate set.
        const id = selectorIdentity(sel as HashSelector)
        if (seen.has(id)) return false
        seen.add(id)
      }
      return true
    },
  )
  .required('At least one hash input is required.')

/**
 * Inferred TS type for the validated payload — convenience for callers
 * that want to type their state from the schema rather than the wire
 * mirror.
 */
export type HashInputsFormValue = Yup.InferType<typeof hashInputsSchema>
