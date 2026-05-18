/**
 * Dropdown primitive — controlled or uncontrolled open/close state,
 * click-outside dismiss, Escape-key dismiss, focus management on open.
 *
 * Two APIs:
 *   • Simple list: `items={[{label, onClick, active?}]}` — renders a standard item list.
 *   • Arbitrary panel: `children` — renders children inside the panel.
 *
 * Controlled: pass `isOpen` + `onOpenChange`.
 * Uncontrolled: omit both; component manages state internally.
 */
import { useState, useRef, useEffect, useCallback, type ReactNode } from 'react'

export interface DropdownItem {
  label: ReactNode
  onClick: () => void
  active?: boolean
  disabled?: boolean
}

interface BaseProps {
  trigger: ReactNode
  /** Controlled open state */
  isOpen?: boolean
  /** Controlled open-change callback */
  onOpenChange?: (open: boolean) => void
  /** CSS class on the root wrapper */
  className?: string
  /** Inline style on the root wrapper */
  style?: React.CSSProperties
  /** CSS class on the panel */
  panelClassName?: string
  /** Inline style on the panel */
  panelStyle?: React.CSSProperties
}

interface WithItems extends BaseProps {
  items: DropdownItem[]
  children?: never
}

interface WithChildren extends BaseProps {
  children: ReactNode
  items?: never
}

type Props = WithItems | WithChildren

export function Dropdown({
  trigger,
  isOpen: controlledOpen,
  onOpenChange,
  className,
  style,
  panelClassName,
  panelStyle,
  items,
  children,
}: Props) {
  const isControlled = controlledOpen !== undefined
  const [internalOpen, setInternalOpen] = useState(false)
  const open = isControlled ? controlledOpen : internalOpen

  const rootRef = useRef<HTMLDivElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)

  const setOpen = useCallback(
    (next: boolean) => {
      if (!isControlled) setInternalOpen(next)
      onOpenChange?.(next)
    },
    [isControlled, onOpenChange],
  )

  // Click-outside dismiss
  useEffect(() => {
    if (!open) return
    function onMouseDown(e: MouseEvent) {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', onMouseDown)
    return () => document.removeEventListener('mousedown', onMouseDown)
  }, [open, setOpen])

  // Escape-key dismiss
  useEffect(() => {
    if (!open) return
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        setOpen(false)
        // Return focus to trigger element
        const btn = rootRef.current?.querySelector<HTMLElement>('[data-dropdown-trigger]')
        btn?.focus()
      }
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [open, setOpen])

  // Focus first focusable element in panel on open
  useEffect(() => {
    if (!open || !panelRef.current) return
    const focusable = panelRef.current.querySelector<HTMLElement>(
      'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
    )
    focusable?.focus()
  }, [open])

  return (
    <div ref={rootRef} style={{ position: 'relative', ...style }} className={className}>
      {/* Wrap trigger so we can mark it for focus-restore on Escape */}
      <div
        data-dropdown-trigger="true"
        tabIndex={-1}
        onClick={() => setOpen(!open)}
        style={{ outline: 'none' }}
      >
        {trigger}
      </div>

      {open && (
        <div
          ref={panelRef}
          role="menu"
          className={panelClassName}
          style={{
            position: 'absolute',
            top: '100%',
            left: 0,
            right: 0,
            zIndex: 100,
            background: 'var(--surface)',
            border: '1px solid var(--border)',
            borderRadius: 8,
            boxShadow: 'var(--shadow-md)',
            overflow: 'hidden',
            marginTop: 4,
            ...panelStyle,
          }}
        >
          {items
            ? items.map((item, i) => (
                <button
                  key={i}
                  role="menuitem"
                  className={`dropdown-item${item.active ? ' active' : ''}`}
                  disabled={item.disabled}
                  onClick={() => {
                    item.onClick()
                    setOpen(false)
                  }}
                >
                  {item.label}
                </button>
              ))
            : children}
        </div>
      )}
    </div>
  )
}
