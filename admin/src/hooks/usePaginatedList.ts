/**
 * usePaginatedList — generic cursor-paginated data fetcher.
 *
 * URL-driven via useSearchParams (?cursor=<opaque>), AbortController on dep
 * change, error mapping via extractErrorMessage.
 *
 * The backend uses opaque cursor tokens (base64url) — they are treated as a
 * black box here: never parsed or constructed, only echoed back as the
 * `cursor` query param.
 *
 * Signature:
 *   usePaginatedList<T>(
 *     fetcher: (params: { cursor: string | null; limit: number; signal: AbortSignal })
 *       => Promise<{ items: T[]; next_cursor: string | null }>,
 *     deps: unknown[],
 *     limit?: number,
 *   )
 *
 * Returns:
 *   { data, loading, error, hasNext, hasPrev, onNext, onPrev, refresh }
 */
import { useState, useEffect, useLayoutEffect, useCallback, useRef } from 'react'
import { useSearchParams } from 'react-router-dom'
import { extractErrorMessage } from '../lib/errors'

const DEFAULT_LIMIT = 50

export interface PaginatedListResult<T> {
  data: T[]
  loading: boolean
  error: string | null
  /** True when the last response carried a non-null next_cursor. */
  hasNext: boolean
  /** True when there is a previous page to return to (we're not on the first page). */
  hasPrev: boolean
  /** Advance to the next page using the last response's next_cursor. No-op when !hasNext. */
  onNext: () => void
  /** Return to the previous page. No-op when !hasPrev. */
  onPrev: () => void
  /** Force a refresh of the current page without changing cursor/deps. */
  refresh: () => void
}

export function usePaginatedList<T>(
  fetcher: (params: {
    cursor: string | null
    limit: number
    signal: AbortSignal
  }) => Promise<{ items: T[]; next_cursor: string | null }>,
  deps: unknown[],
  limit: number = DEFAULT_LIMIT,
): PaginatedListResult<T> {
  const [searchParams, setSearchParams] = useSearchParams()
  const cursor = searchParams.get('cursor')

  const [data, setData] = useState<T[]>([])
  const [nextCursor, setNextCursor] = useState<string | null>(null)
  // Stack of cursors for pages we've navigated away from, enabling "Previous".
  // Each entry is the `cursor` value that rendered that earlier page (null = first page).
  const [prevStack, setPrevStack] = useState<(string | null)[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [tick, setTick] = useState(0)

  // Stable ref to fetcher to avoid spurious re-runs.
  // Use useLayoutEffect (not render-time assignment) to satisfy react-hooks/no-direct-mutation-in-render.
  const fetcherRef = useRef(fetcher)
  useLayoutEffect(() => {
    fetcherRef.current = fetcher
  })

  // When the deps change (e.g. a filter), reset navigation back to the first page.
  // Track previous deps to distinguish a dep change from a cursor change.
  const depsKey = JSON.stringify(deps)
  const prevDepsKey = useRef(depsKey)
  useEffect(() => {
    if (prevDepsKey.current !== depsKey) {
      prevDepsKey.current = depsKey
      setPrevStack([])
      setSearchParams(
        (prev) => {
          prev.delete('cursor')
          return prev
        },
        { replace: true },
      )
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [depsKey])

  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    setError(null)
    fetcherRef
      .current({ cursor, limit, signal: controller.signal })
      .then(({ items, next_cursor }) => {
        if (controller.signal.aborted) return
        setData(items)
        setNextCursor(next_cursor)
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
  }, [cursor, limit, tick, ...deps])

  const onNext = useCallback(() => {
    if (nextCursor == null) return
    setPrevStack((s) => [...s, cursor])
    setSearchParams(
      (prev) => {
        prev.set('cursor', nextCursor)
        return prev
      },
      { replace: false },
    )
  }, [nextCursor, cursor, setSearchParams])

  const onPrev = useCallback(() => {
    setPrevStack((s) => {
      if (s.length === 0) return s
      const target = s[s.length - 1]
      setSearchParams(
        (prev) => {
          if (target == null) prev.delete('cursor')
          else prev.set('cursor', target)
          return prev
        },
        { replace: false },
      )
      return s.slice(0, -1)
    })
  }, [setSearchParams])

  const refresh = useCallback(() => setTick((t) => t + 1), [])

  return {
    data,
    loading,
    error,
    hasNext: nextCursor != null,
    hasPrev: prevStack.length > 0,
    onNext,
    onPrev,
    refresh,
  }
}
