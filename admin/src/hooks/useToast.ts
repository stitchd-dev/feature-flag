import { useState, useCallback } from 'react'

export interface ToastItem {
  id: string
  message: string
  kind: 'success' | 'error' | 'info'
  action?: { label: string; onClick: () => void }
}

export function useToast() {
  const [toasts, setToasts] = useState<ToastItem[]>([])

  const toast = useCallback((message: string, kind: ToastItem['kind'] = 'info', action?: ToastItem['action']) => {
    const id = Math.random().toString(36).slice(2, 9)
    setToasts((prev) => [...prev, { id, message, kind, action }])
    return id
  }, [])

  const dismiss = useCallback((id: string) => {
    setToasts((prev) => prev.filter((t) => t.id !== id))
  }, [])

  return { toasts, toast, dismiss }
}
