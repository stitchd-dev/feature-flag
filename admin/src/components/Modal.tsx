/**
 * Modal primitive — backdrop overlay, Escape-key close, focus trap, focus-restore.
 *
 * Props:
 *   isOpen         — whether the modal is visible
 *   onClose        — called when user dismisses (Escape or backdrop click)
 *   title          — optional header content
 *   footer         — optional footer content
 *   children       — body content
 *   size           — 'sm' (400px) | 'md' (520px, default) | 'lg' (720px)
 *   closeOnBackdrop — click-backdrop closes (default true)
 */
import { useEffect, useRef, type ReactNode } from 'react'

interface Props {
  isOpen: boolean
  onClose: () => void
  title?: ReactNode
  footer?: ReactNode
  children: ReactNode
  size?: 'sm' | 'md' | 'lg'
  closeOnBackdrop?: boolean
}

const WIDTHS: Record<NonNullable<Props['size']>, number> = {
  sm: 400,
  md: 520,
  lg: 720,
}

/** Returns all keyboard-focusable elements inside a container. */
function getFocusable(container: HTMLElement): HTMLElement[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>(
      'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    ),
  )
}

export function Modal({
  isOpen,
  onClose,
  title,
  footer,
  children,
  size = 'md',
  closeOnBackdrop = true,
}: Props) {
  const dialogRef = useRef<HTMLDivElement>(null)
  // Remember the element that had focus before the modal opened
  const triggerRef = useRef<Element | null>(null)

  // Capture trigger and manage body scroll
  useEffect(() => {
    if (isOpen) {
      triggerRef.current = document.activeElement
      document.body.style.overflow = 'hidden'
    } else {
      document.body.style.overflow = ''
    }
    return () => { document.body.style.overflow = '' }
  }, [isOpen])

  // Focus first focusable element when modal opens
  useEffect(() => {
    if (!isOpen || !dialogRef.current) return
    const focusable = getFocusable(dialogRef.current)
    focusable[0]?.focus()
  }, [isOpen])

  // Restore focus to trigger on close
  useEffect(() => {
    if (!isOpen && triggerRef.current instanceof HTMLElement) {
      triggerRef.current.focus()
      triggerRef.current = null
    }
  }, [isOpen])

  // Escape-key close + focus trap
  useEffect(() => {
    if (!isOpen) return

    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        onClose()
        return
      }
      // Focus trap: cycle Tab within the modal
      if (e.key === 'Tab' && dialogRef.current) {
        const focusable = getFocusable(dialogRef.current)
        if (focusable.length === 0) { e.preventDefault(); return }
        const first = focusable[0]
        const last = focusable[focusable.length - 1]
        if (e.shiftKey) {
          if (document.activeElement === first) { e.preventDefault(); last.focus() }
        } else {
          if (document.activeElement === last) { e.preventDefault(); first.focus() }
        }
      }
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [isOpen, onClose])

  if (!isOpen) return null

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 1000,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
      }}
      role="dialog"
      aria-modal="true"
    >
      {/* Backdrop */}
      <div
        style={{ position: 'absolute', inset: 0, background: 'rgba(0,0,0,0.5)' }}
        onClick={closeOnBackdrop ? onClose : undefined}
        aria-hidden="true"
      />

      {/* Dialog panel */}
      <div
        ref={dialogRef}
        className="card"
        style={{
          position: 'relative',
          width: WIDTHS[size],
          maxHeight: '90vh',
          overflow: 'auto',
          zIndex: 1,
          padding: 0,
        }}
      >
        {title !== undefined && (
          <div className="card-header" style={{ padding: '16px 20px', borderBottom: '1px solid var(--border)' }}>
            {typeof title === 'string'
              ? <div className="card-title">{title}</div>
              : title}
          </div>
        )}

        <div style={{ padding: title !== undefined || footer !== undefined ? 20 : 0 }}>
          {children}
        </div>

        {footer !== undefined && (
          <div style={{ padding: '12px 20px', borderTop: '1px solid var(--border)' }}>
            {footer}
          </div>
        )}
      </div>
    </div>
  )
}
