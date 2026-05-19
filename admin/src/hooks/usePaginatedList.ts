/**
 * usePaginatedList — generic paginated data fetcher.
 *
 * URL-driven via useSearchParams (?page=N), AbortController on dep change,
 * error mapping via extractErrorMessage.
 *
 * Signature:
 *   usePaginatedList<T>(
 *     fetcher: (params: { page: number; perPage: number; signal: AbortSignal }) => Promise<{ items: T[]; total: number }>,
 *     deps: unknown[]
 *   )
 *
 * Returns:
 *   { data, total, loading, error, page, perPage, onPageChange }
 */
import { useState, useEffect, useLayoutEffect, useCallback, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { extractErrorMessage } from '../lib/errors'

const DEFAULT_PER_PAGE = 50

export interface PaginatedListResult<T> {
  data: T[]
  total: number
  loading: boolean
  error: string | null
  page: number
  perPage: number
  onPageChange: (p: number) => void
  /** Increment to force a refresh without changing page/deps */
  refresh: () => void
}

export function usePaginatedList<T>(
  fetcher: (params: { page: number; perPage: number; signal: AbortSignal }) => Promise<{ items: T[]; total: number }>,
  deps: unknown[],
  perPage: number = DEFAULT_PER_PAGE,
): PaginatedListResult<T> {
  const [searchParams, setSearchParams] = useSearchParams()
  const page = Math.max(1, Number(searchParams.get('page') ?? 1))

  const [data, setData] = useState<T[]>([])
  const [total, setTotal] = useState(0)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [tick, setTick] = useState(0)

  // Stable ref to fetcher to avoid spurious re-runs.
  // Use useLayoutEffect (not render-time assignment) to satisfy react-hooks/no-direct-mutation-in-render.
  const fetcherRef = useRef(fetcher)
  useLayoutEffect(() => {
    fetcherRef.current = fetcher
  })

  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    setError(null)
    fetcherRef.current({ page, perPage, signal: controller.signal })
      .then(({ items, total: t }) => {
        if (controller.signal.aborted) return
        setData(items)
        setTotal(t)
      })
      .catch((err: unknown) => {
        if (controller.signal.aborted) return
        setError(extractErrorMessage(err))
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page, perPage, tick, ...deps])

  const onPageChange = useCallback(
    (p: number) => {
      setSearchParams((prev) => {
        prev.set('page', String(p))
        return prev
      })
    },
    [setSearchParams],
  )

  const refresh = useCallback(() => setTick((t) => t + 1), [])

  return { data, total, loading, error, page, perPage, onPageChange, refresh }
}
