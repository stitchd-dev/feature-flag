/**
 * Yup schema for the exclusion-group create / edit form (Phase 8 — xexp).
 *
 * Both `name` and `description` are optional on the wire (the gateway accepts
 * `{name?, description?}`), but the create/edit modal requires a non-empty
 * name so groups are identifiable in the picker + capacity views.
 */
import * as Yup from 'yup'

export interface ExclusionGroupFormValues {
  name: string
  description?: string
  /**
   * Randomization unit the group diverts on (e.g. "user"). Required on create,
   * immutable on edit. Every member experiment must randomize on this unit.
   */
  unit_context_type: string
}

export const exclusionGroupSchema: Yup.ObjectSchema<ExclusionGroupFormValues> =
  Yup.object({
    name: Yup.string()
      .trim()
      .min(1, 'Name is required')
      .max(120, 'Name must be 120 characters or fewer')
      .required('Name is required'),

    description: Yup.string()
      .trim()
      .max(500, 'Description must be 500 characters or fewer'),

    unit_context_type: Yup.string()
      .trim()
      .min(1, 'Diversion unit is required')
      .required('Diversion unit is required'),
  })
