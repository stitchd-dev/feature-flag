/**
 * Unit tests for Phase 7/8 experiment + flag default-rule-distribution API
 * wrappers in `admin/src/lib/api.ts`.
 *
 * Tests mock `axios.create` (called once when `api.ts` is first imported) so
 * the wrappers under test record their calls into a shared spy table without
 * touching the network. Each test asserts:
 *   • happy-path: correct method + URL + body, response data passed through.
 *   • error case: rejections propagate to callers (mirrors the gateway's
 *     axios-error pipeline already exercised by `extractErrorMessage`).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'

// ─── axios mock plumbing ─────────────────────────────────────────────────────
//
// `axios.create()` is called at module load by `api.ts` to produce the
// configured client. We override it here BEFORE importing `api.ts` so the
// client is our spy. Interceptors are no-op'd: the wrappers only use
// `.get` / `.post` / `.put` / `.patch` / `.delete`.

interface SpyClient {
  get: ReturnType<typeof vi.fn>
  post: ReturnType<typeof vi.fn>
  put: ReturnType<typeof vi.fn>
  patch: ReturnType<typeof vi.fn>
  delete: ReturnType<typeof vi.fn>
  interceptors: {
    request: { use: () => void }
    response: { use: () => void }
  }
  defaults: { baseURL: string }
}

const spyClient: SpyClient = {
  get: vi.fn(),
  post: vi.fn(),
  put: vi.fn(),
  patch: vi.fn(),
  delete: vi.fn(),
  interceptors: {
    request: { use: () => undefined },
    response: { use: () => undefined },
  },
  defaults: { baseURL: '/api' },
}

vi.mock('axios', () => ({
  default: {
    create: () => spyClient,
    post: vi.fn(),
  },
}))

// Imports must come AFTER vi.mock for the mock to take effect.
const {
  getExperimentResults,
  listExperimentExposures,
  getExperimentTimeseries,
  listExperimentIterations,
  mapIterationRow,
  triggerExperimentRecompute,
  getRecomputeStatus,
  setDefaultRuleDistribution,
  transitionExperiment,
} = await import('./api')

// ─── Test helpers ────────────────────────────────────────────────────────────

beforeEach(() => {
  spyClient.get.mockReset()
  spyClient.post.mockReset()
  spyClient.put.mockReset()
  spyClient.patch.mockReset()
  spyClient.delete.mockReset()
})

// ─── getExperimentResults ────────────────────────────────────────────────────

describe('getExperimentResults', () => {
  it('GETs the per-context-type results envelope', async () => {
    const fixture = {
      results_by_context_type: {
        user: {
          variants: [
            {
              variant_key: 'control',
              assigned_count: 100,
              mean: 0.1,
              sample_size: 100,
              p_value: null,
              ci_lower: 0.08,
              ci_upper: 0.12,
              prob_to_beat_control: null,
              expected_lift: null,
              is_winner: false,
            },
          ],
          srm: { per_variant: [], overall_chi_sq_p: 0.5, health: 'green' },
          guardrails: [],
        },
      },
      bound_target: { kind: 'rule', rule_id: 'rule-1', label: 'Pct rule' },
      pre_period_days: 0,
    }
    spyClient.get.mockResolvedValueOnce({ data: fixture })

    const result = await getExperimentResults('env-1', 'exp-key')

    expect(spyClient.get).toHaveBeenCalledTimes(1)
    expect(spyClient.get.mock.calls[0][0]).toBe(
      '/v1/environments/env-1/experiments/exp-key/results',
    )
    expect(result).toEqual(fixture)
  })

  it('propagates axios errors (e.g. 409 no-iteration-yet)', async () => {
    spyClient.get.mockRejectedValueOnce({
      response: { status: 409, data: { message: 'no completed iteration' } },
    })
    await expect(getExperimentResults('env-1', 'exp-key')).rejects.toMatchObject({
      response: { status: 409 },
    })
  })
})

// ─── listExperimentExposures ─────────────────────────────────────────────────

describe('listExperimentExposures', () => {
  it('GETs the paginated exposures URL with context_type + page params', async () => {
    spyClient.get.mockResolvedValueOnce({
      data: { items: [], total: 0, page: 1, per_page: 25 },
    })

    await listExperimentExposures('env-1', 'exp-key', 'user', {
      page: 2,
      per_page: 50,
    })

    expect(spyClient.get).toHaveBeenCalledTimes(1)
    const url = spyClient.get.mock.calls[0][0] as string
    expect(url).toContain('/v1/environments/env-1/experiments/exp-key/exposures?')
    // URLSearchParams encoding — `context_type=user&page=2&per_page=50`.
    expect(url).toMatch(/context_type=user/)
    expect(url).toMatch(/page=2/)
    expect(url).toMatch(/per_page=50/)
  })

  it('omits page params when not provided (defaults handled server-side)', async () => {
    spyClient.get.mockResolvedValueOnce({
      data: { items: [], total: 0, page: 1, per_page: 25 },
    })

    await listExperimentExposures('env-1', 'exp-key', 'account')

    const url = spyClient.get.mock.calls[0][0] as string
    expect(url).toContain('context_type=account')
    expect(url).not.toContain('page=')
    expect(url).not.toContain('per_page=')
  })

  it('propagates network errors', async () => {
    spyClient.get.mockRejectedValueOnce(new Error('Network down'))
    await expect(
      listExperimentExposures('env-1', 'exp-key', 'user'),
    ).rejects.toThrow(/Network down/)
  })
})

// ─── getExperimentTimeseries ─────────────────────────────────────────────────

describe('getExperimentTimeseries', () => {
  it('GETs the timeseries URL with metric_id + context_type + days', async () => {
    const fixture = {
      metric_id: 'metric-1',
      context_type: 'user',
      buckets: [
        { day: '2026-05-01', variant_key: 'control', value: 0.1, sample_size: 50 },
      ],
    }
    spyClient.get.mockResolvedValueOnce({ data: fixture })

    const result = await getExperimentTimeseries('env-1', 'exp-key', {
      metric_id: 'metric-1',
      context_type: 'user',
      days: 14,
    })

    expect(spyClient.get).toHaveBeenCalledTimes(1)
    const url = spyClient.get.mock.calls[0][0] as string
    expect(url).toContain('/v1/environments/env-1/experiments/exp-key/timeseries?')
    expect(url).toMatch(/metric_id=metric-1/)
    expect(url).toMatch(/context_type=user/)
    expect(url).toMatch(/days=14/)
    expect(result).toEqual(fixture)
  })

  it('propagates 404 when the metric is not part of the experiment', async () => {
    spyClient.get.mockRejectedValueOnce({
      response: { status: 404, data: { message: 'metric not attached' } },
    })
    await expect(
      getExperimentTimeseries('env-1', 'exp-key', {
        metric_id: 'bad',
        context_type: 'user',
        days: 14,
      }),
    ).rejects.toMatchObject({ response: { status: 404 } })
  })
})

// ─── triggerExperimentRecompute + getRecomputeStatus ─────────────────────────

describe('triggerExperimentRecompute', () => {
  it('POSTs to /recompute with an empty body', async () => {
    spyClient.post.mockResolvedValueOnce({
      data: { job_id: 'job-1', status: 'queued' },
    })

    const result = await triggerExperimentRecompute('env-1', 'exp-key')

    expect(spyClient.post).toHaveBeenCalledTimes(1)
    expect(spyClient.post.mock.calls[0][0]).toBe(
      '/v1/environments/env-1/experiments/exp-key/recompute',
    )
    expect(spyClient.post.mock.calls[0][1]).toEqual({})
    expect(result).toEqual({ job_id: 'job-1', status: 'queued' })
  })

  it('propagates 409 when a recompute is already in flight', async () => {
    spyClient.post.mockRejectedValueOnce({
      response: { status: 409, data: { message: 'recompute already running' } },
    })
    await expect(triggerExperimentRecompute('env-1', 'exp-key')).rejects.toMatchObject({
      response: { status: 409 },
    })
  })
})

describe('getRecomputeStatus', () => {
  it('GETs the recompute job status URL with the job id', async () => {
    spyClient.get.mockResolvedValueOnce({
      data: { job_id: 'job-1', status: 'succeeded', finished_at: '2026-05-22T00:00:00Z' },
    })

    const result = await getRecomputeStatus('env-1', 'exp-key', 'job-1')

    expect(spyClient.get).toHaveBeenCalledTimes(1)
    expect(spyClient.get.mock.calls[0][0]).toBe(
      '/v1/environments/env-1/experiments/exp-key/recompute/job-1',
    )
    expect(result.status).toBe('succeeded')
  })

  it('propagates 404 for an unknown job id', async () => {
    spyClient.get.mockRejectedValueOnce({ response: { status: 404 } })
    await expect(
      getRecomputeStatus('env-1', 'exp-key', 'job-unknown'),
    ).rejects.toMatchObject({ response: { status: 404 } })
  })
})

// ─── setDefaultRuleDistribution ──────────────────────────────────────────────

describe('setDefaultRuleDistribution', () => {
  it('POSTs the distribution wrapped in `{ distribution }`', async () => {
    spyClient.post.mockResolvedValueOnce({ data: undefined })

    const dist = {
      allocations: [
        { variant_key: 'on', percentage: 50 },
        { variant_key: 'off', percentage: 50 },
      ],
    }
    await setDefaultRuleDistribution('proj-1', 'my-flag', dist)

    expect(spyClient.post).toHaveBeenCalledTimes(1)
    expect(spyClient.post.mock.calls[0][0]).toBe(
      '/v1/projects/proj-1/flags/my-flag/default-rule-distribution',
    )
    expect(spyClient.post.mock.calls[0][1]).toEqual({ distribution: dist })
  })

  it('passes `null` distribution to clear the fallthrough', async () => {
    spyClient.post.mockResolvedValueOnce({ data: undefined })

    await setDefaultRuleDistribution('proj-1', 'my-flag', null)

    expect(spyClient.post.mock.calls[0][1]).toEqual({ distribution: null })
  })

  it('propagates 409 FLAG_LOCKED_BY_EXPERIMENT', async () => {
    spyClient.post.mockRejectedValueOnce({
      response: {
        status: 409,
        data: { message: 'flag locked by experiment', experiment_id: 'exp-1' },
      },
    })
    await expect(
      setDefaultRuleDistribution('proj-1', 'my-flag', null),
    ).rejects.toMatchObject({ response: { status: 409 } })
  })

  it('propagates 422 on invalid distribution', async () => {
    spyClient.post.mockRejectedValueOnce({
      response: { status: 422, data: { message: 'percentages must sum to 100' } },
    })
    await expect(
      setDefaultRuleDistribution('proj-1', 'my-flag', {
        allocations: [{ variant_key: 'on', percentage: 60 }],
      }),
    ).rejects.toMatchObject({ response: { status: 422 } })
  })
})

// ─── listExperimentIterations + mapIterationRow ─────────────────────────────

describe('mapIterationRow', () => {
  it('maps the proto-millis row shape to the admin IterationSummary shape', () => {
    const row = {
      id: 'iter-1',
      experiment_id: 'exp-1',
      iteration_number: 3,
      started_at_ms: Date.UTC(2026, 3, 10, 8, 0, 0),
      ended_at_ms: Date.UTC(2026, 3, 24, 8, 0, 0),
      traffic_allocation: 1.0,
      unit_context_types: ['user', 'account'],
    }
    const summary = mapIterationRow(row)
    expect(summary.iteration_id).toBe('iter-1')
    expect(summary.iteration_number).toBe(3)
    expect(summary.started_at).toBe('2026-04-10T08:00:00.000Z')
    expect(summary.ended_at).toBe('2026-04-24T08:00:00.000Z')
    expect(summary.unit_context_types).toEqual(['user', 'account'])
    // `computed_at` is reserved for a future migration; null today.
    expect(summary.computed_at).toBeNull()
  })

  it('treats `ended_at_ms == 0` as still-running (ended_at = null)', () => {
    const row = {
      id: 'iter-2',
      experiment_id: 'exp-1',
      iteration_number: 4,
      started_at_ms: Date.UTC(2026, 3, 25, 8, 0, 0),
      ended_at_ms: 0,
      traffic_allocation: 1.0,
      unit_context_types: ['user'],
    }
    const summary = mapIterationRow(row)
    expect(summary.ended_at).toBeNull()
  })
})

describe('listExperimentIterations', () => {
  it('GETs the iterations endpoint and maps rows to the admin summary shape', async () => {
    const fixture = {
      items: [
        {
          id: 'iter-1',
          experiment_id: 'exp-1',
          iteration_number: 1,
          started_at_ms: Date.UTC(2026, 3, 10, 8, 0, 0),
          ended_at_ms: Date.UTC(2026, 3, 24, 8, 0, 0),
          traffic_allocation: 1.0,
          unit_context_types: ['user'],
        },
      ],
      total: 1,
      page: 1,
      per_page: 50,
    }
    spyClient.get.mockResolvedValueOnce({ data: fixture })

    const result = await listExperimentIterations('env-1', 'exp-1')

    expect(spyClient.get).toHaveBeenCalledTimes(1)
    expect(spyClient.get.mock.calls[0][0]).toBe(
      '/v1/environments/env-1/experiments/exp-1/iterations',
    )
    expect(result.items).toHaveLength(1)
    expect(result.items[0].iteration_id).toBe('iter-1')
    expect(result.items[0].started_at).toBe('2026-04-10T08:00:00.000Z')
    expect(result.total).toBe(1)
  })

  it('appends pagination query params when provided', async () => {
    spyClient.get.mockResolvedValueOnce({
      data: { items: [], total: 0, page: 2, per_page: 25 },
    })
    await listExperimentIterations('env-1', 'exp-1', { page: 2, per_page: 25 })
    expect(spyClient.get.mock.calls[0][0]).toBe(
      '/v1/environments/env-1/experiments/exp-1/iterations?page=2&per_page=25',
    )
  })

  it('propagates 502 errors from the gateway', async () => {
    spyClient.get.mockRejectedValueOnce({
      response: { status: 502, data: { message: 'upstream unavailable' } },
    })
    await expect(
      listExperimentIterations('env-1', 'exp-1'),
    ).rejects.toMatchObject({ response: { status: 502 } })
  })
})

describe('transitionExperiment', () => {
  it('POSTs { new_status, reason } to the transitions URL and returns the experiment', async () => {
    spyClient.post.mockResolvedValueOnce({ data: { id: 'exp-1', key: 'exp-1', status: 'running' } })
    const out = await transitionExperiment('env-1', 'exp-1', 'active', 'kickoff')
    expect(spyClient.post.mock.calls[0][0]).toBe('/v1/environments/env-1/experiments/exp-1/transitions')
    expect(spyClient.post.mock.calls[0][1]).toEqual({ new_status: 'active', reason: 'kickoff' })
    expect(out.status).toBe('running')
  })

  it('omits reason when not provided', async () => {
    spyClient.post.mockResolvedValueOnce({ data: { id: 'exp-1', key: 'exp-1', status: 'paused' } })
    await transitionExperiment('env-1', 'exp-1', 'paused')
    expect(spyClient.post.mock.calls[0][1]).toEqual({ new_status: 'paused' })
  })

  it('propagates gateway errors', async () => {
    spyClient.post.mockRejectedValueOnce({ response: { status: 409, data: { message: 'invalid transition' } } })
    await expect(transitionExperiment('env-1', 'exp-1', 'concluded')).rejects.toMatchObject({ response: { status: 409 } })
  })
})
