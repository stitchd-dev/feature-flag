import type { ReactNode } from 'react'

interface Props {
  icon: ReactNode
  title: string
  desc?: string
  action?: ReactNode
}

export function EmptyState({ icon, title, desc, action }: Props) {
  return (
    <div className="empty" role="status">
      <div className="empty-icon">{icon}</div>
      <div className="empty-title">{title}</div>
      {desc && <div className="empty-desc">{desc}</div>}
      {action && <div style={{ marginTop: 8 }}>{action}</div>}
    </div>
  )
}
