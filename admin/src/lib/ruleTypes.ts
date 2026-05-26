// TypeScript mirror of the Rust rule engine domain types (serde_json compatible).
//
// ConditionExpr is serde-encoded as externally-tagged enums:
//   {"Leaf": <Condition>}  {"And": [...]}  {"Or": [...]}  {"Not": <ConditionExpr>}
//
// ParameterValue is serde(untagged): bool | number | string maps transparently.

export type ParameterValue = boolean | number | string

// ─── Condition (leaf tests) ──────────────────────────────────────────────────

type ComparePayload = { context_type: string; param: string; value: ParameterValue }
type StringPayload = ComparePayload & { value: string }

export type Condition =
  | { Eq: ComparePayload }
  | { Ne: ComparePayload }
  | { Lt: ComparePayload }
  | { Lte: ComparePayload }
  | { Gt: ComparePayload }
  | { Gte: ComparePayload }
  | { Contains: { context_type: string; param: string; substr: string } }
  | { StartsWith: { context_type: string; param: string; prefix: string } }
  | { EndsWith: { context_type: string; param: string; suffix: string } }
  | { SemverGte: StringPayload }
  | { SemverTilde: StringPayload }
  | { SemverCaret: StringPayload }
  | { InSegment: string }
  | { NotInSegment: string }
  | { FlagEvaluatedAs: { flag_id: string; variant_id: string } }

// ─── ConditionExpr (recursive tree) ─────────────────────────────────────────

export type ConditionExpr =
  | { Leaf: Condition }
  | { And: ConditionExpr[] }
  | { Or: ConditionExpr[] }
  | { Not: ConditionExpr }

// ─── Rule output (gateway wire format) ──────────────────────────────────────

export interface AllocationBucket {
  variant_key: string
  /** Basis points (0–10000). 10000 = 100 %. */
  weight_bp: number
}

/**
 * One input to the percentage hash — legacy wire shape preserved for
 * backwards-compatibility (the gateway still emits `hash_targets` on read).
 * The Phase 4 cutover introduced `hash_inputs` (a tagged union) as the
 * canonical authoring shape; new payloads we build set `hash_inputs` and
 * leave `hash_targets` to the gateway to derive server-side.
 *
 * `field === "key"` → hash Context.key.
 * Any other string → hash Context.parameters[field].
 */
export interface HashTarget {
  context_type: string
  field: string // "key" or a parameter name
}

import type { HashSelector } from './hashInputTypes'
export type { HashSelector }

export interface AllocationOutput {
  /**
   * Canonical Phase 4 selector list. Required on write (the admin UI
   * authors this) and present on read for all post-Phase-4 payloads.
   */
  hash_inputs: HashSelector[]
  /**
   * Legacy projection — still emitted by the gateway for older readers,
   * still accepted on input. The Phase 4 admin UI builds `hash_inputs` and
   * derives `hash_targets` on the fly via `hashTargetsFromInputs` so the
   * legacy shape stays consistent until the gateway drops it.
   */
  hash_targets: HashTarget[]
  /** Must sum to 1000 (= 100%). */
  buckets: AllocationBucket[]
}

/**
 * Project a canonical `HashSelector` list to the legacy `HashTarget` shape.
 *
 * Used only when persisting allocation outputs — the gateway prefers
 * `hash_inputs` but still validates `hash_targets`, so we send both. After
 * the gateway drops `hash_targets`, this helper becomes dead code.
 */
export function hashTargetsFromInputs(inputs: HashSelector[]): HashTarget[] {
  return inputs.map((s) => {
    if (s.kind === 'context_key') {
      return { context_type: s.context_type, field: 'key' }
    }
    return { context_type: s.context_type, field: s.parameter }
  })
}

/**
 * Reverse projection — convert a legacy `HashTarget` list to the canonical
 * `HashSelector` shape. Used by `normalizeOutput` to upgrade older
 * payloads in-memory so the admin UI always works with `hash_inputs`.
 */
export function hashInputsFromTargets(targets: HashTarget[]): HashSelector[] {
  return targets.map((t) => {
    if (t.field === 'key' || t.field === '') {
      return { kind: 'context_key', context_type: t.context_type }
    }
    return {
      kind: 'context_parameter',
      context_type: t.context_type,
      parameter: t.field,
    }
  })
}

export type RuleOutputJson =
  | { variant_key: string }
  | { allocation: AllocationOutput }

/**
 * Normalise legacy wire format (allocation as bare array) to the current
 * object format. Call this when parsing rules received from the backend.
 *
 * Phase 4 cutover: payloads may now carry either `hash_inputs` (canonical),
 * `hash_targets` (legacy), or both. This normaliser ALWAYS produces a
 * canonical `hash_inputs` field — when absent, it's derived from
 * `hash_targets` via `hashInputsFromTargets`.
 */
export function normalizeOutput(raw: unknown): RuleOutputJson {
  if (!raw || typeof raw !== 'object') return { variant_key: '' }
  const o = raw as Record<string, unknown>
  if ('variant_key' in o) return { variant_key: String(o.variant_key ?? '') }
  if ('allocation' in o) {
    const alloc = o.allocation
    if (Array.isArray(alloc)) {
      // Legacy: bare array — migrate to object form with user.key as default target.
      const targets: HashTarget[] = [{ context_type: 'user', field: 'key' }]
      return {
        allocation: {
          hash_inputs: hashInputsFromTargets(targets),
          hash_targets: targets,
          buckets: alloc as AllocationBucket[],
        },
      }
    }
    const a = alloc as Partial<AllocationOutput>
    const targets: HashTarget[] = Array.isArray(a.hash_targets)
      ? a.hash_targets
      : [{ context_type: 'user', field: 'key' }]
    const inputs: HashSelector[] = Array.isArray(a.hash_inputs)
      ? a.hash_inputs
      : hashInputsFromTargets(targets)
    return {
      allocation: {
        hash_inputs: inputs,
        // Keep `hash_targets` in sync with `hash_inputs` so older readers
        // see a consistent projection. The two fields always describe the
        // same selector list — the parent UI mutates `hash_inputs` and
        // derives `hash_targets` via `hashTargetsFromInputs` on persist.
        hash_targets: hashTargetsFromInputs(inputs),
        buckets: Array.isArray(a.buckets) ? a.buckets : [],
      },
    }
  }
  return { variant_key: '' }
}

// ─── Working rule state (for the rule builder UI) ───────────────────────────

export interface RuleState {
  /** Stable local key for React list reconciliation — not sent to backend. */
  _localId: string
  /** Optional human-readable label. Stored as rule metadata; ignored by the evaluator. */
  name?: string
  condition: ConditionExpr
  output: RuleOutputJson
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/** Return the key of a Condition variant, e.g. "Eq", "InSegment". */
export function conditionKey(c: Condition): string {
  return Object.keys(c)[0]
}

/** Return the key of a ConditionExpr variant, e.g. "Leaf", "And". */
export function exprKey(e: ConditionExpr): string {
  return Object.keys(e)[0]
}

/** True when output is a single-variant output. */
export function isVariantOutput(o: RuleOutputJson): o is { variant_key: string } {
  return 'variant_key' in o
}

/** Sum of weight_bp values in an allocation output. Should equal 10000. */
export function allocationSum(buckets: AllocationBucket[]): number {
  return buckets.reduce((s, b) => s + b.weight_bp, 0)
}

/** Build a default AllocationOutput for the given variant list. */
export function defaultAllocationOutput(variants: string[]): AllocationOutput {
  const each = Math.floor(10000 / Math.max(variants.length, 1))
  const leftover = 10000 - each * variants.length
  const inputs: HashSelector[] = [{ kind: 'context_key', context_type: 'user' }]
  return {
    hash_inputs: inputs,
    hash_targets: hashTargetsFromInputs(inputs),
    buckets: variants.map((v, i) => ({
      variant_key: v,
      weight_bp: each + (i === 0 ? leftover : 0),
    })),
  }
}

/** Create a default leaf condition for a new rule. */
export function defaultCondition(): ConditionExpr {
  return { Leaf: { Eq: { context_type: 'user', param: '', value: '' } } }
}

/** Create a default variant output targeting the given variant key. */
export function defaultOutput(variantKey: string): RuleOutputJson {
  return { variant_key: variantKey }
}

/** Generate a short random local ID. */
export function localId(): string {
  return Math.random().toString(36).slice(2, 9)
}

/**
 * True when this rule's condition is the always-true sentinel (`And: []`).
 * The Rust evaluator treats an empty AND as always-matching, making it the
 * natural catch-all / default rule at the end of the rules array.
 */
export function isCatchAll(rule: RuleState): boolean {
  const expr = rule.condition
  return 'And' in expr && (expr as { And: ConditionExpr[] }).And.length === 0
}

/** Build a new catch-all rule (always-true condition, single-variant output). */
export function defaultCatchAll(variantKey: string): RuleState {
  return {
    _localId: localId(),
    condition: { And: [] },
    output: { variant_key: variantKey },
  }
}

/** True when output is a percentage allocation (not single variant). */
export function isAllocationOutput(o: RuleOutputJson): o is { allocation: AllocationOutput } {
  return 'allocation' in o
}
