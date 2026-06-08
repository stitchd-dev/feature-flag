import { ScheduleBuilder } from '../../components/schedule/ScheduleBuilder'
import type { MutationPreset } from '../../components/schedule/ScheduleBuilder'
import { useOrgContext } from '../../context/OrgContext'
import type { AdminFlagResponse } from '../../lib/types'

/**
 * Flag schedule tab (flag_lifecycle_20260604, Phase 8.2). Mounts the reusable
 * `ScheduleBuilder` with flag-appropriate `MutateFlag` presets (enable/disable
 * and a per-variant default-rule switch). Scheduled changes are environment-
 * scoped, so this needs the current `envId`.
 */
export function FlagScheduleTab({ flag }: { flag: AdminFlagResponse }) {
  const { envId } = useOrgContext()

  if (!envId) {
    return (
      <div style={{ fontSize: 13, color: 'var(--fg-muted)', padding: 12 }}>
        Select an environment to schedule changes for this flag.
      </div>
    )
  }

  const presets: MutationPreset[] = [
    { id: 'disable', label: 'Disable flag', payload: { enabled_override: false } },
    { id: 'enable', label: 'Enable flag', payload: { enabled_override: true } },
    ...flag.variants.map((v) => ({
      id: `default-${v.key}`,
      label: `Default variant → ${v.key}`,
      payload: { default_variant_key: v.key },
    })),
  ]

  return (
    <ScheduleBuilder
      envId={envId}
      entityKind="flags"
      entityId={flag.key}
      presets={presets}
    />
  )
}
