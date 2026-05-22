/**
 * TypeScript mirror of the gateway's `HashSelectorJson` (Phase 4 cutover).
 *
 * Wire shape — see `crates/stitchd-gateway/src/routes/flags.rs`:
 *
 * ```json
 * { "kind": "context_key",       "context_type": "user" }
 * { "kind": "context_parameter", "context_type": "device", "parameter": "os" }
 * ```
 *
 * The serde tag is `"kind"` with `rename_all = "snake_case"`, so the
 * variants serialise as `"context_key"` / `"context_parameter"`. Mirroring
 * the same camel-case discriminator here keeps round-trip serialisation
 * trivial — no DTO layer required.
 *
 * Phase 4 introduced this canonical selector list alongside the legacy
 * `hash_targets` array. The gateway accepts both shapes on input + still
 * emits `hash_targets` for backwards compatibility, but the canonical wire
 * shape is `hash_inputs`, and the admin UI now authors that field. The
 * `hash_targets` field is dropped from new payloads we build (the gateway
 * derives it from `hash_inputs` server-side).
 */

/** Identity of one hash input — either a context key or a context parameter. */
export type HashSelector =
  | { kind: 'context_key'; context_type: string }
  | { kind: 'context_parameter'; context_type: string; parameter: string }

/** Convenience kind discriminator used in the UI's `<select>` widgets. */
export type HashSelectorKind = HashSelector['kind']

/**
 * Format one selector as a dotted display string:
 *
 *   ContextKey       → "<context_type>.key"
 *   ContextParameter → "<context_type>.params.<parameter>"
 *
 * Mirrors the worked-example banner format from `spec.md` FR-9.
 */
export function formatSelector(sel: HashSelector): string {
  if (sel.kind === 'context_key') {
    return `${sel.context_type || '?'}.key`
  }
  return `${sel.context_type || '?'}.params.${sel.parameter || '?'}`
}

/**
 * Stable identity used by uniqueness checks — matches the gateway's
 * `identity()` in `HashSelectorJson`. The reserved `__key__` sentinel can
 * never collide with a real parameter name (the schema rejects empty
 * parameters, and `__key__` is never typed by users in practice).
 */
export function selectorIdentity(sel: HashSelector): string {
  if (sel.kind === 'context_key') {
    return `${sel.context_type}|__key__`
  }
  return `${sel.context_type}|${sel.parameter}`
}

/**
 * Default selector used when the user clicks "Add hash input" on a fresh
 * percentage rule. Returns an unconfigured ContextKey so successive clicks
 * produce visually distinct rows the user can configure independently — a
 * `user.key` default would collide with the existing first row on the
 * uniqueness check and silently look like the add did nothing (feature-flag-xf2).
 */
export function defaultSelector(): HashSelector {
  return { kind: 'context_key', context_type: '' }
}
