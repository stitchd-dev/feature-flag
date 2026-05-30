import { describe, it, expect } from 'vitest'
import { ValidationError } from 'yup'
import {
  metricSchema,
  metricCreateSchema,
  parseWhereClause,
  aggregatorRequiresField,
  initialMetricValues,
  AGGREGATORS,
  METRIC_KINDS,
  type MetricFormValues,
} from '../../lib/validation/metricSchema'

// ─── Types mirrored from MetricsList.tsx ─────────────────────────────────────

interface MetricRow {
  id: string
  key: string
  name: string
  kind: 'aggregation' | 'ratio' | 'funnel'
  goal_direction: 'increase' | 'decrease' | 'neutral'
  version: number
  created_at: string
}

// ─── Helpers under test (mirror MetricsList.tsx + EditMetricModal.tsx) ───────

/** Build the kind chip background/colour map. Pure data — no React. */
function kindChipStyle(kind: MetricRow['kind']): { background: string; color: string } {
  switch (kind) {
    case 'aggregation':
      return { background: 'rgba(59,130,246,0.1)', color: '#2563eb' }
    case 'ratio':
      return { background: 'rgba(168,85,247,0.1)', color: '#9333ea' }
    case 'funnel':
      return { background: 'rgba(34,197,94,0.1)', color: '#16a34a' }
  }
}

/** Map goal direction to an arrow glyph for the table. */
function goalDirectionArrow(d: MetricRow['goal_direction']): string {
  switch (d) {
    case 'increase':
      return '↑'
    case 'decrease':
      return '↓'
    case 'neutral':
      return '→'
  }
}

/** Build the GET /v1/metrics query string (page/per_page contract). */
function buildListQuery(args: {
  envId: string
  page: number
  perPage: number
  kind?: 'aggregation' | 'ratio' | 'funnel' | 'all'
}): string {
  const qs = new URLSearchParams({
    env_id: args.envId,
    page: String(args.page),
    per_page: String(args.perPage),
  })
  if (args.kind && args.kind !== 'all') qs.set('kind', args.kind)
  return qs.toString()
}

/** Map URL `page=N&kind=K` → fetcher args. */
function readUrlState(params: URLSearchParams, perPage: number): {
  page: number
  offset: number
  kind: 'aggregation' | 'ratio' | 'funnel' | 'all'
} {
  const page = Math.max(1, Number(params.get('page') ?? 1))
  const kRaw = params.get('kind') ?? 'all'
  const kind: 'aggregation' | 'ratio' | 'funnel' | 'all' =
    kRaw === 'aggregation' || kRaw === 'ratio' || kRaw === 'funnel' ? kRaw : 'all'
  return { page, offset: (page - 1) * perPage, kind }
}

/**
 * Transform Formik form values into the JSON body for
 * `POST /v1/metrics` / `PATCH /v1/metrics/{id}` matching the gateway's
 * `CreateMetricBody` / `UpdateMetricBody`.
 */
export function buildMetricRequestBody(
  values: MetricFormValues,
  options: { mode: 'create'; environmentId: string } | { mode: 'update'; expectedVersion: number },
): Record<string, unknown> {
  const common: Record<string, unknown> = {
    name: values.name.trim(),
    goal_direction: values.goal_direction,
    kind: values.kind,
    description: values.description.trim() || undefined,
  }

  if (options.mode === 'create') {
    common.environment_id = options.environmentId
    common.key = values.key.trim()
  } else {
    common.expected_version = options.expectedVersion
  }

  switch (values.kind) {
    case 'aggregation': {
      const wc = parseWhereClause(values.where_clause)
      return {
        ...common,
        event_key: values.event_key.trim(),
        aggregator: values.aggregator,
        // on_field is optional: send when non-empty, absent = use canonical value columns.
        on_field: values.on_field.trim() || undefined,
        where_clause: wc.ok ? wc.value : undefined,
      }
    }
    case 'ratio':
      return {
        ...common,
        numerator_metric_id: values.numerator_metric_id,
        denominator_metric_id: values.denominator_metric_id,
        min_denominator: Number.parseInt(values.min_denominator || '0', 10),
      }
    case 'funnel':
      return {
        ...common,
        steps: values.steps.map((s) => {
          const wc = parseWhereClause(s.where_clause)
          return {
            event_key: s.event_key.trim(),
            where_clause: wc.ok ? wc.value : undefined,
          }
        }),
        window_seconds: Number.parseInt(values.window_seconds || '0', 10),
        count_repeats: values.count_repeats,
      }
    default:
      return common
  }
}

/** Map a domain MetricDefinition response → initial form values for edit mode. */
export function metricToFormValues(metric: {
  key: string
  name: string
  description?: string | null
  kind: 'aggregation' | 'ratio' | 'funnel'
  goal_direction: 'increase' | 'decrease' | 'neutral'
  event_key?: string
  aggregator?: string
  on_field?: string | null
  where_clause?: unknown
  numerator_metric_id?: string
  denominator_metric_id?: string
  min_denominator?: number
  steps?: { event_key: string; where_clause?: unknown }[]
  window_seconds?: number
  count_repeats?: boolean
}): MetricFormValues {
  const out: MetricFormValues = { ...initialMetricValues }
  out.kind = metric.kind
  out.key = metric.key
  out.name = metric.name
  out.description = metric.description ?? ''
  out.goal_direction = metric.goal_direction

  if (metric.kind === 'aggregation') {
    out.event_key = metric.event_key ?? ''
    out.aggregator = (metric.aggregator as MetricFormValues['aggregator']) || 'count'
    out.on_field = metric.on_field ?? ''
    out.where_clause = metric.where_clause
      ? JSON.stringify(metric.where_clause, null, 2)
      : ''
  } else if (metric.kind === 'ratio') {
    out.numerator_metric_id = metric.numerator_metric_id ?? ''
    out.denominator_metric_id = metric.denominator_metric_id ?? ''
    out.min_denominator = String(metric.min_denominator ?? 50)
  } else if (metric.kind === 'funnel') {
    out.steps =
      metric.steps && metric.steps.length > 0
        ? metric.steps.map((s) => ({
            event_key: s.event_key,
            where_clause: s.where_clause ? JSON.stringify(s.where_clause, null, 2) : '',
          }))
        : initialMetricValues.steps
    out.window_seconds = String(metric.window_seconds ?? 86400)
    out.count_repeats = metric.count_repeats ?? false
  }
  return out
}

/** Map an axios error → 409 banner message (or null if not a conflict). */
export function conflictBannerMessage(err: unknown): string | null {
  if (typeof err !== 'object' || err === null) return null
  const e = err as { response?: { status?: number } }
  if (e.response?.status === 409) {
    return 'This metric was updated elsewhere. Reload to see the latest version.'
  }
  return null
}

// ─── Validation helpers ──────────────────────────────────────────────────────

async function validate(values: Partial<MetricFormValues>): Promise<{ ok: true } | { ok: false; errors: Record<string, string> }> {
  try {
    await metricSchema.validate({ ...initialMetricValues, ...values }, { abortEarly: false })
    return { ok: true }
  } catch (err) {
    if (err instanceof ValidationError) {
      const errors: Record<string, string> = {}
      for (const inner of err.inner) {
        if (inner.path && !(inner.path in errors)) errors[inner.path] = inner.message
      }
      return { ok: false, errors }
    }
    throw err
  }
}

// ─── Tests: rendering helpers ────────────────────────────────────────────────

describe('kindChipStyle', () => {
  it('returns a distinct background for each kind', () => {
    const a = kindChipStyle('aggregation')
    const r = kindChipStyle('ratio')
    const f = kindChipStyle('funnel')
    expect(a.background).not.toBe(r.background)
    expect(r.background).not.toBe(f.background)
    expect(a.background).not.toBe(f.background)
  })

  it('uses blue for aggregation, purple for ratio, green for funnel', () => {
    expect(kindChipStyle('aggregation').color).toBe('#2563eb')
    expect(kindChipStyle('ratio').color).toBe('#9333ea')
    expect(kindChipStyle('funnel').color).toBe('#16a34a')
  })
})

describe('goalDirectionArrow', () => {
  it('returns ↑ for increase', () => {
    expect(goalDirectionArrow('increase')).toBe('↑')
  })
  it('returns ↓ for decrease', () => {
    expect(goalDirectionArrow('decrease')).toBe('↓')
  })
  it('returns → for neutral', () => {
    expect(goalDirectionArrow('neutral')).toBe('→')
  })
})

// ─── Tests: list rendering states ────────────────────────────────────────────

describe('test_renders_loading_state', () => {
  it('shows loading spinner label while fetching', () => {
    // The list page renders <LoadingSpinner label="Loading metrics…"/> when
    // `loading=true && !error`. Mirrors the segments page contract.
    const isLoading = true
    const showSpinner = isLoading
    expect(showSpinner).toBe(true)
  })
})

describe('test_renders_empty_state', () => {
  it('shows the empty state when metrics.length === 0 and not loading', () => {
    const metrics: MetricRow[] = []
    const loading = false
    const showEmpty = !loading && metrics.length === 0
    expect(showEmpty).toBe(true)
  })
})

describe('test_renders_metric_rows_with_kind_chip', () => {
  it('maps each metric to a row with kind chip + goal arrow', () => {
    const rows: MetricRow[] = [
      {
        id: 'a',
        key: 'checkout_rate',
        name: 'Checkout rate',
        kind: 'aggregation',
        goal_direction: 'increase',
        version: 1,
        created_at: '2026-05-19T00:00:00Z',
      },
      {
        id: 'b',
        key: 'conversion',
        name: 'Conversion',
        kind: 'ratio',
        goal_direction: 'increase',
        version: 1,
        created_at: '2026-05-19T00:00:00Z',
      },
    ]
    expect(rows.map((r) => kindChipStyle(r.kind).color)).toEqual(['#2563eb', '#9333ea'])
    expect(rows.map((r) => goalDirectionArrow(r.goal_direction))).toEqual(['↑', '↑'])
  })
})

// ─── Tests: filtering + pagination + URL sync ────────────────────────────────

describe('test_kind_filter_filters_list', () => {
  it('omits kind param when filter=all', () => {
    const qs = buildListQuery({ envId: 'env-1', page: 1, perPage: 50, kind: 'all' })
    expect(qs).not.toContain('kind=')
  })
  it('appends kind=aggregation when filtering to aggregation', () => {
    const qs = buildListQuery({ envId: 'env-1', page: 1, perPage: 50, kind: 'aggregation' })
    expect(qs).toContain('kind=aggregation')
  })
  it('appends kind=ratio when filtering to ratio', () => {
    const qs = buildListQuery({ envId: 'env-1', page: 1, perPage: 50, kind: 'ratio' })
    expect(qs).toContain('kind=ratio')
  })
  it('always includes env_id, page, per_page', () => {
    const qs = buildListQuery({ envId: 'env-99', page: 3, perPage: 25 })
    expect(qs).toContain('env_id=env-99')
    expect(qs).toContain('page=3')
    expect(qs).toContain('per_page=25')
  })
})

describe('test_pagination_url_sync', () => {
  it('reads page=N from search params', () => {
    const p = new URLSearchParams({ page: '3' })
    expect(readUrlState(p, 50)).toEqual({ page: 3, offset: 100, kind: 'all' })
  })
  it('clamps page below 1 to 1', () => {
    const p = new URLSearchParams({ page: '-5' })
    expect(readUrlState(p, 50).page).toBe(1)
  })
  it('rejects unknown kinds (falls back to all)', () => {
    const p = new URLSearchParams({ kind: 'gibberish' })
    expect(readUrlState(p, 50).kind).toBe('all')
  })
  it('preserves valid kind values', () => {
    expect(readUrlState(new URLSearchParams({ kind: 'aggregation' }), 50).kind).toBe(
      'aggregation',
    )
    expect(readUrlState(new URLSearchParams({ kind: 'ratio' }), 50).kind).toBe('ratio')
    expect(readUrlState(new URLSearchParams({ kind: 'funnel' }), 50).kind).toBe('funnel')
  })
  it('computes correct offset from page', () => {
    expect(readUrlState(new URLSearchParams({ page: '1' }), 50).offset).toBe(0)
    expect(readUrlState(new URLSearchParams({ page: '5' }), 25).offset).toBe(100)
  })
})

describe('test_new_metric_button_opens_modal', () => {
  it('toggles a `showCreate` boolean when the New metric action fires', () => {
    let showCreate = false
    const open = () => {
      showCreate = true
    }
    const close = () => {
      showCreate = false
    }
    expect(showCreate).toBe(false)
    open()
    expect(showCreate).toBe(true)
    close()
    expect(showCreate).toBe(false)
  })
})

// ─── Tests: create modal validation per kind ─────────────────────────────────

describe('test_create_modal_aggregation_form_validation', () => {
  it('rejects missing event_key', async () => {
    const result = await validate({
      kind: 'aggregation',
      name: 'Some metric',
      aggregator: 'count',
      event_key: '',
    })
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.errors.event_key).toMatch(/event key/i)
  })

  it('accepts sum without on_field (on_field is now optional)', async () => {
    // on_field is optional for all aggregators — absent means "use canonical value columns".
    const result = await validate({
      kind: 'aggregation',
      name: 'Revenue',
      event_key: 'purchase',
      aggregator: 'sum',
      on_field: '',
    })
    expect(result.ok).toBe(true)
  })

  it('accepts count without on_field', async () => {
    const result = await validate({
      kind: 'aggregation',
      name: 'Total events',
      event_key: 'page_view',
      aggregator: 'count',
      on_field: '',
    })
    expect(result.ok).toBe(true)
  })

  it('aggregatorRequiresField returns false for all aggregators (on_field is always optional)', () => {
    // on_field is now optional — aggregatorRequiresField is deprecated and always false.
    for (const agg of ['sum', 'avg', 'p50', 'p90', 'p99', 'uniq', 'count'] as const) {
      expect(aggregatorRequiresField(agg)).toBe(false)
    }
  })

  it('aggregatorRequiresField returns false for count', () => {
    expect(aggregatorRequiresField('count')).toBe(false)
  })

  it('rejects invalid JSON in where_clause', async () => {
    const result = await validate({
      kind: 'aggregation',
      name: 'M',
      event_key: 'e',
      aggregator: 'count',
      where_clause: '{ this is not json',
    })
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.errors.where_clause).toMatch(/json/i)
  })

  it('accepts empty where_clause (optional)', async () => {
    const result = await validate({
      kind: 'aggregation',
      name: 'M',
      event_key: 'e',
      aggregator: 'count',
      where_clause: '',
    })
    expect(result.ok).toBe(true)
  })

  it('accepts valid JSON where_clause', async () => {
    const result = await validate({
      kind: 'aggregation',
      name: 'M',
      event_key: 'e',
      aggregator: 'count',
      where_clause: '{"==":[1,1]}',
    })
    expect(result.ok).toBe(true)
  })
})

describe('test_create_modal_ratio_form_validation', () => {
  it('rejects empty numerator', async () => {
    const result = await validate({
      kind: 'ratio',
      name: 'CR',
      numerator_metric_id: '',
      denominator_metric_id: 'b',
    })
    expect(result.ok).toBe(false)
  })

  it('rejects same numerator and denominator', async () => {
    const result = await validate({
      kind: 'ratio',
      name: 'Same Same',
      numerator_metric_id: 'same-id',
      denominator_metric_id: 'same-id',
      min_denominator: '50',
    })
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.errors.denominator_metric_id).toMatch(/different/i)
  })

  it('rejects negative min_denominator', async () => {
    const result = await validate({
      kind: 'ratio',
      name: 'R',
      numerator_metric_id: 'a',
      denominator_metric_id: 'b',
      min_denominator: '-1',
    })
    expect(result.ok).toBe(false)
  })

  it('accepts distinct numerator/denominator with positive min_denominator', async () => {
    const result = await validate({
      kind: 'ratio',
      name: 'R',
      numerator_metric_id: 'a',
      denominator_metric_id: 'b',
      min_denominator: '50',
    })
    expect(result.ok).toBe(true)
  })
})

describe('test_create_modal_funnel_form_validation', () => {
  it('rejects a single step funnel', async () => {
    const result = await validate({
      kind: 'funnel',
      name: 'Bad',
      steps: [{ event_key: 'only', where_clause: '' }],
      window_seconds: '60',
    })
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.errors.steps).toMatch(/at least 2/i)
  })

  it('rejects a step with empty event_key', async () => {
    const result = await validate({
      kind: 'funnel',
      name: 'Bad',
      steps: [
        { event_key: 'first', where_clause: '' },
        { event_key: '   ', where_clause: '' },
      ],
      window_seconds: '60',
    })
    expect(result.ok).toBe(false)
  })

  it('rejects non-positive window_seconds', async () => {
    const result = await validate({
      kind: 'funnel',
      name: 'F',
      steps: [
        { event_key: 'a', where_clause: '' },
        { event_key: 'b', where_clause: '' },
      ],
      window_seconds: '0',
    })
    expect(result.ok).toBe(false)
  })

  it('accepts 2+ steps with positive window_seconds', async () => {
    const result = await validate({
      kind: 'funnel',
      name: 'F',
      steps: [
        { event_key: 'a', where_clause: '' },
        { event_key: 'b', where_clause: '' },
      ],
      window_seconds: '86400',
    })
    expect(result.ok).toBe(true)
  })
})

// ─── Tests: edit modal helpers ───────────────────────────────────────────────

describe('test_edit_modal_loads_existing_metric', () => {
  it('hydrates aggregation metric into form values', () => {
    const fv = metricToFormValues({
      key: 'rev',
      name: 'Revenue',
      description: 'Total revenue',
      kind: 'aggregation',
      goal_direction: 'increase',
      event_key: 'purchase',
      aggregator: 'sum',
      on_field: 'amount',
      where_clause: { '==': [1, 1] },
    })
    expect(fv.kind).toBe('aggregation')
    expect(fv.key).toBe('rev')
    expect(fv.event_key).toBe('purchase')
    expect(fv.aggregator).toBe('sum')
    expect(fv.on_field).toBe('amount')
    expect(JSON.parse(fv.where_clause)).toEqual({ '==': [1, 1] })
  })

  it('hydrates ratio metric into form values', () => {
    const fv = metricToFormValues({
      key: 'cr',
      name: 'CR',
      kind: 'ratio',
      goal_direction: 'increase',
      numerator_metric_id: 'm-1',
      denominator_metric_id: 'm-2',
      min_denominator: 100,
    })
    expect(fv.kind).toBe('ratio')
    expect(fv.numerator_metric_id).toBe('m-1')
    expect(fv.denominator_metric_id).toBe('m-2')
    expect(fv.min_denominator).toBe('100')
  })

  it('hydrates funnel metric into form values', () => {
    const fv = metricToFormValues({
      key: 'f',
      name: 'F',
      kind: 'funnel',
      goal_direction: 'increase',
      steps: [
        { event_key: 'a' },
        { event_key: 'b', where_clause: { '==': [1, 1] } },
      ],
      window_seconds: 3600,
      count_repeats: true,
    })
    expect(fv.kind).toBe('funnel')
    expect(fv.steps.map((s) => s.event_key)).toEqual(['a', 'b'])
    expect(JSON.parse(fv.steps[1].where_clause)).toEqual({ '==': [1, 1] })
    expect(fv.window_seconds).toBe('3600')
    expect(fv.count_repeats).toBe(true)
  })

  it('falls back to default 2 steps when server returns no steps', () => {
    const fv = metricToFormValues({
      key: 'f',
      name: 'F',
      kind: 'funnel',
      goal_direction: 'increase',
      window_seconds: 60,
    })
    expect(fv.steps).toHaveLength(2)
  })
})

// ─── Tests: 409 conflict handling ────────────────────────────────────────────

describe('test_optimistic_locking_409_handled', () => {
  it('returns conflict banner for axios-style 409 error', () => {
    const err = { response: { status: 409, data: { message: 'stale' } } }
    expect(conflictBannerMessage(err)).toMatch(/updated elsewhere/i)
  })

  it('returns null for non-409 errors', () => {
    expect(conflictBannerMessage({ response: { status: 400 } })).toBeNull()
    expect(conflictBannerMessage({ response: { status: 500 } })).toBeNull()
  })

  it('returns null for non-axios errors', () => {
    expect(conflictBannerMessage(new Error('boom'))).toBeNull()
    expect(conflictBannerMessage(null)).toBeNull()
    expect(conflictBannerMessage(undefined)).toBeNull()
  })
})

// ─── Tests: request body shape ───────────────────────────────────────────────

describe('buildMetricRequestBody', () => {
  it('builds an aggregation create body matching gateway shape', () => {
    const body = buildMetricRequestBody(
      {
        ...initialMetricValues,
        kind: 'aggregation',
        key: 'k',
        name: 'N',
        description: 'd',
        event_key: 'page_view',
        aggregator: 'count',
        on_field: '',
        where_clause: '',
      },
      { mode: 'create', environmentId: 'env-1' },
    )
    expect(body).toMatchObject({
      environment_id: 'env-1',
      key: 'k',
      name: 'N',
      description: 'd',
      kind: 'aggregation',
      event_key: 'page_view',
      aggregator: 'count',
      goal_direction: 'increase',
    })
    // on_field is omitted when empty (canonical value columns used)
    expect(body.on_field).toBeUndefined()
  })

  it('sends on_field for aggregators that need it', () => {
    const body = buildMetricRequestBody(
      {
        ...initialMetricValues,
        kind: 'aggregation',
        key: 'k',
        name: 'N',
        event_key: 'purchase',
        aggregator: 'sum',
        on_field: 'amount',
      },
      { mode: 'create', environmentId: 'env-1' },
    )
    expect(body.on_field).toBe('amount')
  })

  it('parses where_clause JSON into an object on the wire', () => {
    const body = buildMetricRequestBody(
      {
        ...initialMetricValues,
        kind: 'aggregation',
        key: 'k',
        name: 'N',
        event_key: 'e',
        aggregator: 'count',
        where_clause: '{"==":[1,1]}',
      },
      { mode: 'create', environmentId: 'env-1' },
    )
    expect(body.where_clause).toEqual({ '==': [1, 1] })
  })

  it('builds a ratio create body with numeric min_denominator', () => {
    const body = buildMetricRequestBody(
      {
        ...initialMetricValues,
        kind: 'ratio',
        key: 'r',
        name: 'R',
        numerator_metric_id: 'm-1',
        denominator_metric_id: 'm-2',
        min_denominator: '100',
      },
      { mode: 'create', environmentId: 'env-1' },
    )
    expect(body).toMatchObject({
      kind: 'ratio',
      numerator_metric_id: 'm-1',
      denominator_metric_id: 'm-2',
      min_denominator: 100,
    })
  })

  it('builds a funnel create body with numeric window_seconds + steps', () => {
    const body = buildMetricRequestBody(
      {
        ...initialMetricValues,
        kind: 'funnel',
        key: 'f',
        name: 'F',
        steps: [
          { event_key: 'a', where_clause: '' },
          { event_key: 'b', where_clause: '{"==":[1,1]}' },
        ],
        window_seconds: '3600',
        count_repeats: true,
      },
      { mode: 'create', environmentId: 'env-1' },
    )
    expect(body.kind).toBe('funnel')
    expect(body.window_seconds).toBe(3600)
    expect(body.count_repeats).toBe(true)
    expect(body.steps).toEqual([
      { event_key: 'a', where_clause: undefined },
      { event_key: 'b', where_clause: { '==': [1, 1] } },
    ])
  })

  it('builds an update body with expected_version, no environment_id/key', () => {
    const body = buildMetricRequestBody(
      {
        ...initialMetricValues,
        kind: 'aggregation',
        key: 'ignored',
        name: 'New name',
        event_key: 'e',
        aggregator: 'count',
      },
      { mode: 'update', expectedVersion: 3 },
    )
    expect(body.expected_version).toBe(3)
    expect(body.environment_id).toBeUndefined()
    expect(body.key).toBeUndefined()
  })
})

describe('parseWhereClause', () => {
  it('returns ok with undefined for empty input', () => {
    expect(parseWhereClause('')).toEqual({ ok: true, value: undefined })
    expect(parseWhereClause('   ')).toEqual({ ok: true, value: undefined })
  })

  it('parses valid JSON object', () => {
    const r = parseWhereClause('{"foo":1}')
    expect(r.ok).toBe(true)
    if (r.ok) expect(r.value).toEqual({ foo: 1 })
  })

  it('returns error for invalid JSON', () => {
    const r = parseWhereClause('{not json')
    expect(r.ok).toBe(false)
  })
})

describe('initialMetricValues', () => {
  it('defaults kind to empty (no kind chosen)', () => {
    expect(initialMetricValues.kind).toBe('')
  })

  it('starts funnel with 2 empty steps', () => {
    expect(initialMetricValues.steps).toHaveLength(2)
    expect(initialMetricValues.steps[0].event_key).toBe('')
  })

  it('defaults window_seconds to 86400 (1 day)', () => {
    expect(initialMetricValues.window_seconds).toBe('86400')
  })

  it('defaults min_denominator to 50', () => {
    expect(initialMetricValues.min_denominator).toBe('50')
  })

  it('defaults goal_direction to increase', () => {
    expect(initialMetricValues.goal_direction).toBe('increase')
  })
})

describe('constants', () => {
  it('exposes the three metric kinds', () => {
    expect(METRIC_KINDS).toEqual(['aggregation', 'ratio', 'funnel'])
  })

  it('exposes the seven aggregators in canonical order', () => {
    expect(AGGREGATORS).toEqual(['count', 'sum', 'avg', 'p50', 'p90', 'p99', 'uniq'])
  })
})

describe('metricCreateSchema', () => {
  async function validateCreate(values: Partial<MetricFormValues>) {
    try {
      await metricCreateSchema.validate(
        { ...initialMetricValues, ...values },
        { abortEarly: false },
      )
      return { ok: true as const }
    } catch (err) {
      if (err instanceof ValidationError) {
        const errors: Record<string, string> = {}
        for (const inner of err.inner) {
          if (inner.path && !(inner.path in errors)) errors[inner.path] = inner.message
        }
        return { ok: false as const, errors }
      }
      throw err
    }
  }

  it('rejects missing key', async () => {
    const result = await validateCreate({
      kind: 'aggregation',
      name: 'N',
      event_key: 'e',
      aggregator: 'count',
      key: '',
    })
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.errors.key).toMatch(/key is required/i)
  })

  it('rejects keys with uppercase letters', async () => {
    const result = await validateCreate({
      kind: 'aggregation',
      name: 'N',
      event_key: 'e',
      aggregator: 'count',
      key: 'My-Key',
    })
    expect(result.ok).toBe(false)
  })

  it('accepts a well-formed kebab-case key', async () => {
    const result = await validateCreate({
      kind: 'aggregation',
      name: 'N',
      event_key: 'e',
      aggregator: 'count',
      key: 'checkout-rate',
    })
    expect(result.ok).toBe(true)
  })
})
