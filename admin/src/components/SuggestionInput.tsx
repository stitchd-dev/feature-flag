import React, { useEffect, useRef, useState } from 'react'

export interface SuggestionItem {
  label: string
  isPrivate?: boolean
}

interface SuggestionInputProps {
  value: string
  onChange: (value: string) => void
  suggestions: SuggestionItem[]
  placeholder?: string
  disabled?: boolean
  className?: string
  style?: React.CSSProperties
}

/**
 * Text input with a keyboard-navigable dropdown of suggestions.
 * Private params are displayed with a 🔒 badge.
 */
export function SuggestionInput({
  value,
  onChange,
  suggestions,
  placeholder,
  disabled,
  className,
  style,
}: SuggestionInputProps) {
  const [open, setOpen] = useState(false)
  const [activeIndex, setActiveIndex] = useState(-1)
  const containerRef = useRef<HTMLDivElement>(null)

  const filtered = suggestions.filter((s) =>
    s.label.toLowerCase().includes(value.toLowerCase()),
  )

  useEffect(() => {
    setActiveIndex(-1)
  }, [value])

  useEffect(() => {
    function handleClickOutside(e: MouseEvent) {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', handleClickOutside)
    return () => document.removeEventListener('mousedown', handleClickOutside)
  }, [])

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (!open || filtered.length === 0) {
      if (e.key === 'ArrowDown' && filtered.length > 0) {
        setOpen(true)
        setActiveIndex(0)
      }
      return
    }

    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setActiveIndex((i) => Math.min(i + 1, filtered.length - 1))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setActiveIndex((i) => Math.max(i - 1, 0))
    } else if (e.key === 'Enter' && activeIndex >= 0) {
      e.preventDefault()
      onChange(filtered[activeIndex].label)
      setOpen(false)
    } else if (e.key === 'Escape') {
      setOpen(false)
    }
  }

  return (
    <div ref={containerRef} style={{ position: 'relative', display: 'inline-block', ...style }}>
      <input
        type="text"
        value={value}
        onChange={(e) => {
          onChange(e.target.value)
          setOpen(true)
        }}
        onFocus={() => setOpen(true)}
        onKeyDown={handleKeyDown}
        placeholder={placeholder}
        disabled={disabled}
        className={className}
        style={{ width: '100%', boxSizing: 'border-box' as const }}
        role="combobox"
        aria-autocomplete="list"
        aria-expanded={open && filtered.length > 0}
      />
      {open && filtered.length > 0 && (
        <ul
          role="listbox"
          style={{
            position: 'absolute',
            top: '100%',
            left: 0,
            right: 0,
            zIndex: 50,
            background: '#fff',
            border: '1px solid #e2e8f0',
            borderRadius: 6,
            boxShadow: '0 4px 12px rgba(0,0,0,0.1)',
            listStyle: 'none',
            margin: 0,
            padding: '4px 0',
            maxHeight: 220,
            overflowY: 'auto',
          }}
        >
          {filtered.map((item, idx) => (
            <li
              key={item.label}
              role="option"
              aria-selected={idx === activeIndex}
              onMouseEnter={() => setActiveIndex(idx)}
              onMouseDown={(e) => {
                e.preventDefault()
                onChange(item.label)
                setOpen(false)
              }}
              style={{
                padding: '6px 12px',
                cursor: 'pointer',
                display: 'flex',
                alignItems: 'center',
                gap: 6,
                background: idx === activeIndex ? '#f1f5f9' : 'transparent',
                fontSize: 13,
              }}
            >
              <span>{item.label}</span>
              {item.isPrivate && (
                <span title="Private parameter" style={{ fontSize: 12 }}>
                  🔒
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
