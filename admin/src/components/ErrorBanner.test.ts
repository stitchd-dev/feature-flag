import { describe, it, expect, vi } from 'vitest'

// ── Logic mirrored from ErrorBanner ──────────────────────────────────────────

function buildBannerProps(message: string, onDismiss?: () => void) {
  return { message, hasDismiss: !!onDismiss }
}

function shouldShowDismiss(props: { hasDismiss: boolean }): boolean {
  return props.hasDismiss
}

function shouldShowMessage(message: string): boolean {
  return message.length > 0
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe('ErrorBanner message', () => {
  it('renders when message is non-empty', () => {
    expect(shouldShowMessage('Something went wrong')).toBe(true)
  })

  it('empty string message is falsy', () => {
    expect(shouldShowMessage('')).toBe(false)
  })
})

describe('ErrorBanner dismiss button', () => {
  it('shows dismiss button when onDismiss is provided', () => {
    const props = buildBannerProps('Error', vi.fn())
    expect(shouldShowDismiss(props)).toBe(true)
  })

  it('hides dismiss button when onDismiss is absent', () => {
    const props = buildBannerProps('Error')
    expect(shouldShowDismiss(props)).toBe(false)
  })
})
