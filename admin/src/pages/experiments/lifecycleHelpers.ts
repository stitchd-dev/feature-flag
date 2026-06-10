/**
 * Experiment lifecycle helpers: the status → valid-transitions state machine and
 * a timestamp-derived lifecycle timeline. Pure functions — no fabricated data.
 *
 * Gateway status strings: `draft`, `running` (=ACTIVE), `paused`, `concluded`.
 * The transitions endpoint accepts target statuses `active`/`paused`/`concluded`
 * (and `draft`); `active`|`running` both map to ACTIVE server-side.
 */
import type { ExperimentTransitionStatus } from '../../lib/api'

export interface TransitionAction {
  /** Button label shown to the operator. */
  label: string
  /** Target status sent to the transitions endpoint. */
  target: ExperimentTransitionStatus
  /** Whether to style/confirm the action as destructive. */
  danger: boolean
  /** Confirm-dialog body. */
  confirm: string
}

const PAUSE: TransitionAction = {
  label: 'Pause',
  target: 'paused',
  danger: false,
  confirm: 'Pause this experiment? Enrolment stops while paused; assignments are preserved.',
}
const RESUME: TransitionAction = {
  label: 'Resume',
  target: 'active',
  danger: false,
  confirm: 'Resume this experiment? Enrolment and stats computation continue.',
}
const START: TransitionAction = {
  label: 'Start',
  target: 'active',
  danger: false,
  confirm: 'Start this experiment? It will begin enrolling traffic and locking its bound flag.',
}
const CONCLUDE: TransitionAction = {
  label: 'Conclude',
  target: 'concluded',
  danger: true,
  confirm: 'Conclude this experiment? This is terminal — it stops enrolment and unlocks the bound flag.',
}

/** Valid transition actions for the current status. Empty for terminal/unknown. */
export function allowedTransitions(status: string): TransitionAction[] {
  switch (status) {
    case 'draft':
      return [START]
    case 'running':
    case 'active':
      return [PAUSE, CONCLUDE]
    case 'paused':
      return [RESUME, CONCLUDE]
    default:
      // concluded + anything unrecognised → no actions (fail safe).
      return []
  }
}

export interface TimelineStage {
  label: string
  /** RFC3339 timestamp; always a real value (stages without one are omitted). */
  at: string
}

/** Build a lifecycle timeline from the experiment's real timestamps only. */
export function lifecycleTimeline(exp: {
  created_at: string | null
  started_at: string | null
  ended_at: string | null
  status: string
}): TimelineStage[] {
  const stages: TimelineStage[] = []
  if (exp.created_at) stages.push({ label: 'Created', at: exp.created_at })
  if (exp.started_at) stages.push({ label: 'Started', at: exp.started_at })
  if (exp.ended_at) stages.push({ label: 'Ended', at: exp.ended_at })
  return stages
}
