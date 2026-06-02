/**
 * Create / edit modal for a mutual-exclusion group (Phase 8 — xexp).
 *
 * Mirrors the segment + experiment modal conventions: Formik + Yup, the shared
 * `Modal` primitive with a header/footer slot, and `FormErrorBanner` for
 * server-side failures. Edit mode passes the group's `version` through on PATCH
 * for optimistic-concurrency.
 */
import { Formik, Form } from 'formik'
import { I } from '../../../components/icons'
import { Modal } from '../../../components/Modal'
import { FormField } from '../../../components/form/FormField'
import { FormSelect } from '../../../components/form/FormSelect'
import { FormTextarea } from '../../../components/form/FormTextarea'
import { FormErrorBanner } from '../../../components/form/FormErrorBanner'
import { FormSubmit } from '../../../components/form/FormSubmit'
import { useContextTypeSuggestions } from '../../../hooks/useContextSuggestions'
import { extractErrorMessage } from '../../../lib/errors'
import {
  exclusionGroupSchema,
  type ExclusionGroupFormValues,
} from '../../../lib/validation/exclusionGroupSchema'
import {
  createExclusionGroup,
  updateExclusionGroup,
  type ExclusionGroup,
} from '../../../lib/api/exclusionGroups'

/**
 * Build the diversion-unit <select> options from the env's known context
 * types. Always includes "user" (the default unit) and, in edit mode, the
 * group's current unit — even if it's no longer reported as a context type —
 * so the select reflects the persisted value.
 */
export function buildUnitOptions(
  contextTypes: string[],
  currentUnit?: string,
): { value: string; label: string }[] {
  const seen = new Set<string>()
  const values: string[] = []
  for (const ct of ['user', ...contextTypes, currentUnit ?? '']) {
    const v = ct.trim()
    if (!v || seen.has(v)) continue
    seen.add(v)
    values.push(v)
  }
  return values.map((v) => ({ value: v, label: v }))
}

interface Props {
  envId: string
  /** When present, the modal opens in edit mode for this group. */
  group?: ExclusionGroup
  onClose: () => void
  onSaved: (group: ExclusionGroup) => void
}

export function ExclusionGroupModal({ envId, group, onClose, onSaved }: Props) {
  const isEdit = Boolean(group)

  // Source the diversion-unit options from the env's known context types.
  // Always include "user" (the default) and the group's existing unit so the
  // select never renders an empty/mismatched value.
  const { data: contextTypes } = useContextTypeSuggestions(envId)
  const unitOptions = buildUnitOptions(
    contextTypes.map((c) => c.context_type),
    group?.unit_context_type,
  )

  const initialValues: ExclusionGroupFormValues = {
    name: group?.name ?? '',
    description: group?.description ?? '',
    unit_context_type: group?.unit_context_type ?? 'user',
  }

  async function handleSubmit(
    values: ExclusionGroupFormValues,
    { setStatus }: { setStatus: (s: unknown) => void },
  ) {
    const name = values.name.trim()
    const description = values.description?.trim() || undefined
    try {
      const saved =
        isEdit && group
          ? // unit_context_type is immutable — never sent on PATCH.
            await updateExclusionGroup(envId, group.id, {
              name,
              description,
              version: group.version,
            })
          : await createExclusionGroup(envId, {
              name,
              description,
              unit_context_type: values.unit_context_type.trim(),
            })
      onSaved(saved)
    } catch (err: unknown) {
      setStatus({ error: extractErrorMessage(err) })
    }
  }

  const header = (
    <div
      className="card-header"
      style={{
        padding: '16px 20px',
        borderBottom: '1px solid var(--border)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
      }}
    >
      <div className="card-title">
        <I.layers size={15} /> {isEdit ? 'Edit exclusion group' : 'New exclusion group'}
      </div>
      <button type="button" className="icon-btn" onClick={onClose}>
        <I.x size={16} />
      </button>
    </div>
  )

  const footer = (
    <div style={{ display: 'flex', gap: 10, justifyContent: 'flex-end' }}>
      <button type="button" className="btn" onClick={onClose}>
        Cancel
      </button>
      <FormSubmit
        label={isEdit ? 'Save changes' : 'Create group'}
        loadingLabel={isEdit ? 'Saving…' : 'Creating…'}
        form="exclusion-group-form"
      />
    </div>
  )

  return (
    <Formik
      initialValues={initialValues}
      validationSchema={exclusionGroupSchema}
      onSubmit={handleSubmit}
    >
      <Modal isOpen onClose={onClose} size="md" title={header} footer={footer}>
        <Form
          id="exclusion-group-form"
          style={{ display: 'flex', flexDirection: 'column', gap: 16 }}
        >
          <FormErrorBanner />
          <FormField
            name="name"
            label="Name"
            placeholder="e.g. Checkout funnel"
            autoFocus
          />
          <FormSelect
            name="unit_context_type"
            label="Diversion unit"
            options={unitOptions}
            disabled={isEdit}
            hint={
              isEdit
                ? 'The diversion unit is fixed once a group is created.'
                : 'All experiments in this group must randomize on this unit.'
            }
          />
          <FormTextarea
            name="description"
            label="Description"
            placeholder="Optional — what these experiments share."
            hint="Experiments in the same group never share traffic."
            style={{ minHeight: 64 }}
          />
        </Form>
      </Modal>
    </Formik>
  )
}
