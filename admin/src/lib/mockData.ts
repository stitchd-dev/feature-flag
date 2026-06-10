/**
 * Mock data — TRIMMED to only the entries still consumed by `src/pages/stubs.tsx`
 * (the not-yet-wired Events / Members / Audit pages).
 *
 * flag_eval_unify_20260522 / feature-flag-tcy + feature-flag-zh9 cleanup:
 *   The dashboard FLAGS/SEGMENTS/ENVIRONMENTS/SDK_KEYS arrays were removed
 *   along with the static service-health port table. The dashboard now reads
 *   real counts from the management API instead of these fakes. The Events,
 *   Members and Audit pages will follow in a future pass.
 */

export interface Event {
  key: string
  type: string
  described: string
  env: string
  volume30d: string
  lastSeen: string
  schema: string
}

export interface AuditEntry {
  t: string
  actor: string
  action: string
  resource: string
  env: string
  change: string
}

export const EVENTS: Event[] = [
  { key: 'checkout_completed', type: 'bool', described: 'User completed checkout flow', env: 'production', volume30d: '1.2M', lastSeen: '2s ago', schema: '{ user_id, cart_value, currency }' },
  { key: 'search_click', type: 'int', described: 'User clicked a search result (position)', env: 'production', volume30d: '44.1M', lastSeen: '0s ago', schema: '{ user_id, query, position }' },
  { key: 'first_session_complete', type: 'bool', described: 'User finished first session', env: 'production', volume30d: '84K', lastSeen: '11s ago', schema: '{ user_id }' },
  { key: 'dispute_filed', type: 'bool', described: 'Customer filed a payment dispute', env: 'production', volume30d: '1.4K', lastSeen: '3m ago', schema: '{ merchant_id, amount }' },
  { key: 'csat_score', type: 'double', described: 'CSAT score from support', env: 'production', volume30d: '6.7K', lastSeen: '12s ago', schema: '{ ticket_id, score }' },
  { key: 'dau_minutes', type: 'double', described: 'Daily active minutes per user', env: 'production', volume30d: '12M', lastSeen: '1s ago', schema: '{ user_id, minutes }' },
  { key: 'cart_abandoned', type: 'double', described: 'Cart abandoned with value', env: 'production', volume30d: '240K', lastSeen: '4s ago', schema: '{ user_id, cart_value }' },
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
