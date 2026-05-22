/**
 * ContextTypeContext — provider + hook tests.
 *
 * The admin vitest env is `environment: 'node'` (no jsdom), so interactive
 * `setState` flows aren't directly testable. We cover:
 *   1. Storage-key shape (pure helper `contextTypeStorageKey`).
 *   2. Initial-value selection logic, exercised by rendering the provider
 *      with `react-dom/server.renderToString` and reading the resolved
 *      active type out via a sentinel child component.
 *   3. `useActiveContextType()` throws when used outside the provider.
 *
 * The persistence side-effect runs in a `useEffect`, which doesn't fire
 * under SSR — interactive behaviour will be covered by playwright/e2e or a
 * future jsdom suite. Until then, the provider's initial-read path (which
 * is the load-bearing branch for restoring state after a page refresh) is
 * fully exercised here via direct localStorage shim.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { renderToString } from 'react-dom/server'
import {
  ContextTypeProvider,
  useActiveContextType,
  contextTypeStorageKey,
} from './ContextTypeContext'

// ── localStorage shim ────────────────────────────────────────────────────────
//
// In `environment: 'node'`, neither `window` nor `localStorage` exists. The
// provider guards window access (`typeof window === 'undefined'`) but the
// initial-read path reads `localStorage` directly. We install a global shim
// before each test and restore it after.

interface FakeStorage {
  store: Record<string, string>
  getItem: (k: string) => string | null
  setItem: (k: string, v: string) => void
  removeItem: (k: string) => void
  clear: () => void
  readonly length: number
  key: (i: number) => string | null
}

function makeFakeStorage(initial: Record<string, string> = {}): FakeStorage {
  const store = { ...initial }
  return {
    store,
    getItem: (k) => (k in store ? store[k] : null),
    setItem: (k, v) => {
      store[k] = String(v)
    },
    removeItem: (k) => {
      delete store[k]
    },
    clear: () => {
      Object.keys(store).forEach((k) => delete store[k])
    },
    get length() {
      return Object.keys(store).length
    },
    key: (i) => Object.keys(store)[i] ?? null,
  }
}

const globalAny = globalThis as unknown as {
  window?: unknown
  localStorage?: FakeStorage
}
let originalWindow: unknown
let originalLocalStorage: FakeStorage | undefined

beforeEach(() => {
  originalWindow = globalAny.window
  originalLocalStorage = globalAny.localStorage
  // Shim a minimal window so `typeof window === 'undefined'` is false.
  globalAny.window = { localStorage: makeFakeStorage() }
  globalAny.localStorage = (globalAny.window as { localStorage: FakeStorage })
    .localStorage
})

afterEach(() => {
  globalAny.window = originalWindow
  globalAny.localStorage = originalLocalStorage
})

// ── contextTypeStorageKey ────────────────────────────────────────────────────

describe('contextTypeStorageKey', () => {
  it('formats the key as `experiment_${id}_ctx`', () => {
    expect(contextTypeStorageKey('checkout-v2')).toBe(
      'experiment_checkout-v2_ctx',
    )
  })

  it('preserves UUID-style experiment IDs verbatim', () => {
    const id = 'a1b2c3d4-1234-5678-9abc-def012345678'
    expect(contextTypeStorageKey(id)).toBe(`experiment_${id}_ctx`)
  })
})

// ── useActiveContextType (outside provider) ──────────────────────────────────

describe('useActiveContextType', () => {
  it('throws when used outside ContextTypeProvider', () => {
    function Consumer() {
      useActiveContextType()
      return null
    }
    expect(() => renderToString(<Consumer />)).toThrow(
      /ContextTypeProvider/,
    )
  })
})

// ── ContextTypeProvider initial-value selection ──────────────────────────────

// Sentinel child renders the active context type into the SSR string so we
// can read the provider's chosen initial value back out.
function Sentinel() {
  const { activeContextType } = useActiveContextType()
  return <span data-active={activeContextType}>{activeContextType}</span>
}

describe('ContextTypeProvider — initial value', () => {
  it('uses the first context type when no saved value exists', () => {
    const html = renderToString(
      <ContextTypeProvider
        contextTypes={['user', 'account']}
        experimentId="exp-1"
      >
        <Sentinel />
      </ContextTypeProvider>,
    )
    expect(html).toMatch(/data-active="user"/)
  })

  it('restores the saved context type when valid', () => {
    globalAny.localStorage!.setItem(
      contextTypeStorageKey('exp-2'),
      'account',
    )
    const html = renderToString(
      <ContextTypeProvider
        contextTypes={['user', 'account']}
        experimentId="exp-2"
      >
        <Sentinel />
      </ContextTypeProvider>,
    )
    expect(html).toMatch(/data-active="account"/)
  })

  it('ignores a saved context type that is not in contextTypes', () => {
    globalAny.localStorage!.setItem(
      contextTypeStorageKey('exp-3'),
      'device', // not in the experiment's types
    )
    const html = renderToString(
      <ContextTypeProvider
        contextTypes={['user', 'account']}
        experimentId="exp-3"
      >
        <Sentinel />
      </ContextTypeProvider>,
    )
    // Falls back to the first available context type.
    expect(html).toMatch(/data-active="user"/)
  })

  it('falls back to "user" when contextTypes is empty', () => {
    const html = renderToString(
      <ContextTypeProvider contextTypes={[]} experimentId="exp-4">
        <Sentinel />
      </ContextTypeProvider>,
    )
    expect(html).toMatch(/data-active="user"/)
  })

  it('uses the documented key prefix `experiment_${id}_ctx`', () => {
    // Write to the documented key shape and verify the provider picked it up
    // — this pins the persistence contract (Phase 9 readers depend on it).
    globalAny.localStorage!.setItem('experiment_exp-5_ctx', 'account')
    const html = renderToString(
      <ContextTypeProvider
        contextTypes={['user', 'account', 'org']}
        experimentId="exp-5"
      >
        <Sentinel />
      </ContextTypeProvider>,
    )
    expect(html).toMatch(/data-active="account"/)
  })

  it('tolerates localStorage throwing on read (e.g. disabled storage)', () => {
    const throwingStorage = makeFakeStorage()
    throwingStorage.getItem = vi.fn(() => {
      throw new Error('storage disabled')
    })
    globalAny.localStorage = throwingStorage
    ;(globalAny.window as { localStorage: FakeStorage }).localStorage =
      throwingStorage

    const html = renderToString(
      <ContextTypeProvider
        contextTypes={['user', 'account']}
        experimentId="exp-6"
      >
        <Sentinel />
      </ContextTypeProvider>,
    )
    expect(html).toMatch(/data-active="user"/)
  })
})
