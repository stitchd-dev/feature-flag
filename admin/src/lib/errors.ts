/**
 * extractErrorMessage — centralized error-to-string mapper.
 *
 * Handles:
 *   - Axios error shape: err.response?.data?.message (structured errors, e.g.
 *     422 invalid-distribution carries a human `message`)
 *   - Axios error shape: err.response?.data?.error — the gateway's standard
 *     error envelope is `{ "error": "<message>" }` for 400/401/404/409/502
 *     (e.g. validation moved server-side now surfaces here as a 400 BadRequest)
 *   - Axios with a plain-string response body
 *   - Plain Error: err.message
 *   - Fallback: String(err)
 */
export function extractErrorMessage(err: unknown): string {
  if (err == null) return 'An unknown error occurred'
  if (typeof err === 'string') return err
  if (typeof err === 'object') {
    const e = err as Record<string, unknown>
    const responseData = (e.response as Record<string, unknown> | undefined)?.data
    // Structured body: prefer a human `message`, then the gateway's `error` key.
    if (responseData && typeof responseData === 'object') {
      const data = responseData as Record<string, unknown>
      const msg = data.message
      if (typeof msg === 'string' && msg) return msg
      const errField = data.error
      if (typeof errField === 'string' && errField) return errField
    }
    // Plain-string response body.
    if (typeof responseData === 'string' && responseData) return responseData
    // Plain Error or anything with .message
    if (typeof e.message === 'string' && e.message) return e.message
  }
  return String(err)
}
