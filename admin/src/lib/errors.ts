/**
 * extractErrorMessage — centralized error-to-string mapper.
 *
 * Handles:
 *   - Axios error shape: err.response?.data?.message
 *   - Plain Error: err.message
 *   - Fallback: String(err)
 */
export function extractErrorMessage(err: unknown): string {
  if (err == null) return 'An unknown error occurred'
  if (typeof err === 'string') return err
  if (typeof err === 'object') {
    const e = err as Record<string, unknown>
    // Axios-style: { response: { data: { message: string } } }
    const responseData = (e.response as Record<string, unknown> | undefined)?.data
    if (responseData && typeof responseData === 'object') {
      const msg = (responseData as Record<string, unknown>).message
      if (typeof msg === 'string' && msg) return msg
    }
    // Plain Error or anything with .message
    if (typeof e.message === 'string' && e.message) return e.message
  }
  return String(err)
}
