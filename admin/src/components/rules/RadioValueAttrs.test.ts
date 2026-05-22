/**
 * Radio value-attribute audit (feature-flag-hw9).
 *
 * Every `<input type="radio">` in the rule editor and the hash-input
 * selector must declare an explicit `value=` so the DOM `value` reflects
 * the React state's discriminant rather than the HTML default ("on"). The
 * elements function correctly with `checked={}` alone — this is a
 * semantic-correctness fix that also unblocks form-event consumers and
 * accessibility tools that read `value`.
 *
 * The admin vitest env is `environment: 'node'` (no jsdom), so we audit
 * the source files via Vite's `?raw` import. The assertions pin both the
 * presence and the specific value of each radio.
 */
import { describe, it, expect } from 'vitest'
import HASH_INPUT_LIST from '../flag/HashInputSelectorList.tsx?raw'
import RULE_CARD from './RuleCard.tsx?raw'
import RULE_LIST from './RuleList.tsx?raw'

// ── HashInputSelectorList — key vs param ─────────────────────────────────────

describe('HashInputSelectorList — radio value attrs', () => {
  it('the key/param radio pair declares value="context_key" and value="context_parameter"', () => {
    expect(HASH_INPUT_LIST).toMatch(/value="context_key"/)
    expect(HASH_INPUT_LIST).toMatch(/value="context_parameter"/)
  })

  it('has no radio without a value attr', () => {
    const radios = HASH_INPUT_LIST.match(/<input[^>]*type="radio"[^>]*>/g) ?? []
    expect(radios.length).toBeGreaterThan(0)
    for (const tag of radios) {
      expect(tag).toMatch(/value="/)
    }
  })
})

// ── RuleCard.OutputEditor — Serve variant / Percentage rollout ───────────────

describe('RuleCard OutputEditor — radio value attrs', () => {
  it('declares value="variant" and value="allocation"', () => {
    expect(RULE_CARD).toMatch(/type="radio"[^>]*value="variant"/)
    expect(RULE_CARD).toMatch(/type="radio"[^>]*value="allocation"/)
  })

  it('has no radio without a value attr', () => {
    // Radios may span newlines; tolerate both self-closing and JSX block forms.
    const radios = RULE_CARD.match(/<input[\s\S]*?type="radio"[\s\S]*?\/>/g) ?? []
    expect(radios.length).toBeGreaterThan(0)
    for (const tag of radios) {
      expect(tag).toMatch(/value="/)
    }
  })
})

// ── RuleList.CatchallOutputEditor — Serve variant / Percentage rollout ───────

describe('RuleList CatchallOutputEditor — radio value attrs', () => {
  it('declares value="variant" and value="allocation"', () => {
    expect(RULE_LIST).toMatch(/type="radio"[^>]*value="variant"/)
    expect(RULE_LIST).toMatch(/type="radio"[^>]*value="allocation"/)
  })

  it('has no radio without a value attr', () => {
    const radios = RULE_LIST.match(/<input[\s\S]*?type="radio"[\s\S]*?\/>/g) ?? []
    expect(radios.length).toBeGreaterThan(0)
    for (const tag of radios) {
      expect(tag).toMatch(/value="/)
    }
  })
})
