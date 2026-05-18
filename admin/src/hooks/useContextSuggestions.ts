import { useEffect, useRef, useState } from 'react'
import { api } from '../lib/api'
import type { ContextTypeSuggestion, ContextParamSuggestion } from '../lib/api'

export type { ContextTypeSuggestion, ContextParamSuggestion }

interface SuggestionsState<T> {
  data: T[]
  loading: boolean
  error: string | null
}

/** Fetch context type suggestions for an environment, debounced 200 ms. */
export function useContextTypeSuggestions(envId: string | null): SuggestionsState<ContextTypeSuggestion> {
  const [state, setState] = useState<SuggestionsState<ContextTypeSuggestion>>({
    data: [],
    loading: false,
    error: null,
  })
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    if (!envId) {
      setState({ data: [], loading: false, error: null })
      return
    }

    setState((prev) => ({ ...prev, loading: true, error: null }))

    timerRef.current = setTimeout(() => {
      let cancelled = false
      api
        .get<ContextTypeSuggestion[]>(`/v1/environments/${envId}/context-types`)
        .then(({ data }) => {
          if (!cancelled) setState({ data, loading: false, error: null })
        })
        .catch((err) => {
          if (!cancelled)
            setState({ data: [], loading: false, error: err?.message ?? 'Failed to load' })
        })
      return () => {
        cancelled = true
      }
    }, 200)

    return () => {
      if (timerRef.current) clearTimeout(timerRef.current)
    }
  }, [envId])

  return state
}

/** Fetch context param suggestions for a context type within an environment, debounced 200 ms. */
export function useContextParamSuggestions(
  envId: string | null,
  contextType: string | null,
): SuggestionsState<ContextParamSuggestion> {
  const [state, setState] = useState<SuggestionsState<ContextParamSuggestion>>({
    data: [],
    loading: false,
    error: null,
  })
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    if (!envId || !contextType) {
      setState({ data: [], loading: false, error: null })
      return
    }

    setState((prev) => ({ ...prev, loading: true, error: null }))

    timerRef.current = setTimeout(() => {
      let cancelled = false
      api
        .get<ContextParamSuggestion[]>(
          `/v1/environments/${envId}/context-types/${contextType}/params`,
        )
        .then(({ data }) => {
          if (!cancelled) setState({ data, loading: false, error: null })
        })
        .catch((err) => {
          if (!cancelled)
            setState({ data: [], loading: false, error: err?.message ?? 'Failed to load' })
        })
      return () => {
        cancelled = true
      }
    }, 200)

    return () => {
      if (timerRef.current) clearTimeout(timerRef.current)
    }
  }, [envId, contextType])

  return state
}
