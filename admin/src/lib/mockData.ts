export interface FlagVariant {
  name: string
  value?: boolean | string | number
  alloc: number
}

export interface Flag {
  key: string
  name: string
  type: 'bool' | 'string' | 'double' | 'int' | 'json'
  state: 'on' | 'off'
  env: string
  rules: number
  segments: string[]
  lastEval: string
  evalDelta: string
  updated: string
  owner: string
  variants: FlagVariant[]
  experiments: number
  sparkline: number[]
  staged?: boolean
  archived?: boolean
}

export interface Segment {
  key: string
  name: string
  type: 'rule' | 'list'
  rules: number
  members: number
  contextType: string
  updated: string
  usedBy: number
}

export interface Experiment {
  key: string
  name: string
  flag: string
  model: 'Bayesian' | 'Frequentist'
  state: 'running' | 'draft' | 'stopped' | 'completed'
  started: string
  remaining: string
  primary: string
  samples: string
  lift: string
  confidence: number
  variants: number
}

export interface Event {
  key: string
  type: string
  described: string
  env: string
  volume30d: string
  lastSeen: string
  schema: string
}

export interface Environment {
  key: string
  name: string
  color: string
  flags: number
  keys: number
  latencyP95: string
  evalRate: string
}

export interface SdkKey {
  id: string
  env: string
  created: string
  lastUsed: string
  status: 'active' | 'deprecated'
  rotated: boolean
  by: string
}

export interface Member {
  name: string
  email: string
  role: string
  projects: number
  mfa: boolean
  last: string
  initials: string
  color: string
}

export interface AuditEntry {
  t: string
  actor: string
  action: string
  resource: string
  env: string
  change: string
}

export const FLAGS: Flag[] = [
  { key: 'checkout-v2', name: 'Checkout V2', type: 'bool', state: 'on', env: 'production', rules: 4, segments: ['beta-customers', 'internal'], lastEval: '2.1M', evalDelta: '+12%', updated: '3h ago', owner: 'payments', variants: [{ name: 'on', value: true, alloc: 30 }, { name: 'off', value: false, alloc: 70 }], experiments: 1, sparkline: [4, 6, 8, 7, 9, 12, 14, 16, 15, 18, 22, 28] },
  { key: 'payments-fraud-shield', name: 'Payments Fraud Shield', type: 'string', state: 'on', env: 'production', rules: 7, segments: ['high-risk-merchants'], lastEval: '890K', evalDelta: '+3%', updated: 'yesterday', owner: 'trust-safety', variants: [{ name: 'strict', value: 'strict', alloc: 50 }, { name: 'standard', value: 'standard', alloc: 35 }, { name: 'lenient', value: 'lenient', alloc: 15 }], experiments: 0, sparkline: [12, 11, 13, 12, 10, 11, 14, 13, 12, 11, 10, 12] },
  { key: 'ml-ranker-v3', name: 'ML Ranker V3', type: 'double', state: 'on', env: 'production', rules: 2, segments: ['search-test-cohort'], lastEval: '5.4M', evalDelta: '+38%', updated: '1d ago', owner: 'search', variants: [{ name: 'control', value: 1.0, alloc: 50 }, { name: 'v3', value: 1.34, alloc: 50 }], experiments: 1, sparkline: [2, 3, 5, 7, 9, 12, 16, 22, 28, 35, 40, 48] },
  { key: 'onboarding-walkthrough', name: 'Onboarding Walkthrough', type: 'json', state: 'on', env: 'production', rules: 3, segments: ['new-signups-7d'], lastEval: '120K', evalDelta: '+8%', updated: '2d ago', owner: 'growth', variants: [{ name: 'variant-a', alloc: 34 }, { name: 'variant-b', alloc: 33 }, { name: 'variant-c', alloc: 33 }], experiments: 1, sparkline: [3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9] },
  { key: 'support-ai-summarize', name: 'Support AI Summarize', type: 'bool', state: 'on', env: 'production', rules: 1, segments: ['pro-tier', 'enterprise'], lastEval: '44K', evalDelta: '+220%', updated: '5h ago', owner: 'support', variants: [{ name: 'on', alloc: 100 }, { name: 'off', alloc: 0 }], experiments: 0, sparkline: [1, 1, 2, 3, 5, 8, 13, 21, 28, 33, 40, 44] },
  { key: 'billing-upgrade-modal', name: 'Billing Upgrade Modal', type: 'bool', state: 'off', env: 'production', rules: 0, segments: [], lastEval: '0', evalDelta: '—', updated: '1w ago', owner: 'growth', variants: [{ name: 'on', alloc: 0 }, { name: 'off', alloc: 100 }], experiments: 0, sparkline: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
  { key: 'rate-limit-tier-2', name: 'Rate Limit Tier 2', type: 'int', state: 'on', env: 'production', rules: 5, segments: ['api-power-users'], lastEval: '11M', evalDelta: '+1%', updated: '3w ago', owner: 'platform', variants: [{ name: '5000', alloc: 100 }], experiments: 0, sparkline: [10, 11, 11, 10, 11, 11, 12, 11, 11, 11, 11, 11] },
  { key: 'dashboard-redesign-2026', name: 'Dashboard Redesign 2026', type: 'string', state: 'on', env: 'production', rules: 2, segments: ['beta-customers'], lastEval: '84K', evalDelta: '—', updated: '10m ago', owner: 'design', variants: [{ name: 'new', alloc: 10 }, { name: 'current', alloc: 90 }], experiments: 1, sparkline: [0, 0, 1, 2, 4, 6, 7, 8, 8, 8, 8, 9], staged: true },
  { key: 'slow-query-killer', name: 'Slow Query Killer', type: 'double', state: 'on', env: 'production', rules: 1, segments: [], lastEval: '0', evalDelta: '—', updated: '2mo ago', owner: 'platform', variants: [{ name: '30s', alloc: 100 }], experiments: 0, sparkline: [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1] },
]

export const SEGMENTS: Segment[] = [
  { key: 'beta-customers', name: 'Beta Customers', type: 'rule', rules: 3, members: 1247, contextType: 'user', updated: '2d ago', usedBy: 4 },
  { key: 'internal', name: 'Internal Users', type: 'list', rules: 0, members: 86, contextType: 'user', updated: '1w ago', usedBy: 12 },
  { key: 'high-risk-merchants', name: 'High-Risk Merchants', type: 'rule', rules: 5, members: 2134, contextType: 'merchant', updated: '5h ago', usedBy: 2 },
  { key: 'new-signups-7d', name: 'New Signups (≤7 days)', type: 'rule', rules: 1, members: 8412, contextType: 'user', updated: 'auto', usedBy: 3 },
  { key: 'pro-tier', name: 'Pro Tier', type: 'rule', rules: 2, members: 14209, contextType: 'account', updated: '3w ago', usedBy: 6 },
  { key: 'enterprise', name: 'Enterprise', type: 'rule', rules: 4, members: 318, contextType: 'account', updated: '1mo ago', usedBy: 9 },
  { key: 'api-power-users', name: 'API Power Users', type: 'rule', rules: 2, members: 422, contextType: 'user', updated: '5d ago', usedBy: 1 },
  { key: 'search-test-cohort', name: 'Search Test Cohort', type: 'list', rules: 0, members: 5000, contextType: 'user', updated: 'yesterday', usedBy: 1 },
]

export const EXPERIMENTS: Experiment[] = [
  { key: 'checkout-v2-conv', name: 'Checkout V2 — Conversion Lift', flag: 'checkout-v2', model: 'Bayesian', state: 'running', started: '8d ago', remaining: '6 days', primary: 'checkout_completed', samples: '412K', lift: '+4.7%', confidence: 96, variants: 2 },
  { key: 'ml-ranker-ctr', name: 'ML Ranker — CTR', flag: 'ml-ranker-v3', model: 'Frequentist', state: 'running', started: '21d ago', remaining: 'ready', primary: 'search_click', samples: '5.4M', lift: '+12.3%', confidence: 99, variants: 2 },
  { key: 'onboarding-3way', name: 'Onboarding — Walkthrough A/B/C', flag: 'onboarding-walkthrough', model: 'Bayesian', state: 'running', started: '14d ago', remaining: 'ready', primary: 'first_session_complete', samples: '120K', lift: '+2.1%', confidence: 71, variants: 3 },
  { key: 'dashboard-redesign-eng', name: 'Dashboard Redesign — Engagement', flag: 'dashboard-redesign-2026', model: 'Frequentist', state: 'draft', started: '—', remaining: '—', primary: 'dau_minutes', samples: '0', lift: '—', confidence: 0, variants: 2 },
  { key: 'fraud-strict-disputes', name: 'Fraud Strict — Dispute Rate', flag: 'payments-fraud-shield', model: 'Frequentist', state: 'stopped', started: '32d ago', remaining: '—', primary: 'dispute_filed', samples: '890K', lift: '−1.2%', confidence: 88, variants: 3 },
  { key: 'support-ai-csat', name: 'Support AI — CSAT', flag: 'support-ai-summarize', model: 'Bayesian', state: 'completed', started: '60d ago', remaining: '—', primary: 'csat_score', samples: '44K', lift: '+0.4', confidence: 99, variants: 2 },
]

export const EVENTS: Event[] = [
  { key: 'checkout_completed', type: 'bool', described: 'User completed checkout flow', env: 'production', volume30d: '1.2M', lastSeen: '2s ago', schema: '{ user_id, cart_value, currency }' },
  { key: 'search_click', type: 'int', described: 'User clicked a search result (position)', env: 'production', volume30d: '44.1M', lastSeen: '0s ago', schema: '{ user_id, query, position }' },
  { key: 'first_session_complete', type: 'bool', described: 'User finished first session', env: 'production', volume30d: '84K', lastSeen: '11s ago', schema: '{ user_id }' },
  { key: 'dispute_filed', type: 'bool', described: 'Customer filed a payment dispute', env: 'production', volume30d: '1.4K', lastSeen: '3m ago', schema: '{ merchant_id, amount }' },
  { key: 'csat_score', type: 'double', described: 'CSAT score from support', env: 'production', volume30d: '6.7K', lastSeen: '12s ago', schema: '{ ticket_id, score }' },
  { key: 'dau_minutes', type: 'double', described: 'Daily active minutes per user', env: 'production', volume30d: '12M', lastSeen: '1s ago', schema: '{ user_id, minutes }' },
  { key: 'cart_abandoned', type: 'double', described: 'Cart abandoned with value', env: 'production', volume30d: '240K', lastSeen: '4s ago', schema: '{ user_id, cart_value }' },
]

export const ENVIRONMENTS: Environment[] = [
  { key: 'production', name: 'production', color: '#3DD68C', flags: 142, keys: 3, latencyP95: '8ms', evalRate: '11.2K/s' },
  { key: 'staging', name: 'staging', color: '#5BAEF5', flags: 142, keys: 2, latencyP95: '11ms', evalRate: '240/s' },
  { key: 'qa', name: 'qa', color: '#E6B14F', flags: 142, keys: 1, latencyP95: '9ms', evalRate: '12/s' },
  { key: 'dev', name: 'dev', color: '#A892FF', flags: 142, keys: 4, latencyP95: '—', evalRate: '—' },
]

export const SDK_KEYS: SdkKey[] = [
  { id: 'sk_prod_a8f2…91c0', env: 'production', created: '2025-12-04', lastUsed: 'just now', status: 'active', rotated: false, by: 'Priya Reddy' },
  { id: 'sk_prod_b1e7…44a2', env: 'production', created: '2025-09-18', lastUsed: '5m ago', status: 'active', rotated: false, by: 'Marco Greco' },
  { id: 'sk_prod_old…dd11', env: 'production', created: '2025-04-02', lastUsed: '12d ago', status: 'deprecated', rotated: true, by: 'Marco Greco' },
  { id: 'sk_stag_77e3…a02f', env: 'staging', created: '2026-01-22', lastUsed: '1m ago', status: 'active', rotated: false, by: 'Lin Tan' },
  { id: 'sk_qa_xx12…0f81', env: 'qa', created: '2026-03-12', lastUsed: 'yesterday', status: 'active', rotated: false, by: 'Lin Tan' },
]

export const MEMBERS: Member[] = [
  { name: 'Priya Reddy', email: 'priya@stitchd.dev', role: 'Owner', projects: 8, mfa: true, last: 'now', initials: 'PR', color: '#FF7A52' },
  { name: 'Marco Greco', email: 'marco@stitchd.dev', role: 'Admin', projects: 6, mfa: true, last: '5m ago', initials: 'MG', color: '#5BAEF5' },
  { name: 'Lin Tan', email: 'lin@stitchd.dev', role: 'Admin', projects: 4, mfa: true, last: '1h ago', initials: 'LT', color: '#3DD68C' },
  { name: 'Devon Hayes', email: 'devon@stitchd.dev', role: 'Member', projects: 2, mfa: false, last: 'yesterday', initials: 'DH', color: '#A892FF' },
  { name: 'Sara Okonkwo', email: 'sara@stitchd.dev', role: 'Member (custom: payments-write)', projects: 1, mfa: true, last: '2d ago', initials: 'SO', color: '#E6B14F' },
  { name: 'Ahmed Khan', email: 'ahmed@stitchd.dev', role: 'Member (read-only)', projects: 3, mfa: true, last: '3d ago', initials: 'AK', color: '#F26B5E' },
]

export const AUDIT: AuditEntry[] = [
  { t: 'now', actor: 'Priya Reddy', action: 'flag.update', resource: 'checkout-v2', env: 'production', change: 'rollout 20% → 30%' },
  { t: '12m ago', actor: 'Marco Greco', action: 'experiment.start', resource: 'checkout-v2-conv', env: 'production', change: 'started 14-day Bayesian experiment' },
  { t: '1h ago', actor: 'Lin Tan', action: 'segment.update', resource: 'high-risk-merchants', env: 'production', change: 'added rule: dispute_rate > 0.04' },
  { t: '2h ago', actor: 'system', action: 'stats.compute', resource: 'ml-ranker-ctr', env: 'production', change: 'scheduled stats run (Frequentist)' },
  { t: '5h ago', actor: 'Devon Hayes', action: 'sdk_key.rotate', resource: 'sk_prod_a8f2…91c0', env: 'production', change: 'created new key, deprecated sk_prod_old…dd11' },
  { t: 'yesterday', actor: 'Priya Reddy', action: 'member.invite', resource: 'ahmed@stitchd.dev', env: '—', change: 'invited as Member (read-only)' },
  { t: 'yesterday', actor: 'Marco Greco', action: 'flag.create', resource: 'dashboard-redesign-2026', env: 'production', change: 'created flag (string)' },
  { t: '2d ago', actor: 'Lin Tan', action: 'event.register', resource: 'csat_score', env: 'production', change: 'registered event (double)' },
  { t: '3d ago', actor: 'system', action: 'auth.oidc.sync', resource: 'okta', env: '—', change: 'synced 14 users from OIDC provider' },
]
