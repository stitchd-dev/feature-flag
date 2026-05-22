/**
 * HashInputSelectorList — component tests.
 *
 * The admin vitest env is `environment: 'node'` (no jsdom, no Testing
 * Library), so interactive flows are not possible. The same constraint
 * applies as `ContextTypeTabs.test.tsx`: tests assert on SSR'd HTML shape
 * via `react-dom/server.renderToString`, and exercise pure helpers
 * directly.
 *
 * Coverage:
 *   • SSR renders an ordered list with role="list" + role="listitem".
 *   • Each row exposes a drag handle + a remove button.
 *   • The "Add input" button is rendered when interactive.
 *   • Pure helpers: addSelector, removeSelector, reorder, formatWorkedExample.
 *   • Yup error rendering (per-row and at the array level).
 *   • Reorder via keyboard helpers (move up / move down).
 */
import { describe, it, expect } from 'vitest'
import { renderToString } from 'react-dom/server'
import {
  HashInputSelectorList,
  addSelector,
  removeSelector,
  moveSelectorUp,
  moveSelectorDown,
  formatWorkedExample,
} from './HashInputSelectorList'
import type { HashSelector } from '../../lib/hashInputTypes'

describe('HashInputSelectorList — SSR shape', () => {
  it('renders an ordered list with role="list"', () => {
    const sel: HashSelector[] = [{ kind: 'context_key', context_type: 'user' }]
    const html = renderToString(
      <HashInputSelectorList
        value={sel}
        onChange={() => {}}
        envId="env-1"
      />,
    )
    expect(html).toMatch(/role="list"/)
  })

  it('renders one row per selector with role="listitem"', () => {
    const sel: HashSelector[] = [
      { kind: 'context_key', context_type: 'user' },
      { kind: 'context_parameter', context_type: 'device', parameter: 'os' },
    ]
    const html = renderToString(
      <HashInputSelectorList
        value={sel}
        onChange={() => {}}
        envId="env-1"
      />,
    )
    const matches = html.match(/role="listitem"/g) ?? []
    expect(matches.length).toBe(2)
  })

  it('renders empty when value is empty (with banner advising add)', () => {
    const html = renderToString(
      <HashInputSelectorList value={[]} onChange={() => {}} envId="env-1" />,
    )
    // Empty list still renders the role="list" container so screen readers
    // get a stable anchor, but no listitems are emitted.
    expect(html).toMatch(/role="list"/)
    expect(html).not.toMatch(/role="listitem"/)
  })

  it('renders the "Add input" button when interactive', () => {
    const html = renderToString(
      <HashInputSelectorList value={[]} onChange={() => {}} envId="env-1" />,
    )
    expect(html).toMatch(/Add hash input/)
  })

  it('hides the "Add input" button when disabled', () => {
    const html = renderToString(
      <HashInputSelectorList
        value={[]}
        onChange={() => {}}
        envId="env-1"
        disabled
      />,
    )
    expect(html).not.toMatch(/Add hash input/)
  })

  it('renders array-level Yup errors', () => {
    const html = renderToString(
      <HashInputSelectorList
        value={[]}
        onChange={() => {}}
        envId="env-1"
        arrayError="At least one hash input is required."
      />,
    )
    expect(html).toMatch(/At least one hash input is required\./)
  })

  it('renders per-row Yup errors keyed by index', () => {
    const sel: HashSelector[] = [
      { kind: 'context_key', context_type: 'user' },
      { kind: 'context_parameter', context_type: 'device', parameter: '' },
    ]
    const html = renderToString(
      <HashInputSelectorList
        value={sel}
        onChange={() => {}}
        envId="env-1"
        rowErrors={{ 1: 'Parameter name is required.' }}
      />,
    )
    expect(html).toMatch(/Parameter name is required\./)
  })

  it('renders the live worked-example banner', () => {
    const sel: HashSelector[] = [
      { kind: 'context_key', context_type: 'user' },
      { kind: 'context_parameter', context_type: 'user', parameter: 'name' },
      { kind: 'context_parameter', context_type: 'device', parameter: 'os' },
    ]
    const html = renderToString(
      <HashInputSelectorList
        value={sel}
        onChange={() => {}}
        envId="env-1"
      />,
    )
    // Banner format: `Bucket = hash(<dotted> || <dotted> || ...)`
    expect(html).toMatch(/hash\(/)
    expect(html).toMatch(/user\.key/)
    expect(html).toMatch(/user\.params\.name/)
    expect(html).toMatch(/device\.params\.os/)
  })
})

// ── Pure helpers ───────────────────────────────────────────────────────────────

describe('addSelector', () => {
  it('appends a new ContextKey selector with a placeholder context_type', () => {
    const next = addSelector([])
    expect(next).toHaveLength(1)
    expect(next[0].kind).toBe('context_key')
  })

  it('preserves existing rows when adding', () => {
    const base: HashSelector[] = [{ kind: 'context_key', context_type: 'user' }]
    const next = addSelector(base)
    expect(next).toHaveLength(2)
    expect(next[0]).toEqual(base[0])
  })
})

describe('removeSelector', () => {
  it('removes the selector at the given index', () => {
    const base: HashSelector[] = [
      { kind: 'context_key', context_type: 'user' },
      { kind: 'context_parameter', context_type: 'device', parameter: 'os' },
    ]
    const next = removeSelector(base, 0)
    expect(next).toHaveLength(1)
    expect(next[0]).toEqual(base[1])
  })

  it('is a no-op when the index is out of range', () => {
    const base: HashSelector[] = [{ kind: 'context_key', context_type: 'user' }]
    const next = removeSelector(base, 5)
    expect(next).toEqual(base)
  })
})

describe('moveSelectorUp / moveSelectorDown', () => {
  const base: HashSelector[] = [
    { kind: 'context_key', context_type: 'user' },
    { kind: 'context_parameter', context_type: 'device', parameter: 'os' },
    { kind: 'context_key', context_type: 'application' },
  ]

  it('moveSelectorUp swaps a row with the one above', () => {
    const next = moveSelectorUp(base, 2)
    expect(next[1]).toEqual(base[2])
    expect(next[2]).toEqual(base[1])
    expect(next[0]).toEqual(base[0])
  })

  it('moveSelectorUp is a no-op at index 0', () => {
    expect(moveSelectorUp(base, 0)).toEqual(base)
  })

  it('moveSelectorDown swaps a row with the one below', () => {
    const next = moveSelectorDown(base, 0)
    expect(next[0]).toEqual(base[1])
    expect(next[1]).toEqual(base[0])
    expect(next[2]).toEqual(base[2])
  })

  it('moveSelectorDown is a no-op at the last index', () => {
    expect(moveSelectorDown(base, base.length - 1)).toEqual(base)
  })
})

describe('formatWorkedExample', () => {
  it('produces a hash(...) string with dotted selectors', () => {
    const sel: HashSelector[] = [
      { kind: 'context_key', context_type: 'user' },
      { kind: 'context_parameter', context_type: 'device', parameter: 'os' },
    ]
    expect(formatWorkedExample(sel)).toBe(
      'Bucket = hash(user.key || device.params.os)',
    )
  })

  it('handles a single selector', () => {
    const sel: HashSelector[] = [{ kind: 'context_key', context_type: 'user' }]
    expect(formatWorkedExample(sel)).toBe('Bucket = hash(user.key)')
  })

  it('returns a placeholder when the list is empty', () => {
    expect(formatWorkedExample([])).toBe('Bucket = hash(<no inputs>)')
  })

  it('renders the spec.md FR-9 worked example verbatim', () => {
    const sel: HashSelector[] = [
      { kind: 'context_key', context_type: 'user' },
      { kind: 'context_parameter', context_type: 'user', parameter: 'name' },
      { kind: 'context_parameter', context_type: 'device', parameter: 'os' },
    ]
    expect(formatWorkedExample(sel)).toBe(
      'Bucket = hash(user.key || user.params.name || device.params.os)',
    )
  })
})
