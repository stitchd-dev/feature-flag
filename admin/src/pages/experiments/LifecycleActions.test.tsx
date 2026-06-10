/**
 * LifecycleActions — presentational render tests (Phase 2). Node env: renderToString.
 */
import { describe, it, expect } from 'vitest'
import { renderToString } from 'react-dom/server'
import { LifecycleActions } from './LifecycleActions'
import type { ExperimentSummary } from '../../lib/api'

function exp(status: string): ExperimentSummary {
  return {
    id: 'e1', key: 'e1', environment_id: 'env', name: 'Exp', description: '', flag_key: 'f',
    status, model: 'frequentist', metric_ids: [], variants: 2, variant_keys: ['control', 'treatment'],
    started_at: null, ended_at: null, created_at: '2026-06-01T00:00:00Z', updated_at: '2026-06-01T00:00:00Z',
    unit_context_types: ['user'],
  }
}

const noop = () => {}

describe('LifecycleActions', () => {
  it('renders Start for a draft experiment', () => {
    const html = renderToString(<LifecycleActions envId="env" experiment={exp('draft')} canManage onUpdated={noop} onError={noop} />)
    expect(html).toMatch(/Start/)
    expect(html).not.toMatch(/Pause/)
  })

  it('renders Pause + Conclude for a running experiment', () => {
    const html = renderToString(<LifecycleActions envId="env" experiment={exp('running')} canManage onUpdated={noop} onError={noop} />)
    expect(html).toMatch(/Pause/)
    expect(html).toMatch(/Conclude/)
  })

  it('renders Resume + Conclude for a paused experiment', () => {
    const html = renderToString(<LifecycleActions envId="env" experiment={exp('paused')} canManage onUpdated={noop} onError={noop} />)
    expect(html).toMatch(/Resume/)
    expect(html).toMatch(/Conclude/)
  })

  it('renders nothing for a concluded (terminal) experiment', () => {
    const html = renderToString(<LifecycleActions envId="env" experiment={exp('concluded')} canManage onUpdated={noop} onError={noop} />)
    expect(html).toBe('')
  })

  it('renders nothing when the viewer cannot manage the org', () => {
    const html = renderToString(<LifecycleActions envId="env" experiment={exp('running')} canManage={false} onUpdated={noop} onError={noop} />)
    expect(html).toBe('')
  })
})
