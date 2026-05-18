/**
 * Pure helper functions for segment display logic.
 *
 * Entry keys (include/exclude lists) are never fetched from the server — they
 * live in ScyllaDB and membership is evaluated server-side.  The admin UI
 * works with counts only (`include_count` / `exclude_count`).
 */

import type { Segment } from './types'

/**
 * Returns true when the segment stores its membership as a server-side list.
 * Detection is based on `segment_type` — the deprecated `user_list` field is
 * always empty from the server and must NOT be used for this check.
 */
export function isListSegment(segment: Segment): boolean {
  return segment.segment_type === 'list'
}

/**
 * Extract the include / exclude counts from a segment.
 * Both are 0 for rule-based segments.
 */
export function getListCounts(segment: Segment): {
  includeCount: number
  excludeCount: number
} {
  return {
    includeCount: segment.include_count,
    excludeCount: segment.exclude_count,
  }
}
