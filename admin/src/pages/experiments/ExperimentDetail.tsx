import { useState, useEffect } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { useTweaks } from '../../hooks/useTweaks'
import { PageHeader } from '../../components/primitives'
import { I } from '../../components/icons'
import { useOrgContext } from '../../context/OrgContext'
import {
  ContextTypeProvider,
  useActiveContextType,
} from '../../context/ContextTypeContext'
import { ContextTypeTabs } from '../../components/ContextTypeTabs'
import { getExperiment, getExperimentResults, api } from '../../lib/api'
import type { ExperimentSummary } from '../../lib/api'
import type { ExperimentResults } from '../../lib/types'
import { extractErrorMessage } from '../../lib/errors'
import {
  buildExperimentDisplay,
  listContextTypes,
  pickContextTypeResult,
} from './ExperimentDetail.helpers'
import type { ExperimentDisplay } from './ExperimentDetail.helpers'

/**
 * Minimal projection of a `metric_definitions` row used by ExperimentDetail
 * to resolve display names from the IDs referenced by an experiment.
 */
interface MetricNameRow {
  id: string
  name: string
  kind: 'aggregation' | 'ratio' | 'funnel'
}

type Tab = 'results' | 'config' | 'metrics' | 'events'

/**
 * Adapter shape consumed by the Phase 8 mock-flavoured viz subcomponents.
 *
 * Phase 9 will rewrite `BayesianViz`, `VariantBreakdown`, and
 * `MultiVariantMatrix` to read directly from `ExperimentResults`. Until then,
 * this adapter folds the live API responses through `buildExperimentDisplay`
 * into a structure the legacy viz components expect — so we drop the
 * legacy mock-data dependency without invasive viz rewrites.
 */
interface VizAdapter {
  /** Number of variants in the experiment (control + treatments). */
  variants: number
  /** Confidence as an integer percent — drives both P(beats control) and badge thresholds. */
  confidence: number
}

// ─── Frequentist visualization ───────────────────────────────────────────────

function FrequentistViz() {
  return (
    <div className="split-2">
      <div className="card">
        <div className="card-header">
          <div className="card-title">Cumulative lift over time</div>
          <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>p-value: 0.0023</span>
        </div>
        <div style={{ padding: 18 }}>
          <svg viewBox="0 0 600 220" style={{ width: '100%' }}>
            <defs>
              <linearGradient id="ciGrad" x1="0" x2="0" y1="0" y2="1">
                <stop offset="0%" stopColor="var(--accent)" stopOpacity="0.18" />
                <stop offset="100%" stopColor="var(--accent)" stopOpacity="0.04" />
              </linearGradient>
            </defs>
            {[40, 80, 120, 160].map((y) => <line key={y} x1="40" x2="590" y1={y} y2={y} stroke="var(--border-faint)" />)}
            <line x1="40" x2="590" y1="120" y2="120" stroke="var(--fg-faint)" strokeDasharray="4 4" />
            <text x="34" y="124" fontSize="10" fill="var(--fg-muted)" textAnchor="end">0%</text>
            <text x="34" y="84" fontSize="10" fill="var(--fg-muted)" textAnchor="end">+5%</text>
            <text x="34" y="44" fontSize="10" fill="var(--fg-muted)" textAnchor="end">+10%</text>
            <text x="34" y="164" fontSize="10" fill="var(--fg-muted)" textAnchor="end">−5%</text>
            <path d="M40,140 L100,135 L160,128 L220,118 L280,108 L340,100 L400,92 L460,86 L520,82 L590,78 L590,66 L520,72 L460,76 L400,82 L340,90 L280,98 L220,108 L160,118 L100,125 L40,130 Z" fill="url(#ciGrad)" stroke="var(--accent)" strokeWidth="1" strokeOpacity="0.3" strokeDasharray="3 3" />
            <path d="M40,135 L100,130 L160,123 L220,113 L280,103 L340,95 L400,87 L460,81 L520,77 L590,72" fill="none" stroke="var(--accent)" strokeWidth="2" />
            <line x1="40" x2="590" y1="105" y2="105" stroke="var(--success)" strokeDasharray="2 4" strokeWidth="1" />
            <text x="585" y="100" fontSize="9" fill="var(--success)" textAnchor="end">p &lt; 0.05 reached</text>
          </svg>
          <div style={{ display: 'flex', gap: 16, fontSize: 11, color: 'var(--fg-muted)', marginTop: 8 }}>
            <span><span style={{ display: 'inline-block', width: 10, height: 2, background: 'var(--accent)', verticalAlign: 'middle', marginRight: 4 }} />variant-a vs control</span>
            <span><span style={{ display: 'inline-block', width: 10, height: 8, background: 'var(--accent)', opacity: 0.18, verticalAlign: 'middle', marginRight: 4 }} />95% CI band</span>
          </div>
        </div>
      </div>
      <div className="card">
        <div className="card-header"><div className="card-title">Sample size & power</div></div>
        <div style={{ padding: 18 }}>
          <div style={{ marginBottom: 14 }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: 12, marginBottom: 6 }}><span>Samples collected</span><strong style={{ fontFamily: 'var(--font-mono)' }}>412,415 / 380,000</strong></div>
            <div style={{ height: 8, background: 'var(--bg-sunken)', borderRadius: 4, overflow: 'hidden' }}><div style={{ width: '100%', height: '100%', background: 'var(--success)' }} /></div>
            <div style={{ fontSize: 11, color: 'var(--success)', marginTop: 4, fontWeight: 600 }}>✓ Min sample size reached (108%)</div>
          </div>
          <div style={{ marginBottom: 14 }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: 12, marginBottom: 6 }}><span>Statistical power</span><strong style={{ fontFamily: 'var(--font-mono)' }}>0.94</strong></div>
            <div style={{ height: 8, background: 'var(--bg-sunken)', borderRadius: 4, overflow: 'hidden' }}><div style={{ width: '94%', height: '100%', background: 'var(--success)' }} /></div>
          </div>
          <div className="hr" />
          <div style={{ fontSize: 12, lineHeight: 1.6 }}>
            {[['α (significance)', '0.05'], ['β (target power)', '0.80'], ['MDE', '2.0%'], ['Test', 'two-sided z-test'], ['Multiple comp.', 'Bonferroni']].map(([k, v]) => (
              <div key={k} style={{ display: 'flex', justifyContent: 'space-between' }}>
                <span style={{ color: 'var(--fg-muted)' }}>{k}</span>
                <span className="mono-key">{v}</span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  )
}

// ─── Bayesian visualization ──────────────────────────────────────────────────

function BayesianViz({ adapter }: { adapter: VizAdapter }) {
  const isMulti = adapter.variants > 2
  return (
    <div className="split-2">
      <div className="card">
        <div className="card-header">
          <div className="card-title">Posterior distributions</div>
          <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>{isMulti ? `${adapter.variants} arms` : `P(variant > control) = ${adapter.confidence}%`}</span>
        </div>
        <div style={{ padding: 18 }}>
          <svg viewBox="0 0 600 220" style={{ width: '100%' }}>
            {[40, 80, 120, 160].map((y) => <line key={y} x1="20" x2="590" y1={y} y2={y} stroke="var(--border-faint)" />)}
            {isMulti ? (
              <>
                <path d="M30,180 Q120,180 170,90 Q220,180 310,180" fill="rgba(139,141,150,0.15)" stroke="#8B8D96" strokeWidth="2" />
                <path d="M120,180 Q210,180 260,70 Q310,180 400,180" fill="rgba(91,174,245,0.15)" stroke="#5BAEF5" strokeWidth="2" />
                <path d="M220,180 Q310,180 360,30 Q410,180 500,180" fill="rgba(61,214,140,0.18)" stroke="#3DD68C" strokeWidth="2" />
                <line x1="170" x2="170" y1="180" y2="90" stroke="#8B8D96" strokeDasharray="3 3" />
                <line x1="260" x2="260" y1="180" y2="70" stroke="#5BAEF5" strokeDasharray="3 3" />
                <line x1="360" x2="360" y1="180" y2="30" stroke="#3DD68C" strokeDasharray="3 3" />
                <text x="170" y="200" fontSize="10" fill="#8B8D96" textAnchor="middle" fontFamily="var(--font-mono)">9.84%</text>
                <text x="260" y="200" fontSize="10" fill="#5BAEF5" textAnchor="middle" fontFamily="var(--font-mono)">10.21%</text>
                <text x="360" y="200" fontSize="10" fill="#3DD68C" textAnchor="middle" fontFamily="var(--font-mono)">12.04%</text>
              </>
            ) : (
              <>
                <path d="M50,180 Q150,180 200,80 Q250,180 350,180" fill="rgba(91,174,245,0.15)" stroke="var(--info)" strokeWidth="2" />
                <path d="M180,180 Q280,180 330,40 Q380,180 480,180" fill="var(--accent-soft)" stroke="var(--accent)" strokeWidth="2" />
                <line x1="200" x2="200" y1="180" y2="80" stroke="var(--info)" strokeDasharray="3 3" />
                <line x1="330" x2="330" y1="180" y2="40" stroke="var(--accent)" strokeDasharray="3 3" />
                <text x="200" y="200" fontSize="10" fill="var(--info)" textAnchor="middle" fontFamily="var(--font-mono)">11.42%</text>
                <text x="330" y="200" fontSize="10" fill="var(--accent)" textAnchor="middle" fontFamily="var(--font-mono)">11.96%</text>
              </>
            )}
          </svg>
          <div style={{ display: 'flex', gap: 14, fontSize: 11, color: 'var(--fg-muted)', marginTop: 8, flexWrap: 'wrap' }}>
            {isMulti ? (
              <>
                <span><span style={{ display: 'inline-block', width: 10, height: 2, background: '#8B8D96', verticalAlign: 'middle', marginRight: 4 }} />control</span>
                <span><span style={{ display: 'inline-block', width: 10, height: 2, background: '#5BAEF5', verticalAlign: 'middle', marginRight: 4 }} />variant-a</span>
                <span><span style={{ display: 'inline-block', width: 10, height: 2, background: '#3DD68C', verticalAlign: 'middle', marginRight: 4 }} />variant-b</span>
              </>
            ) : (
              <>
                <span><span style={{ display: 'inline-block', width: 10, height: 2, background: 'var(--info)', verticalAlign: 'middle', marginRight: 4 }} />control</span>
                <span><span style={{ display: 'inline-block', width: 10, height: 2, background: 'var(--accent)', verticalAlign: 'middle', marginRight: 4 }} />variant-a</span>
              </>
            )}
            <span style={{ marginLeft: 'auto' }}>Beta(α,β) prior · 5,000 MCMC draws</span>
          </div>
        </div>
      </div>
      <div className="card">
        <div className="card-header"><div className="card-title">Decision metrics</div></div>
        <div style={{ padding: 18 }}>
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 14, marginBottom: 14 }}>
            <div style={{ padding: 12, background: 'var(--success-bg)', borderRadius: 8, border: '1px solid rgba(17,122,61,0.25)' }}>
              <div style={{ fontSize: 10, color: 'var(--success)', textTransform: 'uppercase', letterSpacing: '0.05em', fontWeight: 600 }}>{isMulti ? 'P(variant-b is best)' : 'P(beats control)'}</div>
              <div style={{ fontFamily: 'var(--font-display)', fontSize: 28, fontWeight: 800, color: 'var(--success)' }}>{isMulti ? '85%' : `${adapter.confidence}%`}</div>
            </div>
            <div style={{ padding: 12, background: 'var(--bg-sunken)', borderRadius: 8 }}>
              <div style={{ fontSize: 10, color: 'var(--fg-muted)', textTransform: 'uppercase', letterSpacing: '0.05em', fontWeight: 600 }}>Expected loss</div>
              <div style={{ fontFamily: 'var(--font-display)', fontSize: 28, fontWeight: 800 }}>{isMulti ? '0.06%' : '0.04%'}</div>
            </div>
          </div>
          <div className="hr" />
          <div style={{ fontSize: 12, lineHeight: 1.7 }}>
            {[
              ['Posterior mean lift' + (isMulti ? ' (b vs ctrl)' : ''), isMulti ? '+22.4%' : '+4.7%'],
              ['95% credible interval', isMulti ? '[+1.4%, +6.8%]' : '[+1.2%, +8.3%]'],
              ['Risk threshold', '0.5%'],
              ['Prior', 'Beta(1, 1) — uniform'],
            ].map(([k, v]) => (
              <div key={k} style={{ display: 'flex', justifyContent: 'space-between' }}>
                <span style={{ color: 'var(--fg-muted)' }}>{k}</span>
                <span className="mono-key">{v}</span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  )
}

// ─── Variant breakdown ───────────────────────────────────────────────────────

const PALETTE = ['#8B8D96', '#F0461F', '#5BAEF5', '#3DD68C', '#A892FF']

function VariantBreakdown({ adapter, useBayesian }: { adapter: VizAdapter; useBayesian: boolean }) {
  const v3 = adapter.variants > 2
  const variants = v3 ? [
    { name: 'control', alloc: 34, samples: '40,812', rate: '9.84%', mean: '$24.10', winner: false, vsControl: '—', ci: '[9.31%, 10.39%]' },
    { name: 'variant-a', alloc: 33, samples: '39,694', rate: '10.21%', mean: '$25.88', winner: false, vsControl: useBayesian ? '62%' : '0.184', ci: '[−0.6%, +5.1%]', lift: '+3.8%' },
    { name: 'variant-b', alloc: 33, samples: '39,901', rate: '12.04%', mean: '$28.92', winner: true, vsControl: useBayesian ? '94%' : '0.012', ci: '[+1.4%, +6.8%]', lift: '+22.4%' },
  ] : [
    { name: 'control', alloc: 50, samples: '206,418', rate: '11.42%', mean: '$28.41', winner: false, vsControl: '—', ci: '[10.92%, 11.94%]' },
    { name: 'variant-a', alloc: 50, samples: '205,997', rate: '11.96%', mean: '$29.74', winner: true, vsControl: useBayesian ? `${adapter.confidence}%` : '0.0023', ci: '[+1.2%, +8.3%]', lift: '+4.7%' },
  ]

  return (
    <div className="card" style={{ marginTop: 18 }}>
      <div className="card-header">
        <div className="card-title">Variant breakdown</div>
        <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>{variants.length} arms · sticky bucketing</span>
      </div>
      <table className="table">
        <thead>
          <tr>
            <th>Variant</th><th>Allocation</th><th>Samples</th><th>Conv. rate</th><th>Mean</th><th>Lift</th>
            <th>{useBayesian ? 'P(best)' : 'p-value vs control'}</th>
            <th>{useBayesian ? '95% credible' : '95% CI'}</th>
          </tr>
        </thead>
        <tbody>
          {variants.map((v, i) => (
            <tr key={v.name} style={{ background: v.winner ? 'var(--accent-softer)' : 'transparent' }}>
              <td>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  <span style={{ width: 10, height: 10, borderRadius: 3, background: PALETTE[i] }} />
                  <span className={`badge ${v.winner ? 'accent' : ''}`}>{v.name}{v.winner && ' 🏆'}</span>
                </div>
              </td>
              <td>{v.alloc}%</td>
              <td style={{ fontFamily: 'var(--font-mono)' }}>{v.samples}</td>
              <td style={{ fontFamily: 'var(--font-mono)', color: v.winner ? 'var(--success)' : 'var(--fg)', fontWeight: v.winner ? 600 : 400 }}>{v.rate}</td>
              <td style={{ fontFamily: 'var(--font-mono)', color: v.winner ? 'var(--success)' : 'var(--fg)' }}>{v.mean}</td>
              <td style={{ fontFamily: 'var(--font-mono)', color: 'lift' in v && v.lift?.startsWith('+') ? 'var(--success)' : 'var(--fg-muted)' }}>{'lift' in v ? v.lift : '—'}</td>
              <td style={{ fontFamily: 'var(--font-mono)', fontWeight: v.winner ? 600 : 400 }}>{v.vsControl}</td>
              <td style={{ fontFamily: 'var(--font-mono)', color: 'var(--fg-muted)' }}>{v.ci}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

// ─── Multi-variant pairwise matrix ───────────────────────────────────────────

function MultiVariantMatrix({ useBayesian }: { useBayesian: boolean }) {
  const variants = ['control', 'variant-a', 'variant-b']
  const matrix = useBayesian
    ? [[null, 0.38, 0.06], [0.62, null, 0.11], [0.94, 0.89, null]]
    : [[null, 0.184, 0.012], [0.184, null, 0.041], [0.012, 0.041, null]]

  function cellColor(v: number | null): string {
    if (v === null) return 'var(--bg-sunken)'
    if (useBayesian) {
      if (v >= 0.95) return 'rgba(17,122,61,0.25)'
      if (v >= 0.80) return 'rgba(17,122,61,0.10)'
      if (v <= 0.05) return 'rgba(196,43,28,0.10)'
      return 'var(--surface)'
    }
    if (v < 0.05) return 'rgba(17,122,61,0.20)'
    if (v < 0.10) return 'rgba(184,118,26,0.15)'
    return 'var(--surface)'
  }

  const jointBest = [
    { name: 'control', p: useBayesian ? 4 : 8, color: '#8B8D96', winner: false },
    { name: 'variant-a', p: useBayesian ? 11 : 14, color: '#5BAEF5', winner: false },
    { name: 'variant-b', p: useBayesian ? 85 : 78, color: '#3DD68C', winner: true },
  ]

  return (
    <div className="card" style={{ marginTop: 18 }}>
      <div className="card-header">
        <div className="card-title"><I.layers size={14} /> Pairwise comparison · {useBayesian ? 'P(row beats col)' : 'p-value'}</div>
        <div style={{ display: 'flex', gap: 8, fontSize: 11, color: 'var(--fg-muted)', alignItems: 'center' }}>
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}><span style={{ width: 12, height: 12, background: 'rgba(17,122,61,0.25)', borderRadius: 2 }} /> significant</span>
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}><span style={{ width: 12, height: 12, background: 'rgba(184,118,26,0.15)', borderRadius: 2 }} /> trending</span>
          <span style={{ marginLeft: 8 }}>{useBayesian ? 'MCMC posterior' : 'Bonferroni-corrected'}</span>
        </div>
      </div>
      <div style={{ padding: 18, display: 'grid', gridTemplateColumns: '1.2fr 1fr', gap: 24 }}>
        <div>
          <table className="table" style={{ border: '1px solid var(--border)', borderRadius: 6, overflow: 'hidden' }}>
            <thead>
              <tr>
                <th></th>
                {variants.map((v) => <th key={v} style={{ textAlign: 'center', fontFamily: 'var(--font-mono)', textTransform: 'none', letterSpacing: 0, fontSize: 11 }}>vs {v}</th>)}
              </tr>
            </thead>
            <tbody>
              {variants.map((row, ri) => (
                <tr key={row}>
                  <td style={{ fontFamily: 'var(--font-mono)', fontWeight: 600, background: 'var(--bg-sunken)', fontSize: 11 }}>{row}</td>
                  {variants.map((col, ci) => {
                    const v = matrix[ri][ci]
                    return (
                      <td key={col} style={{ textAlign: 'center', fontFamily: 'var(--font-mono)', fontSize: 12, fontWeight: 600, background: cellColor(v), color: v === null ? 'var(--fg-faint)' : 'var(--fg)', borderLeft: '1px solid var(--border-faint)' }}>
                        {v === null ? '—' : useBayesian ? `${(v * 100).toFixed(0)}%` : v.toFixed(3)}
                      </td>
                    )
                  })}
                </tr>
              ))}
            </tbody>
          </table>
          <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 10, lineHeight: 1.5 }}>
            <I.info size={11} style={{ verticalAlign: 'middle', marginRight: 4 }} />
            Read row → col. {useBayesian ? 'Cells show posterior probability that the row variant beats the column variant.' : 'P-values are Bonferroni-corrected for 3 simultaneous comparisons (α/3 = 0.0167).'}
          </div>
        </div>
        <div>
          <div style={{ fontSize: 11, color: 'var(--fg-muted)', textTransform: 'uppercase', letterSpacing: '0.05em', fontWeight: 600, marginBottom: 8 }}>Joint posterior · P(variant is best)</div>
          {jointBest.map((v) => (
            <div key={v.name} style={{ marginBottom: 10 }}>
              <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: 12, marginBottom: 4 }}>
                <span style={{ fontFamily: 'var(--font-mono)', fontWeight: v.winner ? 700 : 500 }}>
                  {v.name}{v.winner && <span style={{ marginLeft: 6, color: 'var(--accent)' }}>🏆</span>}
                </span>
                <span style={{ fontFamily: 'var(--font-mono)', fontWeight: 600 }}>{v.p}%</span>
              </div>
              <div style={{ height: 8, background: 'var(--bg-sunken)', borderRadius: 4, overflow: 'hidden' }}>
                <div style={{ width: `${v.p}%`, height: '100%', background: v.color, transition: 'width 0.4s' }} />
              </div>
            </div>
          ))}
          <div className="hr" />
          <div style={{ padding: 10, background: 'var(--success-bg)', border: '1px solid rgba(17,122,61,0.25)', borderRadius: 6, fontSize: 12 }}>
            <div style={{ fontWeight: 600, color: 'var(--success)', marginBottom: 4 }}>Recommendation: ship variant-b</div>
            <div style={{ color: 'var(--fg-muted)', lineHeight: 1.5 }}>
              Beats control with {useBayesian ? '94%' : 'p=0.012'} and beats variant-a with {useBayesian ? '89%' : 'p=0.041'}. Expected loss vs best-case: 0.06%.
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

// ─── Config tab ──────────────────────────────────────────────────────────────

function ExpConfig({ display, metricNames }: { display: ExperimentDisplay; metricNames: string[] }) {
  // Phase 7 cutover: display the resolved metric names (from
  // `metric_definitions.name`) instead of the legacy free-form `primary_metric`
  // event key. Falls back to the raw metric_id when name resolution hasn't
  // landed yet.
  const primaryLabel = metricNames[0] ?? display.primary
  const secondary = metricNames.slice(1)
  const rows: [string, React.ReactNode][] = [
    ['Bound flag', <span className="mono-key">{display.flag}</span>],
    ['Statistical model', <span className="badge">{display.model}</span>],
    ['Primary metric', <span style={{ fontWeight: 600 }}>{primaryLabel}</span>],
    [
      'Secondary metrics',
      secondary.length > 0
        ? <span style={{ fontSize: 12 }}>{secondary.join(' · ')}</span>
        : <span style={{ fontSize: 12, color: 'var(--fg-muted)' }}>—</span>,
    ],
    ['Allocation', <span style={{ fontFamily: 'var(--font-mono)' }}>50 / 50</span>],
    ['Targeting', <span style={{ fontSize: 12 }}>segment <span className="mono-key">beta-customers</span> AND country in [US, CA]</span>],
    ['Duration', <span>14 days (started {display.started})</span>],
    ['Min sample size', <span style={{ fontFamily: 'var(--font-mono)' }}>380,000</span>],
    ['MDE', <span style={{ fontFamily: 'var(--font-mono)' }}>2.0%</span>],
    ['α / β', <span style={{ fontFamily: 'var(--font-mono)' }}>0.05 / 0.20</span>],
    ['CUPED', <span className="badge success">enabled · variance reduction 18%</span>],
  ]
  const lifecycle = [
    ['Drafted', 'Marco G.', 'Apr 19'],
    ['Review approved', 'Priya R.', 'Apr 19'],
    ['Started', 'system', 'Apr 19, 09:14'],
    ['Stats run', 'scheduler', 'every 60m'],
    ['Ready to ship', 'reached 95%', 'Apr 25'],
  ]
  return (
    <div className="split-2-third">
      <div className="card">
        <div className="card-header"><div className="card-title">Configuration</div></div>
        <div className="card-body" style={{ padding: 0 }}>
          {rows.map(([k, v], i) => (
            <div key={i} style={{ display: 'flex', padding: '10px 18px', borderBottom: i < rows.length - 1 ? '1px solid var(--border-faint)' : 'none', alignItems: 'center' }}>
              <div style={{ width: 200, color: 'var(--fg-muted)', fontSize: 12 }}>{k}</div>
              <div style={{ flex: 1 }}>{v}</div>
            </div>
          ))}
        </div>
      </div>
      <div className="card">
        <div className="card-header"><div className="card-title">Lifecycle</div></div>
        <div className="card-body">
          {lifecycle.map((e, i) => (
            <div key={i} style={{ display: 'flex', gap: 10, padding: '8px 0', borderBottom: i < lifecycle.length - 1 ? '1px solid var(--border-faint)' : 'none' }}>
              <div style={{ width: 8, height: 8, borderRadius: '50%', background: i === lifecycle.length - 1 ? 'var(--accent)' : 'var(--success)', marginTop: 4, flexShrink: 0 }} />
              <div style={{ flex: 1, fontSize: 12 }}>
                <div style={{ fontWeight: 600 }}>{e[0]}</div>
                <div style={{ color: 'var(--fg-muted)' }}>{e[1]} · {e[2]}</div>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}

// ─── ExperimentDetail ────────────────────────────────────────────────────────

export function ExperimentDetail() {
  const { key } = useParams<{ key: string }>()
  const { envId } = useOrgContext()
  const [apiExp, setApiExp] = useState<ExperimentSummary | null>(null)
  const [apiResults, setApiResults] = useState<ExperimentResults | null>(null)
  const [apiLoading, setApiLoading] = useState(false)
  const [apiError, setApiError] = useState<string | null>(null)
  const [metricNames, setMetricNames] = useState<string[]>([])

  useEffect(() => {
    if (!envId || !key) return
    setApiLoading(true)
    setApiError(null)
    const ctrl = new AbortController()
    Promise.all([
      getExperiment(envId, key, ctrl.signal),
      getExperimentResults(envId, key, ctrl.signal).catch((err) => {
        // 409 = no completed iteration yet, 404 = no results endpoint result.
        // Both are non-fatal — render the page with summary-only data.
        if (ctrl.signal.aborted) throw err
        return null
      }),
    ])
      .then(([exp, results]) => {
        setApiExp(exp)
        setApiResults(results)
      })
      .catch((err) => {
        if (ctrl.signal.aborted) return
        setApiError(extractErrorMessage(err))
      })
      .finally(() => {
        if (!ctrl.signal.aborted) setApiLoading(false)
      })
    return () => ctrl.abort()
  }, [envId, key])

  // Phase 7: resolve metric IDs → names by fetching the metric list once.
  useEffect(() => {
    const ids = apiExp?.metric_ids
    if (!envId || !ids || ids.length === 0) {
      setMetricNames([])
      return
    }
    const ctrl = new AbortController()
    api
      .get<{ items: MetricNameRow[] }>(
        `/v1/metrics?env_id=${encodeURIComponent(envId)}&limit=200`,
        { signal: ctrl.signal },
      )
      .then(({ data }) => {
        const byId = new Map((data.items ?? []).map((m) => [m.id, m.name]))
        setMetricNames(ids.map((id) => byId.get(id) ?? id))
      })
      .catch(() => {
        // Best-effort — config tab falls back to the raw metric_id.
      })
    return () => ctrl.abort()
  }, [envId, apiExp])

  if (apiLoading) {
    return (
      <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
        <span style={{ color: 'var(--fg-muted)', fontSize: 14 }}>Loading experiment…</span>
      </div>
    )
  }

  if (apiError || !apiExp) {
    return (
      <div className="page-body">
        <div style={{ padding: '12px 16px', background: 'var(--danger-bg)', border: '1px solid rgba(196,43,28,0.3)', borderRadius: 8, color: 'var(--danger)', fontSize: 13 }}>
          <I.alert size={14} style={{ verticalAlign: 'middle', marginRight: 6 }} />
          {apiError ?? 'Experiment not found'}
        </div>
      </div>
    )
  }

  // Context types come from the results envelope when present, otherwise fall
  // back to ['user'] so the provider has at least one valid value.
  const contextTypes = listContextTypes(apiResults)
  const resolvedContextTypes = contextTypes.length > 0 ? contextTypes : ['user']

  return (
    <ContextTypeProvider
      contextTypes={resolvedContextTypes}
      experimentId={apiExp.key}
    >
      <ExperimentDetailBody
        apiExp={apiExp}
        apiResults={apiResults}
        metricNames={metricNames}
      />
    </ContextTypeProvider>
  )
}

interface BodyProps {
  apiExp: ExperimentSummary
  apiResults: ExperimentResults | null
  metricNames: string[]
}

function ExperimentDetailBody({ apiExp, apiResults, metricNames }: BodyProps) {
  const navigate = useNavigate()
  const { tweaks } = useTweaks()
  const { orgId } = useOrgContext()
  const { contextTypes, activeContextType, setActiveContextType } =
    useActiveContextType()

  const [tab, setTab] = useState<Tab>('results')
  const [vizOverride, setVizOverride] = useState<'auto' | 'frequentist' | 'bayesian'>(tweaks.expViz)

  // The viz toggle picks Bayesian when:
  //   • explicit override is bayesian, OR
  //   • auto + experiment is bayesian, OR
  //   • global tweak is bayesian and no explicit override
  const useBayesian =
    vizOverride === 'bayesian' ||
    (vizOverride === 'auto' && apiExp.model === 'bayesian') ||
    (vizOverride !== 'frequentist' && tweaks.expViz === 'bayesian')

  // Build the display model from real API responses — Phase 9 will replace
  // the remaining placeholder "—" / 0 fields with iteration-aware values.
  const display = buildExperimentDisplay(apiExp, apiResults, activeContextType, useBayesian)
  const adapter: VizAdapter = {
    variants: display.variants,
    confidence: display.confidence,
  }

  // Picked block — used to drive samples accurately when results are present.
  const picked = pickContextTypeResult(apiResults, activeContextType)
  const sampleCount = picked?.block.variants.reduce((a, v) => a + (v.sample_size ?? 0), 0) ?? 0

  const displayName = apiExp.name
  const displayKey = apiExp.key
  const displayStatus = apiExp.status

  // Show the context-type switcher only when results actually carry more than
  // one context type — otherwise the picker would have a single static option.
  const showCtxSwitcher = contextTypes.length > 1

  return (
    <>
      <PageHeader
        crumbs={[<a key="1" onClick={() => navigate(`/org/${orgId}/experiments`)} style={{ cursor: 'pointer' }}>Experiments</a>, displayKey]}
        title={displayName}
        subtitle={`Bound to flag ${display.flag} · ${display.model} model · started ${display.started}`}
        badge={<span className={`badge ${displayStatus === 'running' ? 'info' : 'success'}`}>{displayStatus}</span>}
        actions={
          <>
            {displayStatus === 'running' && (
              <>
                <button className="btn"><I.refresh size={13} /> Recompute</button>
                <button className="btn danger"><I.pause size={13} /> Stop</button>
              </>
            )}
            {displayStatus === 'running' && display.confidence >= 95 && (
              <button className="btn primary"><I.rocket size={13} /> Ship winner</button>
            )}
          </>
        }
      />
      <div className="page-body">
        {showCtxSwitcher && (
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 10,
              marginBottom: 14,
              fontSize: 12,
              color: 'var(--fg-muted)',
            }}
          >
            <span>Context type</span>
            <ContextTypeTabs
              contextTypes={contextTypes}
              activeContextType={activeContextType}
              onChange={setActiveContextType}
            />
          </div>
        )}

        <div className="tabs">
          <button className={`tab ${tab === 'results' ? 'active' : ''}`} onClick={() => setTab('results')}>Results</button>
          <button className={`tab ${tab === 'config' ? 'active' : ''}`} onClick={() => setTab('config')}>Configuration</button>
          <button className={`tab ${tab === 'metrics' ? 'active' : ''}`} onClick={() => setTab('metrics')}>Metrics <span className="count">{metricNames.length || apiExp.metric_ids.length || 0}</span></button>
          <button className={`tab ${tab === 'events' ? 'active' : ''}`} onClick={() => setTab('events')}>Events</button>
        </div>

        {tab === 'results' && (
          <>
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 14, padding: '10px 14px', background: 'var(--bg-sunken)', border: '1px solid var(--border)', borderRadius: 8 }}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 14, fontSize: 12, color: 'var(--fg-muted)' }}>
                <span><I.refresh size={11} style={{ verticalAlign: 'middle', marginRight: 4 }} />Computed <strong style={{ color: 'var(--fg)' }}>14 minutes ago</strong></span>
                <span>·</span>
                <span>Next run in 46m</span>
                <span>·</span>
                <span style={{ fontFamily: 'var(--font-mono)' }}>job_id: cmp_8a2f1c…</span>
              </div>
              <div style={{ display: 'flex', gap: 6 }}>
                <button className="btn sm" style={{ background: useBayesian ? 'var(--surface)' : 'transparent', borderColor: useBayesian ? 'var(--accent)' : 'var(--border-strong)', color: useBayesian ? 'var(--accent)' : 'var(--fg)' }} onClick={() => setVizOverride('bayesian')}>Bayesian</button>
                <button className="btn sm" style={{ background: !useBayesian ? 'var(--surface)' : 'transparent', borderColor: !useBayesian ? 'var(--accent)' : 'var(--border-strong)', color: !useBayesian ? 'var(--accent)' : 'var(--fg)' }} onClick={() => setVizOverride('frequentist')}>Frequentist</button>
                <button className="btn sm">CUPED</button>
              </div>
            </div>

            <div className="stat-grid" style={{ marginBottom: 14 }}>
              <div className="stat"><div className="stat-label">Lift (relative)</div><div className="stat-value" style={{ color: 'var(--success)' }}>{display.lift}</div><div className="stat-delta">vs control</div></div>
              <div className="stat"><div className="stat-label">{useBayesian ? 'P(variant > control)' : 'P-value'}</div><div className="stat-value">{useBayesian ? `${display.confidence}%` : (display.confidence > 0 ? (1 - display.confidence / 100).toFixed(4) : '—')}</div><div className="stat-delta">{useBayesian ? 'Bayesian posterior' : 'two-sided'}</div></div>
              <div className="stat"><div className="stat-label">Samples</div><div className="stat-value">{sampleCount > 0 ? sampleCount.toLocaleString('en-US') : display.samples}</div><div className="stat-delta">control + {display.variants - 1} variant{display.variants > 2 ? 's' : ''}</div></div>
              <div className="stat"><div className="stat-label">Time remaining</div><div className="stat-value" style={{ fontSize: 22 }}>{display.remaining}</div><div className="stat-delta">{display.remaining === 'ready' ? 'min sample size reached' : 'of 14d'}</div></div>
            </div>

            {useBayesian ? <BayesianViz adapter={adapter} /> : <FrequentistViz />}
            <VariantBreakdown adapter={adapter} useBayesian={useBayesian} />
            {display.variants > 2 && <MultiVariantMatrix useBayesian={useBayesian} />}
          </>
        )}

        {tab === 'config' && <ExpConfig display={display} metricNames={metricNames} />}

        {tab === 'metrics' && (
          <div className="card">
            <table className="table">
              <thead><tr><th>Metric</th><th>Type</th><th>Aggregation</th><th>Role</th><th>Threshold</th></tr></thead>
              <tbody>
                {(metricNames.length > 0 ? metricNames : apiExp.metric_ids).map((m, i) => (
                  <tr key={m}>
                    <td><span className="mono-key">{m}</span></td>
                    <td><span className="type-pill bool">—</span></td>
                    <td>—</td>
                    <td><span className={`badge ${i === 0 ? 'accent' : ''}`}>{i === 0 ? 'primary' : 'secondary'}</span></td>
                    <td>—</td>
                  </tr>
                ))}
                {apiExp.metric_ids.length === 0 && (
                  <tr><td colSpan={5} style={{ textAlign: 'center', color: 'var(--fg-muted)', padding: 24 }}>No metrics attached</td></tr>
                )}
              </tbody>
            </table>
          </div>
        )}

        {tab === 'events' && (
          <div className="card">
            <div className="empty">
              <div className="empty-icon"><I.event size={20} /></div>
              <div className="empty-title">Event stream view</div>
              <div className="empty-desc">Live tail of qualifying events from ClickHouse — useful for debugging metric definitions.</div>
            </div>
          </div>
        )}
      </div>
    </>
  )
}
