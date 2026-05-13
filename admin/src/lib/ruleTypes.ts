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
  /** Tenths of a percent (0–1000). 1000 = 100 %. */
  weight_milli: number
}

/**
 * One input to the percentage hash.
 * `field === "key"` → hash Context.key.
 * Any other string → hash Context.parameters[field].
 */
export interface HashTarget {
  context_type: string
  field: string // "key" or a parameter name
}

export interface AllocationOutput {
  /** At least one target required. Default: user.key. */
  hash_targets: HashTarget[]
  /** Must sum to 1000 (= 100%). */
  buckets: AllocationBucket[]
}

export type RuleOutputJson =
  | { variant_key: string }
  | { allocation: AllocationOutput }

/**
 * Normalise legacy wire format (allocation as bare array) to the current
 * object format. Call this when parsing rules received from the backend.
 */
export function normalizeOutput(raw: unknown): RuleOutputJson {
  if (!raw || typeof raw !== 'object') return { variant_key: '' }
  const o = raw as Record<string, unknown>
  if ('variant_key' in o) return { variant_key: String(o.variant_key ?? '') }
  if ('allocation' in o) {
    const alloc = o.allocation
    if (Array.isArray(alloc)) {
      // Legacy: bare array — migrate to object form with user.key as default target
      return {
        allocation: {
          hash_targets: [{ context_type: 'user', field: 'key' }],
          buckets: alloc as AllocationBucket[],
        },
      }
    }
    return { allocation: alloc as AllocationOutput }
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

/** Sum of weight_milli values in an allocation output. Should equal 1000. */
export function allocationSum(buckets: AllocationBucket[]): number {
  return buckets.reduce((s, b) => s + b.weight_milli, 0)
}

/** Build a default AllocationOutput for the given variant list. */
export function defaultAllocationOutput(variants: string[]): AllocationOutput {
  const each = Math.floor(1000 / Math.max(variants.length, 1))
  const leftover = 1000 - each * variants.length
  return {
    hash_targets: [{ context_type: 'user', field: 'key' }],
    buckets: variants.map((v, i) => ({
      variant_key: v,
      weight_milli: each + (i === 0 ? leftover : 0),
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
