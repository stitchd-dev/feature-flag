import { useEffect, useRef } from 'react'
import { useNavigate } from 'react-router-dom'
import { I } from '../components/icons'

interface CmdItem {
  label: string
  path: string
  meta?: string
}

interface CmdSection {
  section: string
  items: CmdItem[]
}

const SECTIONS: CmdSection[] = [
  {
    section: 'Navigate',
    items: [
      { label: 'Dashboard', path: '/', meta: 'g d' },
      { label: 'Feature Flags', path: '/flags', meta: 'g f' },
      { label: 'Segments', path: '/segments', meta: 'g s' },
      { label: 'Experiments', path: '/experiments', meta: 'g e' },
      { label: 'Events', path: '/events', meta: 'g v' },
    ],
  },
  {
    section: 'Admin',
    items: [
      { label: 'Environments & SDK Keys', path: '/environments' },
      { label: 'Members & Roles', path: '/members' },
      { label: 'Audit Log', path: '/audit' },
    ],
  },
]

interface Props {
  open: boolean
  onClose: () => void
}

export function CommandPalette({ open, onClose }: Props) {
  const navigate = useNavigate()
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (open) setTimeout(() => inputRef.current?.focus(), 10)
  }, [open])

  if (!open) return null

  function go(path: string) {
    navigate(path)
    onClose()
  }

  return (
    <div className="cmdk-overlay" onClick={onClose}>
      <div className="cmdk" onClick={(e) => e.stopPropagation()}>
        <div className="cmdk-input">
          <I.search size={16} stroke="var(--fg-muted)" />
          <input ref={inputRef} placeholder="Search flags, segments, experiments…" />
          <kbd className="kbd">esc</kbd>
        </div>
        <div className="cmdk-list">
          {SECTIONS.map((sec, i) => (
            <div key={i}>
              <div className="cmdk-section-label">{sec.section}</div>
              {sec.items.map((it, j) => (
                <div
                  key={j}
                  className={`cmdk-item ${i === 0 && j === 0 ? 'selected' : ''}`}
                  onClick={() => go(it.path)}
                >
                  <I.arrowRight size={13} stroke="var(--fg-subtle)" />
                  <span>{it.label}</span>
                  {it.meta && <span className="cmdk-meta">{it.meta}</span>}
                </div>
              ))}
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
