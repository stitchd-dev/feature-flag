/**
 * Pure helpers for `HashInputSelectorList`. Split into a non-React module
 * so the component file is component-only — keeps the
 * `react-refresh/only-export-components` lint rule happy without
 * suppressions, and gives unit tests a dependency-free import target.
 */
import {
  defaultSelector,
  formatSelector,
  type HashSelector,
} from '../../lib/hashInputTypes'

/** Append a fresh default selector to the end of the list. */
export function addSelector(value: HashSelector[]): HashSelector[] {
  return [...value, defaultSelector()]
}

/** Remove the selector at `index`. No-op when `index` is out of range. */
export function removeSelector(
  value: HashSelector[],
  index: number,
): HashSelector[] {
  if (index < 0 || index >= value.length) return value
  return value.filter((_, i) => i !== index)
}

/** Swap the row at `index` with the row above. No-op at index 0. */
export function moveSelectorUp(
  value: HashSelector[],
  index: number,
): HashSelector[] {
  if (index <= 0 || index >= value.length) return value
  const next = [...value]
  const tmp = next[index - 1]
  next[index - 1] = next[index]
  next[index] = tmp
  return next
}

/** Swap the row at `index` with the row below. No-op at the last index. */
export function moveSelectorDown(
  value: HashSelector[],
  index: number,
): HashSelector[] {
  if (index < 0 || index >= value.length - 1) return value
  const next = [...value]
  const tmp = next[index + 1]
  next[index + 1] = next[index]
  next[index] = tmp
  return next
}

/**
 * Render the live worked-example string used by the helper banner.
 *
 *   `[user.key, user.params.name, device.params.os]`
 *   →
 *   `Bucket = hash(user.key || user.params.name || device.params.os)`
 *
 * Mirrors `spec.md` FR-9.
 */
export function formatWorkedExample(value: HashSelector[]): string {
  if (value.length === 0) return 'Bucket = hash(<no inputs>)'
  return `Bucket = hash(${value.map(formatSelector).join(' || ')})`
}
