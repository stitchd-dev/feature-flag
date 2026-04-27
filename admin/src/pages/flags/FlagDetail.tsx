import { useState } from 'react'
import { useNavigate, useParams } from 'react-router-dom'
import { useTweaks } from '../../hooks/useTweaks'
import { PageHeader, VariantBar } from '../../components/primitives'
import { I } from '../../components/icons'
import { FLAGS, EXPERIMENTS } from '../../lib/mockData'
import type { Flag } from '../../lib/mockData'

// ─── Rule tree types ────────────────────────────────────────────────────────

type ClauseNode = { op: 'clause'; ctx?: string; attr: string; comparator: string; value: string }
type SegmentNode = { op: 'segment'; segmentKey: string; negate?: boolean }
type GroupNode = { op: 'AND' | 'OR' | 'NOT'; children: RuleNode[] }
type RuleNode = ClauseNode | SegmentNode | GroupNode

interface Rule {
  id: number
  contextTypes: string[]
  hit: string
  tree: RuleNode
  then: { variant?: string; alloc?: number } | { rollout: { name: string; alloc: number }[] }
}

const CTX_COLOR: Record<string, string> = {
  user: '#5BAEF5',
  merchant: '#3DD68C',
  device: '#A892FF',
  organization: '#F0B547',
  session: '#F0461F',
}

const MOCK_RULES: Rule[] = [
  {
    id: 1, contextTypes: ['user'], hit: '892K',
    tree: { op: 'AND', children: [
      { op: 'clause', attr: 'country', comparator: 'in', value: '[US, CA, GB]' },
      { op: 'clause', attr: 'plan', comparator: '==', value: 'pro' },
    ]},
    then: { variant: 'on', alloc: 100 },
  },
  {
    id: 2, contextTypes: ['user', 'merchant', 'device'], hit: '318K',
    tree: { op: 'AND', children: [
      { op: 'segment', segmentKey: 'high-value-cart' },
      { op: 'OR', children: [
        { op: 'clause', ctx: 'user', attr: 'plan', comparator: 'in', value: '[pro, enterprise]' },
        { op: 'AND', children: [
          { op: 'clause', ctx: 'user', attr: 'signup_age_days', comparator: '>=', value: '30' },
          { op: 'clause', ctx: 'user', attr: 'lifetime_orders', comparator: '>', value: '5' },
        ]},
      ]},
      { op: 'clause', ctx: 'merchant', attr: 'tier', comparator: '==', value: 'platinum' },
      { op: 'clause', ctx: 'device', attr: 'platform', comparator: 'in', value: '[ios, android]' },
      { op: 'NOT', children: [{ op: 'segment', segmentKey: 'internal' }] },
    ]},
    then: { rollout: [{ name: 'on', alloc: 50 }, { name: 'off', alloc: 50 }] },
  },
  {
    id: 3, contextTypes: ['merchant'], hit: '1.2M',
    tree: { op: 'AND', children: [
      { op: 'clause', attr: 'tier', comparator: '==', value: 'platinum' },
      { op: 'clause', attr: 'dispute_rate', comparator: '<', value: '0.02' },
    ]},
    then: { rollout: [{ name: 'on', alloc: 30 }, { name: 'off', alloc: 70 }] },
  },
  {
    id: 4, contextTypes: ['user'], hit: '44K',
    tree: { op: 'segment', segmentKey: 'beta-customers' },
    then: { variant: 'on', alloc: 100 },
  },
  {
    id: 5, contextTypes: ['device'], hit: '2.1K',
    tree: { op: 'NOT', children: [
      { op: 'clause', attr: 'user_agent', comparator: 'contains', value: 'stitchd-internal' },
    ]},
    then: { variant: 'off', alloc: 100 },
  },
]

// ─── RuleTree ───────────────────────────────────────────────────────────────

function RuleTree({ node, depth = 0, showCtx = false }: { node: RuleNode; depth?: number; showCtx?: boolean }) {
  if (node.op === 'clause') {
    return (
      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6, padding: '3px 8px', background: 'var(--surface)', border: '1px solid var(--border)', borderRadius: 5, fontFamily: 'var(--font-mono)', fontSize: 12 }}>
        {showCtx && node.ctx && (
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4, padding: '1px 5px', marginRight: 2, background: 'var(--bg-sunken)', border: '1px solid var(--border-faint)', borderRadius: 3, fontSize: 10, fontWeight: 600 }}>
            <span className="env-dot" style={{ width: 5, height: 5, background: CTX_COLOR[node.ctx] ?? 'var(--fg-muted)', boxShadow: 'none' }} />
            {node.ctx}
          </span>
        )}
        <span style={{ color: 'var(--info)' }}>{node.attr}</span>
        <span style={{ color: 'var(--fg-subtle)' }}>{node.comparator}</span>
        <span style={{ color: 'var(--success)' }}>{node.value}</span>
      </span>
    )
  }

  if (node.op === 'segment') {
    return (
      <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6, padding: '3px 8px', background: 'var(--accent-soft)', border: '1px solid var(--accent)', borderRadius: 5, fontFamily: 'var(--font-mono)', fontSize: 12, color: 'var(--accent)' }}>
        <I.users size={11} />
        <span style={{ color: 'var(--fg-subtle)' }}>segment</span>
        <span style={{ fontWeight: 600 }}>{node.segmentKey}</span>
      </span>
    )
  }

  const opColor = node.op === 'AND' ? 'var(--info)' : node.op === 'OR' ? '#A892FF' : 'var(--danger)'
  const showLabel = depth > 0

  return (
    <div style={{
      position: 'relative',
      padding: showLabel ? '10px 10px 10px 14px' : 0,
      background: showLabel ? 'var(--bg-sunken)' : 'transparent',
      borderRadius: 6,
      border: showLabel ? '1px solid var(--border-faint)' : 'none',
      borderLeft: showLabel ? `2px solid ${opColor}` : 'none',
    }}>
      {showLabel && (
        <div style={{ position: 'absolute', top: -8, left: 10, padding: '1px 7px', background: 'var(--surface)', border: `1px solid ${opColor}`, borderRadius: 4, fontFamily: 'var(--font-mono)', fontSize: 10, fontWeight: 700, color: opColor, letterSpacing: '0.05em' }}>{node.op}</div>
      )}
      <div style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: 6 }}>
        {node.op === 'NOT' ? (
          <>
            <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, fontWeight: 700, color: 'var(--danger)', padding: '2px 6px', background: 'var(--surface)', borderRadius: 3 }}>NOT</span>
            {node.children.map((c, i) => <RuleTree key={i} node={c} depth={depth + 1} showCtx={showCtx} />)}
          </>
        ) : (
          node.children.map((c, i) => (
            <span key={i} style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
              {i > 0 && <span style={{ fontFamily: 'var(--font-mono)', fontSize: 10, fontWeight: 700, color: opColor, padding: '2px 6px', background: 'var(--surface)', border: `1px solid ${opColor}33`, borderRadius: 3, letterSpacing: '0.05em' }}>{node.op}</span>}
              <RuleTree node={c} depth={depth + 1} showCtx={showCtx} />
            </span>
          ))
        )}
      </div>
    </div>
  )
}

// ─── Panels ─────────────────────────────────────────────────────────────────

function CtxPill({ kind }: { kind: string }) {
  return (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6, padding: '3px 9px', background: 'var(--surface)', border: '1px solid var(--border-strong)', borderRadius: 5, fontFamily: 'var(--font-mono)', fontSize: 12, fontWeight: 600 }}>
      <span className="env-dot" style={{ width: 6, height: 6, background: CTX_COLOR[kind] ?? 'var(--accent)', boxShadow: 'none' }} />
      {kind}
    </span>
  )
}

function RulesPanel({ flag }: { flag: Flag }) {
  const rules = MOCK_RULES.slice(0, Math.max(flag.rules || 4, 5))
  return (
    <div className="card">
      <div className="card-header">
        <div className="card-title"><I.toggle size={14} /> Targeting rules</div>
        <button className="btn sm"><I.plus size={12} /> Add rule</button>
      </div>
      <div style={{ padding: 14 }}>
        {rules.map((r, i) => (
          <div key={r.id} style={{ display: 'flex', gap: 10, padding: '14px 0', borderBottom: i < rules.length - 1 ? '1px solid var(--border-faint)' : 'none' }}>
            <div style={{ width: 28, height: 28, borderRadius: 6, background: 'var(--bg-sunken)', color: 'var(--fg-muted)', display: 'grid', placeItems: 'center', fontFamily: 'var(--font-mono)', fontSize: 12, fontWeight: 600, flexShrink: 0 }}>{i + 1}</div>
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8, flexWrap: 'wrap' }}>
                <span style={{ fontSize: 10, color: 'var(--fg-subtle)', textTransform: 'uppercase', letterSpacing: '0.06em', fontWeight: 600 }}>{r.contextTypes.length > 1 ? 'Contexts' : 'Context'}</span>
                {r.contextTypes.map((kind, ki) => (
                  <span key={kind} style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
                    {ki > 0 && <span style={{ fontSize: 10, color: 'var(--fg-subtle)', fontFamily: 'var(--font-mono)' }}>+</span>}
                    <CtxPill kind={kind} />
                  </span>
                ))}
                {r.contextTypes.length > 1 && <span style={{ fontSize: 10, padding: '2px 6px', background: 'var(--accent-soft)', color: 'var(--accent)', borderRadius: 3, fontWeight: 600, letterSpacing: '0.04em' }}>MULTI-CONTEXT</span>}
                <span style={{ fontSize: 11, color: 'var(--fg-subtle)' }}>where</span>
              </div>
              <RuleTree node={r.tree} depth={0} showCtx={r.contextTypes.length > 1} />
              <div style={{ marginTop: 10, paddingTop: 10, borderTop: '1px dashed var(--border-faint)' }}>
                <div style={{ fontSize: 10, color: 'var(--fg-subtle)', textTransform: 'uppercase', letterSpacing: '0.06em', marginBottom: 4, fontWeight: 600 }}>Serve</div>
                {'variant' in r.then ? (
                  <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                    <span className="badge accent">{r.then.variant}</span>
                    <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>to 100% of matching contexts</span>
                  </div>
                ) : (
                  <div style={{ maxWidth: 360 }}>
                    <VariantBar variants={(r.then as { rollout: { name: string; alloc: number }[] }).rollout} />
                  </div>
                )}
              </div>
            </div>
            <div style={{ textAlign: 'right', flexShrink: 0 }}>
              <div style={{ fontSize: 10, color: 'var(--fg-subtle)', textTransform: 'uppercase', letterSpacing: '0.06em', fontWeight: 600 }}>30d hit</div>
              <div style={{ fontFamily: 'var(--font-mono)', fontWeight: 700, fontSize: 13 }}>{r.hit}</div>
              <button className="icon-btn" style={{ marginTop: 4 }}><I.more size={14} /></button>
            </div>
          </div>
        ))}
        <div style={{ padding: '12px 0 0', display: 'flex', alignItems: 'center', gap: 10, fontSize: 12, color: 'var(--fg-muted)' }}>
          <I.info size={13} />
          <span>Default rule (catch-all): serve <span className="badge">off</span> to 100%</span>
        </div>
      </div>
    </div>
  )
}

const VARIANT_PALETTE = ['#F0461F', '#5BAEF5', '#3DD68C', '#A892FF', '#E6B14F']

function VariantsPanel({ flag, full }: { flag: Flag; full?: boolean }) {
  return (
    <div className="card">
      <div className="card-header">
        <div className="card-title"><I.layers size={14} /> Variants</div>
        <button className="btn sm"><I.plus size={12} /> Add variant</button>
      </div>
      <div style={{ padding: full ? 18 : 14 }}>
        <VariantBar variants={flag.variants} />
        <div style={{ marginTop: 14, display: 'flex', flexDirection: 'column', gap: 8 }}>
          {flag.variants.map((v, i) => (
            <div key={v.name} style={{ display: 'flex', alignItems: 'center', gap: 10, padding: '8px 10px', border: '1px solid var(--border)', borderRadius: 6, background: 'var(--surface-2)' }}>
              <div style={{ width: 10, height: 10, borderRadius: 3, background: VARIANT_PALETTE[i % VARIANT_PALETTE.length] }} />
              <span style={{ fontFamily: 'var(--font-mono)', fontWeight: 600, flex: 1 }}>{v.name}</span>
              {v.value !== undefined && <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, color: 'var(--fg-muted)' }}>= {String(v.value)}</span>}
              <span style={{ fontFamily: 'var(--font-mono)', fontWeight: 600, fontSize: 13 }}>{v.alloc}%</span>
              <button className="icon-btn"><I.pencil size={13} /></button>
            </div>
          ))}
        </div>
        <div className="hr" />
        <div style={{ fontSize: 12, color: 'var(--fg-muted)' }}>
          <strong style={{ color: 'var(--fg)' }}>Bucketing:</strong> hash(user.id, "{flag.key}", project, env) — sticky across evaluations. 0.1% granularity.
        </div>
      </div>
    </div>
  )
}

function EvalPanel() {
  return (
    <div className="card">
      <div className="card-header">
        <div className="card-title">Evaluations · last 30 days</div>
        <div style={{ display: 'flex', gap: 6 }}>
          <button className="btn sm">24h</button>
          <button className="btn sm" style={{ borderColor: 'var(--accent)', color: 'var(--accent)' }}>30d</button>
          <button className="btn sm">90d</button>
        </div>
      </div>
      <div style={{ padding: 18 }}>
        <svg viewBox="0 0 800 200" style={{ width: '100%', height: 200 }}>
          <defs>
            <linearGradient id="evGrad" x1="0" x2="0" y1="0" y2="1">
              <stop offset="0%" stopColor="var(--accent)" stopOpacity="0.3" />
              <stop offset="100%" stopColor="var(--accent)" stopOpacity="0" />
            </linearGradient>
          </defs>
          {[40, 80, 120, 160].map((y) => <line key={y} x1="0" x2="800" y1={y} y2={y} stroke="var(--border-faint)" strokeWidth="1" />)}
          <path d="M0,160 L60,140 L120,150 L180,120 L240,135 L300,100 L360,85 L420,95 L480,70 L540,55 L600,60 L660,40 L720,25 L800,20 L800,200 L0,200 Z" fill="url(#evGrad)" />
          <path d="M0,160 L60,140 L120,150 L180,120 L240,135 L300,100 L360,85 L420,95 L480,70 L540,55 L600,60 L660,40 L720,25 L800,20" fill="none" stroke="var(--accent)" strokeWidth="2" />
          <path d="M0,180 L60,175 L120,178 L180,170 L240,172 L300,165 L360,160 L420,162 L480,155 L540,150 L600,152 L660,148 L720,145 L800,143" fill="none" stroke="var(--info)" strokeWidth="1.5" strokeDasharray="3 3" />
        </svg>
        <div style={{ display: 'flex', gap: 18, marginTop: 12, fontSize: 12 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}><span style={{ width: 10, height: 2, background: 'var(--accent)', display: 'inline-block' }} /> on (served)</div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}><span style={{ width: 10, height: 2, background: 'var(--info)', display: 'inline-block', borderTop: '1px dashed var(--info)' }} /> off (served)</div>
          <div style={{ marginLeft: 'auto', color: 'var(--fg-muted)', fontFamily: 'var(--font-mono)' }}>p95 8ms · p99 14ms · errors 0.001%</div>
        </div>
      </div>
    </div>
  )
}

function FlagExpPanel({ flag }: { flag: Flag }) {
  const navigate = useNavigate()
  const exps = EXPERIMENTS.filter((e) => e.flag === flag.key)
  if (!exps.length) {
    return (
      <div className="card">
        <div className="empty">
          <div className="empty-icon"><I.beaker size={20} /></div>
          <div className="empty-title">No experiments yet</div>
          <div className="empty-desc">When you bind an experiment to this flag, it'll be locked from edits for the duration. Stats compute every 60 minutes.</div>
          <button className="btn primary" style={{ marginTop: 8 }}><I.plus size={13} /> Create experiment</button>
        </div>
      </div>
    )
  }
  return (
    <div className="card">
      <table className="table">
        <thead><tr><th>Experiment</th><th>Model</th><th>Status</th><th>Lift</th><th>Confidence</th><th>Samples</th><th></th></tr></thead>
        <tbody>
          {exps.map((e) => (
            <tr key={e.key} className="row-clickable" onClick={() => navigate(`/experiments/${e.key}`)}>
              <td><div style={{ fontWeight: 600 }}>{e.name}</div><div style={{ fontSize: 11, color: 'var(--fg-muted)' }}>{e.primary}</div></td>
              <td><span className="badge">{e.model}</span></td>
              <td><span className={`badge ${e.state === 'running' ? 'info' : 'success'}`}>{e.state}</span></td>
              <td style={{ color: e.lift.startsWith('+') ? 'var(--success)' : 'var(--danger)', fontFamily: 'var(--font-mono)' }}>{e.lift}</td>
              <td style={{ fontFamily: 'var(--font-mono)' }}>{e.confidence}%</td>
              <td style={{ fontFamily: 'var(--font-mono)' }}>{e.samples}</td>
              <td><I.chevronRight size={14} stroke="var(--fg-subtle)" /></td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function SdkSnippet({ flag }: { flag: Flag }) {
  const typeMap: Record<string, string> = { bool: 'bool', string: 'String', double: 'f64', int: 'i64', json: 'serde_json::Value' }
  return (
    <div className="split-2">
      <div className="card">
        <div className="card-header"><div className="card-title">Rust SDK</div><button className="btn sm"><I.copy size={12} /> Copy</button></div>
        <div className="card-body">
          <pre className="code">{`use stitchd_sdk::{SdkClient, Context};

let client = SdkClient::init(config).await?;

let ctx = Context::user("u_8421")
    .param("country", "US")
    .param("plan", "pro");

let value = client.evaluate::<${typeMap[flag.type] ?? flag.type}>(
    "${flag.key}", &ctx
).await?;`}</pre>
        </div>
      </div>
      <div className="card">
        <div className="card-header"><div className="card-title">Test evaluation</div><button className="btn primary sm"><I.play size={12} /> Run</button></div>
        <div className="card-body">
          <label className="label">Context</label>
          <pre className="code" style={{ marginBottom: 12 }}>{`{
  "_type": "user",
  "key": "u_8421",
  "parameters": {
    "country": "US",
    "plan": "pro",
    "signup_age_days": 14
  },
  "privateParameters": ["email"]
}`}</pre>
          <div style={{ padding: 12, background: 'var(--success-bg)', border: '1px solid rgba(17,122,61,0.25)', borderRadius: 6 }}>
            <div style={{ fontSize: 11, color: 'var(--success)', textTransform: 'uppercase', letterSpacing: '0.05em', fontWeight: 600 }}>Result</div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginTop: 6 }}>
              <span style={{ fontFamily: 'var(--font-mono)', fontSize: 18, fontWeight: 700 }}>{flag.variants[0]?.name}</span>
              <span style={{ fontSize: 11, color: 'var(--fg-muted)' }}>matched rule #1 · evaluated in 0.4ms</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

function FlagHistory() {
  const events = [
    { t: 'now', who: 'Priya Reddy', what: 'rollout 20% → 30%', kind: 'update' },
    { t: '12m ago', who: 'Marco Greco', what: 'added rule #2 (NOT segment is internal)', kind: 'create' },
    { t: '3h ago', who: 'system', what: 'evaluation rate +12% over baseline', kind: 'info' },
    { t: 'yesterday', who: 'Lin Tan', what: 'added segment beta-customers to rule #3', kind: 'update' },
    { t: '2d ago', who: 'Marco Greco', what: 'promoted from staging → production', kind: 'deploy' },
  ]
  return (
    <div className="card">
      <div className="card-body" style={{ padding: 0 }}>
        {events.map((e, i) => (
          <div key={i} style={{ display: 'flex', gap: 12, padding: '14px 18px', borderBottom: i < events.length - 1 ? '1px solid var(--border-faint)' : 'none' }}>
            <div className="user-avatar" style={{ background: e.who === 'system' ? 'var(--bg-sunken)' : 'linear-gradient(135deg, #FF8866, #C8361A)', color: e.who === 'system' ? 'var(--fg-muted)' : 'white', border: e.who === 'system' ? '1px solid var(--border)' : 'none' }}>
              {e.who === 'system' ? 'Σ' : e.who.split(' ').map((s) => s[0]).join('')}
            </div>
            <div style={{ flex: 1 }}>
              <div style={{ fontSize: 13 }}><strong>{e.who}</strong> {e.what}</div>
              <div style={{ fontSize: 11, color: 'var(--fg-muted)', marginTop: 2, fontFamily: 'var(--font-mono)' }}>{e.t} · {e.kind}</div>
            </div>
            <button className="btn sm">View diff</button>
          </div>
        ))}
      </div>
    </div>
  )
}

// ─── FlagDetail ──────────────────────────────────────────────────────────────

type Tab = 'targeting' | 'variants' | 'evals' | 'exp' | 'code' | 'history'

export function FlagDetail() {
  const { key } = useParams<{ key: string }>()
  const navigate = useNavigate()
  const { tweaks } = useTweaks()
  const flag = FLAGS.find((f) => f.key === key) ?? FLAGS[0]
  const [tab, setTab] = useState<Tab>('targeting')
  const [enabled, setEnabled] = useState(flag.state === 'on')
  const layout = tweaks.flagDetailLayout

  return (
    <>
      <PageHeader
        crumbs={[<a key="1" onClick={() => navigate('/flags')} style={{ cursor: 'pointer' }}>Flags</a>, flag.key]}
        title={flag.key}
        mono
        subtitle={`${flag.name} — ${flag.owner} team`}
        badge={
          <>
            <span className={`type-pill ${flag.type}`} style={{ fontSize: 11, padding: '2px 7px' }}>{flag.type}</span>
            {flag.staged && <span className="badge warning">staged changes</span>}
          </>
        }
        actions={
          <>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '6px 12px', border: '1px solid var(--border-strong)', borderRadius: 6, background: 'var(--surface)' }}>
              <span style={{ fontSize: 11, color: 'var(--fg-muted)', textTransform: 'uppercase', letterSpacing: '0.05em' }}>{enabled ? 'Enabled' : 'Disabled'}</span>
              <button className={`toggle lg ${enabled ? 'on' : ''}`} onClick={() => setEnabled(!enabled)} />
            </div>
            <button className="btn"><I.copy size={13} /> Copy key</button>
            <button className="btn"><I.history size={13} /> History</button>
            <button className="btn"><I.more size={14} /></button>
          </>
        }
      />
      <div className="page-body">
        {flag.staged && (
          <div style={{ background: 'var(--warning-bg)', border: '1px solid rgba(184,118,26,0.35)', borderRadius: 8, padding: '10px 14px', marginBottom: 16, display: 'flex', alignItems: 'center', gap: 10 }}>
            <I.alert size={16} stroke="var(--warning)" />
            <div style={{ flex: 1, fontSize: 13 }}>
              <strong>Staged changes</strong> — Marco Greco modified rule order 12m ago. Changes won't be applied until you publish.
            </div>
            <button className="btn sm">View diff</button>
            <button className="btn primary sm">Publish</button>
          </div>
        )}

        <div className="tabs">
          <button className={`tab ${tab === 'targeting' ? 'active' : ''}`} onClick={() => setTab('targeting')}><I.toggle size={13} /> Targeting <span className="count">{flag.rules}</span></button>
          <button className={`tab ${tab === 'variants' ? 'active' : ''}`} onClick={() => setTab('variants')}><I.layers size={13} /> Variants <span className="count">{flag.variants.length}</span></button>
          <button className={`tab ${tab === 'evals' ? 'active' : ''}`} onClick={() => setTab('evals')}><I.zap size={13} /> Evaluations</button>
          <button className={`tab ${tab === 'exp' ? 'active' : ''}`} onClick={() => setTab('exp')}><I.beaker size={13} /> Experiments <span className="count">{flag.experiments}</span></button>
          <button className={`tab ${tab === 'code' ? 'active' : ''}`} onClick={() => setTab('code')}><I.command size={13} /> SDK snippet</button>
          <button className={`tab ${tab === 'history' ? 'active' : ''}`} onClick={() => setTab('history')}><I.history size={13} /> History</button>
        </div>

        {tab === 'targeting' && (
          <div className={layout === 'side' ? 'split-2' : 'stack'}>
            <RulesPanel flag={flag} />
            <VariantsPanel flag={flag} />
          </div>
        )}
        {tab === 'variants' && <VariantsPanel flag={flag} full />}
        {tab === 'evals' && <EvalPanel />}
        {tab === 'exp' && <FlagExpPanel flag={flag} />}
        {tab === 'code' && <SdkSnippet flag={flag} />}
        {tab === 'history' && <FlagHistory />}
      </div>
    </>
  )
}
