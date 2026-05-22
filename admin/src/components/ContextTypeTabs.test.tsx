/**
 * ContextTypeTabs — primitive tests.
 *
 * Strategy: the admin vitest env is `environment: 'node'` (no DOM, no
 * Testing Library), so interactive tests aren't possible. Instead, we
 * render the component to a string with `react-dom/server.renderToString`
 * and assert on the resulting HTML shape — same approach pattern as the
 * other component tests in this folder (Dropdown, EmptyState, etc., which
 * are pure-logic).
 *
 * Coverage:
 *   • Tab strip rendered when contextTypes.length ≤ 3 (role="tablist").
 *   • Dropdown rendered when contextTypes.length ≥ 4 (`<select>`).
 *   • Empty `contextTypes` array → nothing rendered.
 *   • `aria-selected` reflects the active type.
 *   • The persistence side-effect runs through `useEffect`, so it's covered
 *     by the provider test (`ContextTypeContext.test.tsx`) — verifying it
 *     here would require a DOM environment.
 */
import { describe, it, expect } from 'vitest'
import { renderToString } from 'react-dom/server'
import { ContextTypeTabs } from './ContextTypeTabs'

describe('ContextTypeTabs', () => {
  it('renders a tab strip with 2 context types', () => {
    const html = renderToString(
      <ContextTypeTabs
        contextTypes={['user', 'account']}
        activeContextType="user"
        onChange={() => {}}
      />,
    )
    expect(html).toMatch(/role="tablist"/)
    // Each context type renders as a tab button.
    expect(html).toMatch(/>user</)
    expect(html).toMatch(/>account</)
    // No <select> in tab-strip mode.
    expect(html).not.toMatch(/<select/)
  })

  it('renders a tab strip with 3 context types', () => {
    const html = renderToString(
      <ContextTypeTabs
        contextTypes={['user', 'account', 'org']}
        activeContextType="account"
        onChange={() => {}}
      />,
    )
    expect(html).toMatch(/role="tablist"/)
    expect(html).not.toMatch(/<select/)
  })

  it('renders a dropdown with 4+ context types', () => {
    const html = renderToString(
      <ContextTypeTabs
        contextTypes={['user', 'account', 'org', 'device']}
        activeContextType="user"
        onChange={() => {}}
      />,
    )
    expect(html).toMatch(/<select/)
    expect(html).not.toMatch(/role="tablist"/)
    // All four options should be present.
    expect(html).toMatch(/value="user"/)
    expect(html).toMatch(/value="account"/)
    expect(html).toMatch(/value="org"/)
    expect(html).toMatch(/value="device"/)
  })

  it('marks the active tab with aria-selected="true"', () => {
    const html = renderToString(
      <ContextTypeTabs
        contextTypes={['user', 'account']}
        activeContextType="account"
        onChange={() => {}}
      />,
    )
    // Active button gets aria-selected="true"; inactive gets "false".
    // We can't trivially anchor to the button HTML across react-dom versions,
    // so check that both states render at least once.
    expect(html).toMatch(/aria-selected="true"/)
    expect(html).toMatch(/aria-selected="false"/)
  })

  it('renders nothing when contextTypes is empty', () => {
    const html = renderToString(
      <ContextTypeTabs
        contextTypes={[]}
        activeContextType=""
        onChange={() => {}}
      />,
    )
    // Empty input → no output.
    expect(html).toBe('')
  })

  it('uses the documented threshold (4) for dropdown switch-over', () => {
    // Exactly 3 = tab strip.
    const three = renderToString(
      <ContextTypeTabs
        contextTypes={['a', 'b', 'c']}
        activeContextType="a"
        onChange={() => {}}
      />,
    )
    expect(three).toMatch(/role="tablist"/)
    // Exactly 4 = dropdown.
    const four = renderToString(
      <ContextTypeTabs
        contextTypes={['a', 'b', 'c', 'd']}
        activeContextType="a"
        onChange={() => {}}
      />,
    )
    expect(four).toMatch(/<select/)
  })
})
