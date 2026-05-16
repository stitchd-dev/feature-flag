import React from 'react'
import { I } from './icons'

// ============================================
// StitchdMark brand glyph
// ============================================
export function StitchdMark({ size = 28, radius = 7 }: { size?: number; radius?: number }) {
  return (
    <div
      className="brand-mark"
      style={{ width: size, height: size, borderRadius: radius }}
    >
      <svg
        width={size * 0.55}
        height={size * 0.55}
        viewBox="0 0 24 24"
        fill="none"
        stroke="white"
        strokeWidth="2.4"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M3 11h6l3-3M3 14h4M9 17h2" />
        <circle cx="14" cy="8" r="2" fill="white" stroke="none" />
        <circle cx="14" cy="14" r="2" fill="white" stroke="none" />
        <circle cx="11" cy="17" r="1.5" fill="white" stroke="none" />
      </svg>
    </div>
  )
}

// ============================================
// Sparkline SVG chart
// ============================================
interface SparklineProps {
  data: number[]
  color?: string
  height?: number
}

export function Sparkline({ data, color = 'var(--accent)', height = 36 }: SparklineProps) {
  if (!data || !data.length) return null
  const max = Math.max(...data, 1)
  const min = Math.min(...data, 0)
  const range = max - min || 1
  const w = 120
  const h = height
  const pts = data
    .map((d, i) => {
      const x = (i / (data.length - 1)) * w
      const y = h - ((d - min) / range) * (h - 4) - 2
      return `${x},${y}`
    })
    .join(' ')
  const areaPts = `0,${h} ${pts} ${w},${h}`
  return (
    <svg
      className="sparkline"
      viewBox={`0 0 ${w} ${h}`}
      preserveAspectRatio="none"
    >
      <polygon points={areaPts} fill={color} opacity="0.10" />
      <polyline
        points={pts}
        fill="none"
        stroke={color}
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  )
}

// ============================================
// VariantBar proportional allocation visual
// ============================================
interface Variant {
  name: string
  alloc: number
  value?: unknown
}

const PALETTE = ['#F0461F', '#5BAEF5', '#3DD68C', '#A892FF', '#E6B14F', '#F26B5E']

export function VariantBar({ variants }: { variants: Variant[] }) {
  return (
    <div className="variant-bar">
      {variants.map((v, i) =>
        v.alloc > 0 ? (
          <div
            key={v.name}
            className="variant-seg"
            style={{ flex: v.alloc, background: PALETTE[i % PALETTE.length] }}
          >
            {v.alloc >= 8 && `${v.alloc}%`}
          </div>
        ) : null
      )}
    </div>
  )
}

// ============================================
// PageHeader — breadcrumbs + title + actions
// ============================================
interface PageHeaderProps {
  crumbs?: React.ReactNode[]
  title: React.ReactNode
  subtitle?: React.ReactNode
  mono?: boolean
  badge?: React.ReactNode
  actions?: React.ReactNode
}

export function PageHeader({ crumbs, title, subtitle, mono, badge, actions }: PageHeaderProps) {
  return (
    <div className="page-header">
      <div className="page-header-content">
        {crumbs && (
          <div className="breadcrumb">
            {crumbs.map((c, i) => (
              <React.Fragment key={i}>
                {i > 0 && <I.chevronRight size={11} className="crumb-sep" />}
                {React.isValidElement(c) ? c : <span className="crumb">{c}</span>}
              </React.Fragment>
            ))}
          </div>
        )}
        <h1 className="page-title">
          <span className={mono ? 'page-title-mono' : ''}>{title}</span>
          {badge}
        </h1>
        {subtitle && <div className="page-subtitle">{subtitle}</div>}
      </div>
      {actions && <div className="page-header-actions">{actions}</div>}
    </div>
  )
}
